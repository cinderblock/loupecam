//! Headless network service for AmScope / ToupTek cameras.
//!
//! Runs the camera, publishes a live preview (MJPEG and WebSocket), exposes every
//! setting over a small JSON API, saves captures, and can serve the web UI. The
//! desktop app embeds this same server, so both modes share one code path.
//!
//! See [`http`] for the API surface.

pub mod captures;
pub mod geometry;
pub mod http;
pub mod preview;
pub mod service;
pub mod settings;

use service::Service;
use settings::Settings;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

#[cfg(feature = "embed-ui")]
#[derive(rust_embed::Embed)]
#[folder = "$CARGO_MANIFEST_DIR/../../apps/web/dist"]
struct Assets;

/// Where the web UI comes from.
#[derive(Debug, Clone)]
pub enum WebUi {
    Disabled,
    /// Serve a built UI from a directory (e.g. `apps/web/dist`).
    Dir(PathBuf),
    /// Serve the UI compiled into the binary (`embed-ui` feature).
    #[cfg(feature = "embed-ui")]
    Embedded,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub listen: SocketAddr,
    /// Require this bearer token for the API and streams.
    pub token: Option<String>,
    pub web: WebUi,
    pub captures_dir: PathBuf,
    /// Where settings are persisted (JSON). `None` = don't persist.
    pub settings_file: Option<PathBuf>,
}

impl Config {
    pub fn default_captures_dir() -> PathBuf {
        dirs::picture_dir().unwrap_or_else(|| PathBuf::from(".")).join("AmScope")
    }

    pub fn default_settings_file() -> Option<PathBuf> {
        dirs::config_dir().map(|d| d.join("amscope").join("settings.json"))
    }
}

/// Shared state for request handlers.
#[derive(Clone)]
pub struct AppState {
    pub service: Arc<Service>,
    pub preview: Arc<preview::Preview>,
    pub token: Option<Arc<str>>,
    pub web: WebUi,
    pub captures_dir: PathBuf,
    settings_file: Option<PathBuf>,
}

impl AppState {
    /// Save settings (best effort).
    pub fn persist(&self, s: &Settings) {
        let Some(path) = &self.settings_file else { return };
        let r = path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::write(path, serde_json::to_vec_pretty(s).unwrap()));
        if let Err(e) = r {
            tracing::warn!("saving settings to {}: {e}", path.display());
        }
    }
}

fn load_settings(path: Option<&PathBuf>) -> Settings {
    let Some(path) = path else { return Settings::default() };
    let Ok(bytes) = std::fs::read(path) else { return Settings::default() };
    // Merge over defaults so files from older versions (missing fields) still load.
    match serde_json::from_slice::<serde_json::Value>(&bytes).map(|v| Settings::default().patched(&v)) {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => {
            tracing::warn!("ignoring settings in {}: {e}", path.display());
            Settings::default()
        }
        Err(e) => {
            tracing::warn!("ignoring settings in {}: {e}", path.display());
            Settings::default()
        }
    }
}

/// A running server.
pub struct Server {
    pub addr: SocketAddr,
    pub state: AppState,
    shutdown: tokio::sync::oneshot::Sender<()>,
    task: tokio::task::JoinHandle<std::io::Result<()>>,
}

impl Server {
    /// Bind and start serving. Must be called inside a tokio runtime.
    pub async fn start(cfg: Config) -> std::io::Result<Server> {
        let settings = load_settings(cfg.settings_file.as_ref());
        let service = Service::spawn(settings);
        let preview = preview::spawn(service.shared.clone());
        let state = AppState {
            service,
            preview,
            token: cfg.token.map(Into::into),
            web: cfg.web,
            captures_dir: cfg.captures_dir,
            settings_file: cfg.settings_file,
        };
        let app = http::router(state.clone())
            .layer(tower_http::cors::CorsLayer::permissive())
            .layer(tower_http::trace::TraceLayer::new_for_http());
        let listener = tokio::net::TcpListener::bind(cfg.listen).await?;
        let addr = listener.local_addr()?;
        let (shutdown, rx) = tokio::sync::oneshot::channel::<()>();
        let task = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    let _ = rx.await;
                })
                .await
        });
        tracing::info!("listening on http://{addr}");
        Ok(Server { addr, state, shutdown, task })
    }

    /// Stop serving and release the camera.
    pub async fn stop(self) -> std::io::Result<()> {
        let _ = self.shutdown.send(());
        let r = self.task.await.map_err(std::io::Error::other)?;
        let service = self.state.service.clone();
        tokio::task::spawn_blocking(move || service.shutdown()).await.map_err(std::io::Error::other)?;
        r
    }
}
