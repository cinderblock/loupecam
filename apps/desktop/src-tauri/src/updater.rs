//! Desktop updates via `tauri-plugin-updater` (signed with the same key as the CLI).
//! Checks shortly after launch and then every few hours; installs without asking only
//! if the user turned on "Automatically Install Updates".

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tauri::{AppHandle, Manager};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
use tauri_plugin_updater::UpdaterExt;

const FIRST_CHECK: Duration = Duration::from_secs(20);
const INTERVAL: Duration = Duration::from_secs(6 * 3600);

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Prefs {
    pub auto_install: bool,
}

fn prefs_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("loupecam").join("desktop.json"))
}

pub fn load_prefs() -> Prefs {
    prefs_path()
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

pub fn save_prefs(p: &Prefs) {
    let Some(path) = prefs_path() else { return };
    let r = path
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::write(&path, serde_json::to_vec_pretty(p).unwrap()));
    if let Err(e) = r {
        tracing::warn!("saving {}: {e}", path.display());
    }
}

/// Shared updater state.
pub struct State {
    pub auto_install: AtomicBool,
    busy: AtomicBool,
}

impl State {
    pub fn new(auto_install: bool) -> Self {
        State { auto_install: AtomicBool::new(auto_install), busy: AtomicBool::new(false) }
    }
}

/// Start the periodic checks.
pub fn spawn(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(FIRST_CHECK).await;
        loop {
            check(&app, false).await;
            tokio::time::sleep(INTERVAL).await;
        }
    });
}

/// Check for an update. `interactive`: the user asked (menu), so also report "up to
/// date" and errors.
pub async fn check(app: &AppHandle, interactive: bool) {
    let state = app.state::<State>();
    if state.busy.swap(true, Ordering::AcqRel) {
        return;
    }
    let r = check_inner(app, interactive).await;
    state.busy.store(false, Ordering::Release);
    if let Err(e) = r {
        tracing::warn!("update check: {e}");
        if interactive {
            message(app, MessageDialogKind::Error, "Update check failed", &e.to_string()).await;
        }
    }
}

async fn check_inner(app: &AppHandle, interactive: bool) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let current = app.package_info().version.to_string();
    let Some(update) = app.updater()?.check().await? else {
        if interactive {
            message(app, MessageDialogKind::Info, "No updates", &format!("LoupeCam {current} is the latest version.")).await;
        }
        return Ok(());
    };
    let auto = app.state::<State>().auto_install.load(Ordering::Acquire);
    if !auto {
        let notes = update.body.clone().unwrap_or_default();
        let text = format!(
            "LoupeCam {} is available (you have {current}).\n\n{}\n\nInstall it now? LoupeCam will restart.",
            update.version,
            notes.chars().take(800).collect::<String>()
        );
        if !ask(app, "Update available", &text, "Install and restart", "Later").await {
            return Ok(());
        }
    }
    tracing::info!(version = %update.version, "installing update");
    update.download_and_install(|_, _| {}, || {}).await?;
    // Release the camera before the new version starts.
    crate::stop_server(app);
    app.restart();
}

async fn message(app: &AppHandle, kind: MessageDialogKind, title: &str, text: &str) {
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog().message(text).title(title).kind(kind).show(move |_| {
        let _ = tx.send(true);
    });
    let _ = rx.await;
}

async fn ask(app: &AppHandle, title: &str, text: &str, yes: &str, no: &str) -> bool {
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .message(text)
        .title(title)
        .buttons(MessageDialogButtons::OkCancelCustom(yes.into(), no.into()))
        .show(move |ok| {
            let _ = tx.send(ok);
        });
    rx.await.unwrap_or(false)
}
