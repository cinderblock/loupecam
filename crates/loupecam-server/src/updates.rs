//! Periodic update checks and (opt-in) automatic installation for the headless server.
//!
//! The checks run whenever self-update is enabled for this process ([`crate::Config`]).
//! Installing replaces the executable and then asks the embedding program to restart
//! (see [`crate::Server::restart_requested`]), which releases the camera first.

use crate::service::Service;
use serde::Serialize;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Notify, watch};

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateStatus {
    /// Whether this process can update itself (false e.g. inside the desktop app, whose
    /// own updater handles it).
    pub enabled: bool,
    pub current_version: String,
    pub checking: bool,
    pub installing: bool,
    pub available: Option<loupecam_update::Release>,
    /// RFC 3339 time of the last completed check.
    pub last_check: Option<String>,
    pub last_error: Option<String>,
}

pub struct Updates {
    pub status: watch::Sender<UpdateStatus>,
    check_now: Notify,
    /// Set when an update was installed and the process should restart.
    pub restart: Notify,
    service: Arc<Service>,
}

impl Updates {
    pub fn spawn(enabled: bool, service: Arc<Service>) -> Arc<Self> {
        let u = Arc::new(Updates {
            status: watch::Sender::new(UpdateStatus {
                enabled,
                current_version: loupecam_update::current_version().to_string(),
                ..Default::default()
            }),
            check_now: Notify::new(),
            restart: Notify::new(),
            service,
        });
        if enabled {
            let me = u.clone();
            tokio::spawn(async move { me.run().await });
        }
        u
    }

    /// Check now (returns once the check has finished).
    pub async fn check(&self) -> UpdateStatus {
        if !self.status.borrow().enabled {
            return self.status.borrow().clone();
        }
        let mut rx = self.status.subscribe();
        self.check_now.notify_one();
        // Wait for this check to start and finish.
        let _ = rx.wait_for(|s| s.checking).await;
        let _ = rx.wait_for(|s| !s.checking).await;
        self.status.borrow().clone()
    }

    async fn run(self: Arc<Self>) {
        // A little after start-up, then on the configured interval.
        let mut first = true;
        loop {
            let hours = self.service.shared.state.borrow().settings.updates.check_interval_hours.clamp(0.25, 24.0 * 30.0);
            let wait = if first { Duration::from_secs(30) } else { Duration::from_secs_f32(hours * 3600.0) };
            first = false;
            tokio::select! {
                _ = tokio::time::sleep(wait) => {}
                _ = self.check_now.notified() => {}
            }
            self.status.send_modify(|s| s.checking = true);
            let r = tokio::task::spawn_blocking(loupecam_update::check).await;
            let now = jiff::Timestamp::now().to_string();
            match r {
                Ok(Ok(available)) => self.status.send_modify(|s| {
                    s.checking = false;
                    s.available = available;
                    s.last_check = Some(now);
                    s.last_error = None;
                }),
                Ok(Err(e)) => self.status.send_modify(|s| {
                    s.checking = false;
                    s.last_error = Some(e.to_string());
                }),
                Err(e) => self.status.send_modify(|s| {
                    s.checking = false;
                    s.last_error = Some(e.to_string());
                }),
            }
            let st = self.status.borrow().clone();
            if let Some(rel) = &st.available {
                tracing::info!(version = %rel.version, "update available");
                let auto = self.service.shared.state.borrow().settings.updates.auto_install;
                if auto && rel.installable
                    && let Err(e) = self.install().await
                {
                    tracing::warn!("automatic update failed: {e}");
                }
            }
        }
    }

    /// Install the available update and request a restart.
    pub async fn install(&self) -> Result<String, String> {
        let st = self.status.borrow().clone();
        if !st.enabled {
            return Err("updates are managed by the desktop app".into());
        }
        let rel = st.available.ok_or("no update available")?;
        if st.installing {
            return Err("already installing".into());
        }
        self.status.send_modify(|s| s.installing = true);
        let r2 = rel.clone();
        let r = tokio::task::spawn_blocking(move || loupecam_update::install(&r2))
            .await
            .map_err(|e| e.to_string())
            .and_then(|r| r.map_err(|e| e.to_string()));
        match r {
            Ok(()) => {
                self.restart.notify_one();
                Ok(rel.version)
            }
            Err(e) => {
                self.status.send_modify(|s| {
                    s.installing = false;
                    s.last_error = Some(e.clone());
                });
                Err(e)
            }
        }
    }
}
