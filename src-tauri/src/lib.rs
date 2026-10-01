mod commands;
mod config;
mod keepass;
mod state;

use state::AppState;
use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::TrayIconBuilder,
    Emitter, Manager,
};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

pub(crate) fn show_main_window(app: &tauri::AppHandle) {
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.center();
        let _ = win.show();
        let _ = win.set_focus();
        app.state::<AppState>().session.lock().unwrap().touch();
        let _ = app.emit("window-shown", ());
    }
}

/// Keeps the tray menu in sync with the lock state: "Lock" is greyed out
/// while locked, and the status line shows the current database.
pub(crate) fn refresh_tray(app: &tauri::AppHandle) {
    let state = app.state::<AppState>();
    let unlocked = state.session.lock().unwrap().password.is_some();
    let name = config::load(app)
        .db_path
        .and_then(|p| std::path::Path::new(&p).file_name().map(|n| n.to_string_lossy().to_string()))
        .unwrap_or_else(|| "No database".to_string());
    let tray = state.tray.lock().unwrap();
    if let Some(items) = tray.as_ref() {
        let _ = items.lock.set_enabled(unlocked);
        let _ = items
            .status
            .set_text(format!("{}: {}", if unlocked { "Unlocked" } else { "Locked" }, name));
    }
}

fn toggle_main_window(app: &tauri::AppHandle) {
    if let Some(win) = app.get_webview_window("main") {
        if win.is_visible().unwrap_or(false) {
            let _ = win.hide();
        } else {
            show_main_window(app);
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(AppState::default())
        .plugin(tauri_plugin_positioner::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, _shortcut, event| {
                    if event.state() == ShortcutState::Pressed {
                        toggle_main_window(app);
                    }
                })
                .build(),
        )
        .invoke_handler(tauri::generate_handler![
            commands::get_config,
            commands::set_db_path,
            commands::pick_db_file,
            commands::is_unlocked,
            commands::unlock,
            commands::lock,
            commands::get_entries,
            commands::copy_password,
            commands::copy_username,
            commands::copy_totp,
            commands::hide_main_window,
        ])
        .setup(|app| {
            let shortcut = Shortcut::new(Some(Modifiers::SUPER | Modifiers::SHIFT), Code::Space);
            app.global_shortcut().register(shortcut)?;

            let quit = MenuItem::with_id(app, "quit", "Quit KeeSpot", true, None::<&str>)?;
            let show = MenuItem::with_id(app, "show", "Show KeeSpot", true, None::<&str>)?;
            let lock = MenuItem::with_id(app, "lock", "Lock", false, None::<&str>)?;
            let choose = MenuItem::with_id(app, "choose_db", "Choose Database…", true, None::<&str>)?;
            let status = MenuItem::with_id(app, "status", "Locked", false, None::<&str>)?;
            let menu = Menu::with_items(
                app,
                &[
                    &status,
                    &PredefinedMenuItem::separator(app)?,
                    &show,
                    &lock,
                    &choose,
                    &PredefinedMenuItem::separator(app)?,
                    &quit,
                ],
            )?;
            *app.state::<AppState>().tray.lock().unwrap() = Some(state::TrayItems { status, lock });
            refresh_tray(app.handle());

            TrayIconBuilder::new()
                .icon(tauri::image::Image::from_bytes(include_bytes!("../icons/tray.png"))?)
                // Template image: macOS tints it for light/dark menu bars.
                .icon_as_template(true)
                .menu(&menu)
                .show_menu_on_left_click(true)
                .on_menu_event(move |app, event| match event.id.as_ref() {
                    "quit" => app.exit(0),
                    "show" => toggle_main_window(app),
                    "lock" => {
                        let state = app.state::<AppState>();
                        state.session.lock().unwrap().lock();
                        refresh_tray(app);
                        let _ = app.emit("locked", ());
                    }
                    "choose_db" => {
                        let app = app.clone();
                        tauri::async_runtime::spawn(commands::choose_database(app));
                    }
                    _ => {}
                })
                .build(app)?;

            // Auto-lock: poll for inactivity and wipe the session once it exceeds the limit.
            let lock_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                loop {
                    tokio::time::sleep(std::time::Duration::from_secs(10)).await;
                    let limit = config::load(&lock_handle).auto_lock_secs;
                    if limit == 0 {
                        continue;
                    }
                    let state = lock_handle.state::<AppState>();
                    let expired = {
                        let mut session = state.session.lock().unwrap();
                        let idle = session.last_activity.map(|t| t.elapsed().as_secs());
                        match idle {
                            Some(secs) if session.password.is_some() && secs >= limit as u64 => {
                                session.lock();
                                true
                            }
                            _ => false,
                        }
                    };
                    if expired {
                        refresh_tray(&lock_handle);
                        let _ = lock_handle.emit("locked", ());
                    }
                }
            });

            // Hide from Dock; this is a menubar-only utility, like Spotlight.
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            // Hide the window (instead of quitting) when it loses focus, unless a
            // native dialog (e.g. the file picker) is open: that dialog is attached
            // to this window and would close with it.
            if let Some(win) = app.get_webview_window("main") {
                let win_clone = win.clone();
                let app_handle = app.handle().clone();
                win.on_window_event(move |event| {
                    if let tauri::WindowEvent::Focused(false) = event {
                        let dialog_open = app_handle.state::<AppState>().dialog_open.load(std::sync::atomic::Ordering::SeqCst);
                        if !dialog_open {
                            let _ = win_clone.hide();
                        }
                    }
                });
            }

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
