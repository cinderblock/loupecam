//! The browser override: `cage` (a Wayland compositor that runs one fullscreen app) with
//! Chromium, for what the native UI can't show (PDFs, web pages) or, if configured, the
//! full web UI. The kiosk hands the display to it and takes it back when it exits.

use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

pub struct Browser {
    child: Child,
    /// A throwaway profile, deleted when the browser stops.
    temp_profile: Option<PathBuf>,
}

/// Chromium flags for an unattended fullscreen touch display.
const CHROMIUM_FLAGS: &[&str] = &[
    "--kiosk",
    "--ozone-platform=wayland",
    "--noerrdialogs",
    "--disable-infobars",
    "--no-first-run",
    "--disable-session-crashed-bubble",
    "--disable-features=Translate,TranslateUI",
    "--check-for-update-interval=31536000",
    "--password-store=basic",
    "--touch-events=enabled",
];

fn find(names: &[&str]) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).flat_map(|d| names.iter().map(move |n| d.join(n))).find(|p| p.is_file())
}

impl Browser {
    /// Launch on `url`. `command` overrides the default (`cage -d -- chromium …`); `{url}`
    /// and `{profile}` in it are substituted. `profile` keeps browser state between runs;
    /// `None` uses a fresh temporary profile (for untrusted content).
    pub fn launch(url: &str, command: Option<&str>, profile: Option<&Path>) -> anyhow::Result<Browser> {
        let temp_profile = match profile {
            Some(_) => None,
            None => {
                let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
                let p = std::env::temp_dir().join(format!("loupecam-cast-{}-{stamp}", std::process::id()));
                std::fs::create_dir_all(&p)?;
                Some(p)
            }
        };
        let profile = profile.map(Path::to_path_buf).or_else(|| temp_profile.clone()).unwrap();
        let argv: Vec<String> = match command {
            Some(c) => c.split_whitespace().map(|a| a.replace("{url}", url).replace("{profile}", &profile.to_string_lossy())).collect(),
            None => {
                let cage = find(&["cage"]).ok_or_else(|| anyhow::anyhow!("cage isn't installed (apt install cage)"))?;
                let chromium = find(&["chromium-browser", "chromium"]).ok_or_else(|| anyhow::anyhow!("Chromium isn't installed (apt install chromium)"))?;
                let mut v = vec![cage.to_string_lossy().into_owned(), "-d".into(), "--".into(), chromium.to_string_lossy().into_owned()];
                v.extend(CHROMIUM_FLAGS.iter().map(|s| s.to_string()));
                v.push(format!("--user-data-dir={}", profile.display()));
                v.push(url.to_string());
                v
            }
        };
        let (prog, args) = argv.split_first().ok_or_else(|| anyhow::anyhow!("empty browser command"))?;
        tracing::info!("browser: {prog} … {url}");
        let child = Command::new(prog).args(args).spawn().map_err(|e| anyhow::anyhow!("starting {prog}: {e}"))?;
        Ok(Browser { child, temp_profile })
    }

    /// Whether the browser has exited on its own.
    pub fn exited(&mut self) -> bool {
        !matches!(self.child.try_wait(), Ok(None))
    }

    /// Stop it (SIGTERM, then SIGKILL after a few seconds) and wait.
    pub fn stop(mut self) {
        if !self.exited() {
            // SAFETY: signalling our own child.
            unsafe {
                libc::kill(self.child.id() as libc::pid_t, libc::SIGTERM);
            }
            let deadline = Instant::now() + Duration::from_secs(4);
            while Instant::now() < deadline && !self.exited() {
                std::thread::sleep(Duration::from_millis(50));
            }
            if !self.exited() {
                let _ = self.child.kill();
            }
        }
        let _ = self.child.wait();
        if let Some(p) = self.temp_profile.take() {
            let _ = std::fs::remove_dir_all(p);
        }
    }
}
