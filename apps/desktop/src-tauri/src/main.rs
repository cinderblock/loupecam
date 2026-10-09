//! AmScope desktop app: runs the camera server on a private localhost port and shows
//! its web UI in a native window. Headless mode (`amscope serve`) runs the same
//! server, so the two behave identically.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use amscope_server::{Config, Server, WebUi};
use std::sync::Mutex;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::{Manager, RunEvent, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_opener::OpenerExt;

struct Running(Mutex<Option<Server>>);

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
        .with_env_filter(std::env::var("RUST_LOG").unwrap_or_else(|_| "warn,amscope=info,amscope_server=info".into()))
        .init();

    let captures_dir = Config::default_captures_dir();
    let cfg = Config {
        listen: "127.0.0.1:0".parse().unwrap(),
        token: None,
        web: web_ui(),
        captures_dir: captures_dir.clone(),
        settings_file: Config::default_settings_file(),
    };

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(move |app| {
            let server = tauri::async_runtime::block_on(Server::start(cfg))?;
            let url = format!("http://{}/", server.addr);
            app.manage(Running(Mutex::new(Some(server))));

            let open_captures = MenuItem::with_id(app, "open-captures", "Open Captures Folder", true, Some("CmdOrCtrl+O"))?;
            let open_browser = MenuItem::with_id(app, "open-browser", "Open in Browser", true, None::<&str>)?;
            let file = Submenu::with_items(
                app,
                "File",
                true,
                &[&open_captures, &open_browser, &PredefinedMenuItem::separator(app)?, &PredefinedMenuItem::quit(app, None)?],
            )?;
            let menu = Menu::with_items(app, &[&file])?;

            WebviewWindowBuilder::new(app, "main", WebviewUrl::External(url.parse()?))
                .title("AmScope")
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
                "open-browser" => {
                    if let Err(e) = app.opener().open_url(&url, None::<&str>) {
                        tracing::warn!("opening browser: {e}");
                    }
                }
                _ => {}
            });
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("failed to build the app");

    app.run(|app, event| {
        if let RunEvent::Exit = event {
            // Release the camera cleanly.
            if let Some(server) = app.state::<Running>().0.lock().unwrap().take() {
                let _ = tauri::async_runtime::block_on(server.stop());
            }
        }
    });
}
