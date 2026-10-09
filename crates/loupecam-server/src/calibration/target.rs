//! The on-screen calibration target: a full-screen web page (on any device) that shows
//! patterns on the runner's command. Used as an even light source and as a grid of
//! known pitch for scale calibration, when a screen can be placed under the optics.

use serde::Serialize;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;
use tokio::sync::watch;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum Pattern {
    /// Normal page with instructions.
    Idle,
    /// Solid grey, 0..1.
    Solid { level: f32 },
    /// White lines `line` device pixels wide every `period` device pixels, on black.
    Grid { period: u32, line: u32 },
}

pub struct TargetHub {
    /// `(command id, pattern)`; target pages render the latest.
    pub show: watch::Sender<(u64, Pattern)>,
    /// Latest id a target page has finished drawing.
    pub ack: watch::Sender<u64>,
    next: AtomicU64,
    pub connected: AtomicUsize,
}

/// Time for a screen to actually update after the page drew (display latency).
const DISPLAY_SETTLE: Duration = Duration::from_millis(400);

impl Default for TargetHub {
    fn default() -> Self {
        TargetHub { show: watch::Sender::new((0, Pattern::Idle)), ack: watch::Sender::new(0), next: AtomicU64::new(1), connected: AtomicUsize::new(0) }
    }
}

impl TargetHub {
    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Acquire) > 0
    }

    /// Show `p` on the target page(s) and wait until one has drawn it.
    pub async fn show(&self, p: Pattern) -> Result<(), String> {
        if !self.is_connected() {
            return Err("no screen target is open (open the target page on the screen under the scope)".into());
        }
        let id = self.next.fetch_add(1, Ordering::AcqRel);
        let mut ack = self.ack.subscribe();
        self.show.send_replace((id, p));
        tokio::time::timeout(Duration::from_secs(5), ack.wait_for(|a| *a >= id))
            .await
            .map_err(|_| "the screen target did not respond".to_string())?
            .map_err(|_| "target closed".to_string())?;
        tokio::time::sleep(DISPLAY_SETTLE).await;
        Ok(())
    }

    pub fn idle(&self) {
        let id = self.next.fetch_add(1, Ordering::AcqRel);
        self.show.send_replace((id, Pattern::Idle));
    }
}
