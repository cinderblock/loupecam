//! LoupeCam desktop app: runs the camera server on a private localhost port and shows
//! its web UI in a native window. Headless mode (`loupecam serve`) runs the same
//! server, so the two behave identically.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod updater;

use loupecam_server::{Config, Server, WebUi};
use std::sync::Mutex;
use std::sync::atomic::Ordering;
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::{AppHandle, Manager, RunEvent, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_opener::OpenerExt;

struct Running(Mutex<Option<Server>>);

/// Stop the embedded server (releasing the camera). Idempotent.
pub fn stop_server(app: &AppHandle) {
    if let Some(server) = app.state::<Running>().0.lock().unwrap().take() {
        let _ = tauri::async_runtime::block_on(server.stop());
    }
}

fn web_ui() -> WebUi {
    // Debug builds read the built UI from disk so it can be rebuilt without recompiling.
    if cfg!(debug_assertions) {
        return WebUi::Dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../web/dist").into());
    }
    #[cfg(feature = "embed-ui")]
    return WebUi::Embedded;
    #[allow(unreachable_code)]
    WebUi::Dir("web".into())
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(std::env::var("RUST_LOG").unwrap_or_else(|_| "warn,loupecam=info,loupecam_server=info".into()))
        .init();

    let captures_dir = Config::default_captures_dir();
    let cfg = Config {
        listen: "127.0.0.1:0".parse().unwrap(),
        token: None,
        web: web_ui(),
        captures_dir: captures_dir.clone(),
        settings_file: Config::default_settings_file(),
        // The Tauri updater owns updates for the desktop app.
        self_update: false,
        // Localhost only; nothing to announce.
        announce: None,
    };

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_dialog::init())
        .setup(move |app| {
            let prefs = updater::load_prefs();
            app.manage(updater::State::new(prefs.auto_install));
            let server = tauri::async_runtime::block_on(Server::start(cfg))?;
            let url = format!("http://{}/", server.addr);
            app.manage(Running(Mutex::new(Some(server))));

            let open_captures = MenuItem::with_id(app, "open-captures", "Open Captures Folder", true, Some("CmdOrCtrl+O"))?;
            let open_browser = MenuItem::with_id(app, "open-browser", "Open in Browser", true, None::<&str>)?;
            let check_updates = MenuItem::with_id(app, "check-updates", "Check for Updates…", true, None::<&str>)?;
            let auto_update = CheckMenuItem::with_id(app, "auto-update", "Automatically Install Updates", true, prefs.auto_install, None::<&str>)?;
            let file = Submenu::with_items(
                app,
                "File",
                true,
                &[
                    &open_captures,
                    &open_browser,
                    &PredefinedMenuItem::separator(app)?,
                    &check_updates,
                    &auto_update,
                    &PredefinedMenuItem::separator(app)?,
                    &PredefinedMenuItem::quit(app, None)?,
                ],
            )?;
            let menu = Menu::with_items(app, &[&file])?;

            WebviewWindowBuilder::new(app, "main", WebviewUrl::External(url.parse()?))
                .title("LoupeCam")
                .inner_size(1400.0, 900.0)
                .min_inner_size(800.0, 500.0)
                .menu(menu)
                .build()?;

            let dir = captures_dir.clone();
            app.on_menu_event(move |app, ev| match ev.id().as_ref() {
                "open-captures" => {
                    let _ = std::fs::create_dir_all(&dir);
                    if let Err(e) = app.opener().open_path(dir.to_string_lossy(), None::<&str>) {
                        tracing::warn!("opening captures folder: {e}");
                    }
                }
                "check-updates" => {
                    let app = app.clone();
                    tauri::async_runtime::spawn(async move { updater::check(&app, true).await });
                }
                "auto-update" => {
                    let on = auto_update.is_checked().unwrap_or(false);
                    app.state::<updater::State>().auto_install.store(on, Ordering::Release);
                    updater::save_prefs(&updater::Prefs { auto_install: on });
                }
                "open-browser" => {
                    if let Err(e) = app.opener().open_url(&url, None::<&str>) {
                        tracing::warn!("opening browser: {e}");
                    }
                }
                _ => {}
            });
            updater::spawn(app.handle().clone());
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("failed to build the app");

    app.run(|app, event| {
        if let RunEvent::Exit = event {
            // Release the camera cleanly.
            stop_server(app);
        }
    });
}
