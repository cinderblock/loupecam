//! A fullscreen touch UI for a LoupeCam server, for a small display attached to the
//! machine running the camera (e.g. the Raspberry Pi 7" touch display).
//!
//! On Linux it draws straight to the display through DRM/KMS and reads the touchscreen
//! through evdev: no X server, no Wayland compositor, no desktop. With the `window`
//! feature it runs in a desktop window instead, for development.
//!
//! The kiosk is a client of the server's HTTP/WebSocket API, so it runs as its own
//! process (and user) next to `loupecam serve`.

// Without a display backend (no `window` feature, not Linux) only `run` is reachable.
#![cfg_attr(not(any(feature = "window", target_os = "linux")), allow(dead_code))]

pub mod client;
mod gfx;
mod platform;
pub mod ui;
mod view;

use client::ServerEvent;
use std::time::Duration;
use ui::Input;

#[derive(Debug, Clone)]
pub struct Options {
    /// The server, e.g. `http://127.0.0.1`.
    pub server: String,
    /// The server's camera token, if it has one.
    pub token: Option<String>,
    /// The server's display token, if casting needs one (for the cast page in the
    /// browser, which never gets the camera token).
    pub display_token: Option<String>,
    /// What the screen shows when nothing is cast.
    pub home: Home,
    /// Browser command instead of `cage -d -- chromium --kiosk …` (`{url}` and
    /// `{profile}` are substituted).
    pub browser_command: Option<String>,
    /// Where the home browser keeps its profile (`Home::Browser`).
    pub browser_profile: Option<std::path::PathBuf>,
    /// Rotate the UI clockwise by 0, 90, 180 or 270 degrees (for how the display is
    /// mounted).
    pub rotation: u16,
    /// Run in a desktop window of this size instead of on the display hardware.
    pub window: Option<(u32, u32)>,
    /// Turn the backlight off after this long without a touch.
    pub sleep_after: Option<Duration>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Home {
    /// The native UI.
    #[default]
    Native,
    /// The full web UI, in the browser (`cage` + Chromium).
    Browser,
}

/// Everything the main loop reacts to.
#[derive(Debug)]
pub enum Event {
    Server(ServerEvent),
    Input(Input),
}

/// Run the kiosk until it fails (or its window is closed).
pub fn run(opts: Options) -> anyhow::Result<()> {
    if !matches!(opts.rotation, 0 | 90 | 180 | 270) {
        anyhow::bail!("rotation must be 0, 90, 180 or 270");
    }
    if let Some(size) = opts.window {
        #[cfg(feature = "window")]
        return platform::window::run(&opts, size);
        #[cfg(not(feature = "window"))]
        {
            let _ = size;
            anyhow::bail!("this build has no window support (the `window` feature)");
        }
    }
    #[cfg(target_os = "linux")]
    return platform::kms::run(&opts);
    #[cfg(not(target_os = "linux"))]
    anyhow::bail!("drawing to the display directly needs Linux; use --window here")
}

/// Where other devices can reach this server: `http://<LAN address>[:port] · <host>`.
fn address(server: &str) -> String {
    let host = gethostname::gethostname().to_string_lossy().into_owned();
    // The port the server listens on, from its URL.
    let port = server.rsplit(':').next().and_then(|p| p.trim_end_matches('/').parse::<u16>().ok()).filter(|&p| p != 80);
    // The address the OS would route outward from (no packet is sent).
    let ip = std::net::UdpSocket::bind("0.0.0.0:0").and_then(|s| s.connect("192.0.2.1:9").and_then(|()| s.local_addr())).map(|a| a.ip()).ok();
    match (ip, port) {
        (Some(ip), Some(p)) => format!("http://{ip}:{p} · {host}"),
        (Some(ip), None) => format!("http://{ip} · {host}"),
        (None, _) => host,
    }
}
