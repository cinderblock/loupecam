//! Preview pipeline: turns the latest raw frame into a downscaled JPEG, but only while
//! someone is watching (each [`ViewerGuard`] counts as a viewer).

use crate::service::{FrameEnvelope, Shared};
use crate::settings::{DemosaicSetting, Settings};
use loupecam_isp::{Demosaic, develop, downscale, encode};
use bytes::Bytes;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::{Notify, watch};

#[derive(Debug, Clone)]
pub struct PreviewFrame {
    pub jpeg: Bytes,
    pub width: u32,
    pub height: u32,
    pub sequence: u32,
}

pub struct Preview {
    pub frames: watch::Sender<Option<Arc<PreviewFrame>>>,
    viewers: AtomicUsize,
    wake: Notify,
}

/// Keeps the preview pipeline running while alive.
pub struct ViewerGuard(Arc<Preview>);

impl Drop for ViewerGuard {
    fn drop(&mut self) {
        self.0.viewers.fetch_sub(1, Ordering::AcqRel);
    }
}

impl Preview {
    pub fn viewer(self: &Arc<Self>) -> ViewerGuard {
        self.viewers.fetch_add(1, Ordering::AcqRel);
        self.wake.notify_one();
        ViewerGuard(self.clone())
    }

    /// Wait for the next preview frame newer than the current one (starting the
    /// pipeline if idle).
    pub async fn next(self: &Arc<Self>, timeout: Duration) -> Option<Arc<PreviewFrame>> {
        let _guard = self.viewer();
        let mut rx = self.frames.subscribe();
        rx.mark_unchanged();
        tokio::time::timeout(timeout, rx.changed()).await.ok()?.ok()?;
        rx.borrow().clone()
    }
}

/// Start the preview task.
pub fn spawn(shared: Arc<Shared>) -> Arc<Preview> {
    let preview = Arc::new(Preview { frames: watch::Sender::new(None), viewers: AtomicUsize::new(0), wake: Notify::new() });
    let p = preview.clone();
    tokio::spawn(async move {
        let mut frames = shared.frame.subscribe();
        let mut last = Instant::now() - Duration::from_secs(1);
        loop {
            if p.viewers.load(Ordering::Acquire) == 0 {
                p.wake.notified().await;
                frames.mark_unchanged();
                continue;
            }
            if frames.changed().await.is_err() {
                return;
            }
            let settings = shared.state.borrow().settings.clone();
            let min_interval = Duration::from_secs_f32(1.0 / settings.preview.max_fps.clamp(0.5, 120.0));
            let since = last.elapsed();
            if since < min_interval {
                tokio::time::sleep(min_interval - since).await;
            }
            let Some(env) = frames.borrow_and_update().clone() else { continue };
            last = Instant::now();
            let cal = shared.calibration.borrow().clone();
            match tokio::task::spawn_blocking(move || render(&env, &settings, &cal)).await {
                Ok(Ok(frame)) => {
                    p.frames.send_replace(Some(Arc::new(frame)));
                }
                Ok(Err(e)) => tracing::warn!("preview: {e}"),
                Err(e) => tracing::warn!("preview task: {e}"),
            }
        }
    });
    preview
}

/// Develop a frame for display: superpixel demosaic when we'd downscale anyway (it is
/// both faster and alias-free), then box-downscale to the preview width.
pub fn render(env: &FrameEnvelope, s: &Settings, cal: &crate::calibration::Calibration) -> Result<PreviewFrame, encode::EncodeError> {
    let f = &env.raw;
    let max_w = s.preview.max_width.max(160);
    let factor = f.width.div_ceil(max_w).max(1);
    let demosaic = if factor >= 2 { DemosaicSetting::Superpixel } else { s.preview.demosaic };
    let p = s.develop_params(env, demosaic, cal);
    let img = develop(f, &p);
    let rest = if p.demosaic == Demosaic::Superpixel { factor.div_ceil(2) } else { factor };
    let img = downscale(&img, rest);
    let jpeg = encode::jpeg(&img, s.preview.jpeg_quality)?;
    Ok(PreviewFrame { jpeg: Bytes::from(jpeg), width: img.width, height: img.height, sequence: f.trailer.sequence })
}
