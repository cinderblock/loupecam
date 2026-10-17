//! Where captures and recordings are saved: the internal captures directory, or a USB
//! stick.
//!
//! The stick is expected at a fixed directory (by default `/media/loupecam/usb`, where
//! the Linux packaging's udev rule automounts it). Files go in a `LoupeCam` folder on it.
//! Presence is read from the mount table rather than by touching the directory, so
//! checking doesn't keep an idle automount mounted (it unmounts itself when idle, which
//! makes pulling the stick between captures safe).

use crate::settings::UsbMode;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;

/// Folder on the stick that captures go into.
const USB_FOLDER: &str = "LoupeCam";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Location {
    Internal,
    Usb,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageStatus {
    /// Where a USB stick is looked for (`None`: USB storage isn't configured).
    pub usb_dir: Option<String>,
    /// A stick is inserted (and mounted, or mountable on demand).
    pub usb_present: bool,
}

pub struct Storage {
    internal: PathBuf,
    usb: Option<PathBuf>,
    pub status: watch::Sender<StorageStatus>,
}

impl Storage {
    /// Watches for the stick every couple of seconds. Must be called inside a tokio
    /// runtime.
    pub fn spawn(internal: PathBuf, usb: Option<PathBuf>) -> Arc<Storage> {
        let status = StorageStatus { usb_dir: usb.as_ref().map(|p| p.display().to_string()), usb_present: false };
        let s = Arc::new(Storage { internal, usb, status: watch::Sender::new(status) });
        if s.usb.is_some() {
            let weak = Arc::downgrade(&s);
            tokio::spawn(async move {
                while let Some(s) = weak.upgrade() {
                    let present = s.usb_present();
                    s.status.send_if_modified(|st| std::mem::replace(&mut st.usb_present, present) != present);
                    drop(s);
                    tokio::time::sleep(Duration::from_secs(2)).await;
                }
            });
        }
        s
    }

    pub fn internal(&self) -> &Path {
        &self.internal
    }

    /// Whether a stick is there.
    pub fn usb_present(&self) -> bool {
        self.usb.as_deref().is_some_and(present)
    }

    /// The directory for `loc`, if it exists right now.
    pub fn dir(&self, loc: Location) -> Option<PathBuf> {
        match loc {
            Location::Internal => Some(self.internal.clone()),
            Location::Usb => self.usb.as_ref().filter(|p| present(p)).map(|p| p.join(USB_FOLDER)),
        }
    }

    /// Where to save a new capture or recording under `mode`.
    pub fn target(&self, mode: UsbMode) -> Result<(PathBuf, Location), String> {
        let usb = || self.dir(Location::Usb).map(|d| (d, Location::Usb));
        let internal = || (self.internal.clone(), Location::Internal);
        match mode {
            UsbMode::Off => Ok(internal()),
            UsbMode::WhenAvailable => Ok(usb().unwrap_or_else(internal)),
            UsbMode::Always => match (&self.usb, usb()) {
                (None, _) => Err("USB storage isn't set up on this server (--usb-dir)".into()),
                (_, Some(t)) => Ok(t),
                (Some(_), None) => Err("No USB stick: insert one, or change where captures are saved".into()),
            },
        }
    }

    /// Every location that exists now, internal first.
    pub fn locations(&self) -> Vec<(Location, PathBuf)> {
        [Location::Internal, Location::Usb].into_iter().filter_map(|l| self.dir(l).map(|d| (l, d))).collect()
    }
}

/// Whether something is mounted on `dir` (Linux), or `dir` exists (elsewhere: e.g. a
/// drive letter's folder on Windows).
fn present(dir: &Path) -> bool {
    #[cfg(target_os = "linux")]
    {
        std::fs::read_to_string("/proc/self/mountinfo").is_ok_and(|m| mounted_at(&m, dir))
    }
    #[cfg(not(target_os = "linux"))]
    {
        dir.is_dir()
    }
}

/// Whether `/proc/self/mountinfo` text lists a mount (an automount trap counts) at `dir`.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn mounted_at(mountinfo: &str, dir: &Path) -> bool {
    let want = dir.to_string_lossy();
    let want = want.trim_end_matches('/');
    mountinfo.lines().filter_map(|l| l.split(' ').nth(4)).any(|mp| unescape(mp) == want)
}

/// Mount points in mountinfo escape space, tab, newline and backslash as `\ooo`.
fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c == '\\' {
            let oct: String = it.clone().take(3).collect();
            if oct.len() == 3
                && let Ok(v) = u8::from_str_radix(&oct, 8)
            {
                out.push(v as char);
                it.nth(2);
                continue;
            }
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const MOUNTINFO: &str = "22 1 179:2 / / rw,noatime shared:1 - ext4 /dev/root rw\n\
        41 22 0:38 / /media/loupecam/usb rw,relatime shared:20 - autofs systemd-1 rw,fd=50\n\
        52 22 8:1 / /media/My\\040Stick rw - vfat /dev/sda1 rw\n";

    #[test]
    fn finds_mounts() {
        assert!(mounted_at(MOUNTINFO, Path::new("/media/loupecam/usb")));
        assert!(mounted_at(MOUNTINFO, Path::new("/media/loupecam/usb/")));
        assert!(mounted_at(MOUNTINFO, Path::new("/media/My Stick")));
        assert!(!mounted_at(MOUNTINFO, Path::new("/media/loupecam")));
    }

    #[tokio::test]
    async fn targets() {
        let tmp = std::env::temp_dir().join(format!("loupecam-storage-{}", std::process::id()));
        let none = Storage::spawn(tmp.clone(), None);
        assert_eq!(none.target(UsbMode::WhenAvailable).unwrap().1, Location::Internal);
        assert!(none.target(UsbMode::Always).unwrap_err().contains("isn't set up"));
        let usb = tmp.join("no-such-stick");
        let missing = Storage::spawn(tmp.clone(), Some(usb));
        assert_eq!(missing.target(UsbMode::Off).unwrap().1, Location::Internal);
        assert_eq!(missing.target(UsbMode::WhenAvailable).unwrap().1, Location::Internal);
        assert!(missing.target(UsbMode::Always).unwrap_err().contains("No USB stick"));
        assert_eq!(missing.locations().len(), 1);
    }
}
