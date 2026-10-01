use crate::config::{self, Config};
use crate::keepass::{self, Entry};
use crate::state::AppState;
use tauri::Emitter;
use tauri::{AppHandle, Manager, State};
use tauri_plugin_clipboard_manager::ClipboardExt;

#[tauri::command]
pub fn get_config(app: AppHandle) -> Config {
    config::load(&app)
}

#[tauri::command]
pub fn set_db_path(app: AppHandle, path: String) -> Result<(), String> {
    let mut cfg = config::load(&app);
    cfg.db_path = Some(path);
    config::save(&app, &cfg)?;
    crate::refresh_tray(&app);
    Ok(())
}

async fn pick_db(app: &AppHandle) -> Option<String> {
    use std::sync::atomic::Ordering;
    use tauri_plugin_dialog::DialogExt;

    let state = app.state::<AppState>();
    state.dialog_open.store(true, Ordering::SeqCst);
    let app2 = app.clone();
    let file =
        tauri::async_runtime::spawn_blocking(move || app2.dialog().file().add_filter("KeePass database", &["kdbx"]).blocking_pick_file())
            .await
            .ok()
            .flatten();
    state.dialog_open.store(false, Ordering::SeqCst);
    file.map(|f| f.to_string())
}

#[tauri::command]
pub async fn pick_db_file(app: AppHandle) -> Result<Option<String>, ()> {
    Ok(pick_db(&app).await)
}

/// Tray "Choose Database…": pick a file, and only if one was chosen lock the
/// current session, switch to it and bring up the unlock prompt.
pub async fn choose_database(app: AppHandle) {
    let Some(path) = pick_db(&app).await else { return };
    let mut cfg = config::load(&app);
    cfg.db_path = Some(path);
    if config::save(&app, &cfg).is_err() {
        return;
    }
    app.state::<AppState>().session.lock().unwrap().lock();
    crate::refresh_tray(&app);
    let _ = app.emit("locked", ());
    crate::show_main_window(&app);
}

#[tauri::command]
pub fn is_unlocked(state: State<AppState>) -> bool {
    state.session.lock().unwrap().password.is_some()
}

/// Unlocking pays the KDF cost (deliberately ~1s by design) exactly once, via
/// a single `export` call; run off the main thread so the UI stays responsive.
#[tauri::command]
pub async fn unlock(app: AppHandle, state: State<'_, AppState>, password: String) -> Result<Vec<Entry>, String> {
    let cfg = config::load(&app);
    let db_path = cfg.db_path.ok_or("No database configured yet")?;

    let pw = password.clone();
    let entries = tauri::async_runtime::spawn_blocking(move || keepass::unlock_and_load(&db_path, &pw))
        .await
        .map_err(|e| e.to_string())??;

    let mut session = state.session.lock().unwrap();
    session.password = Some(password);
    session.entries = entries.clone();
    session.touch();
    drop(session);
    crate::refresh_tray(&app);

    Ok(entries)
}

#[tauri::command]
pub fn lock(app: AppHandle, state: State<AppState>) {
    state.session.lock().unwrap().lock();
    crate::refresh_tray(&app);
}

#[tauri::command]
pub fn get_entries(state: State<AppState>) -> Result<Vec<Entry>, String> {
    let session = state.session.lock().unwrap();
    if session.password.is_none() {
        return Err("locked".to_string());
    }
    Ok(session.entries.clone())
}

enum Attribute {
    Named(&'static str),
    Totp,
}

/// Fetches the requested value (fast / a single `show` call, not the
/// full-timeout-blocking `clip` command) and writes it to the clipboard
/// ourselves, then schedules a clear after the configured timeout; but only
/// if the clipboard still holds what we put there, so we don't clobber
/// something the user copied in the meantime.
async fn copy_to_clipboard(app: AppHandle, state: State<'_, AppState>, entry_path: String, attribute: Attribute) -> Result<(), String> {
    let cfg = config::load(&app);
    let db_path = cfg.db_path.ok_or("No database configured")?;
    let password = {
        let mut session = state.session.lock().unwrap();
        session.touch();
        session.password.clone().ok_or("locked")?
    };

    let value = tauri::async_runtime::spawn_blocking(move || match attribute {
        Attribute::Named(attr) => keepass::fetch_attribute(&db_path, &password, &entry_path, attr),
        Attribute::Totp => keepass::fetch_totp(&db_path, &password, &entry_path),
    })
    .await
    .map_err(|e| e.to_string())??;

    app.clipboard().write_text(value.clone()).map_err(|e| e.to_string())?;

    let app_for_clear = app.clone();
    let timeout_secs = cfg.clipboard_timeout_secs;
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(timeout_secs as u64)).await;
        if let Ok(current) = app_for_clear.clipboard().read_text() {
            if current == value {
                let _ = app_for_clear.clipboard().write_text(String::new());
            }
        }
    });

    Ok(())
}

#[tauri::command]
pub async fn copy_password(app: AppHandle, state: State<'_, AppState>, entry_path: String) -> Result<(), String> {
    copy_to_clipboard(app, state, entry_path, Attribute::Named("Password")).await
}

#[tauri::command]
pub async fn copy_username(app: AppHandle, state: State<'_, AppState>, entry_path: String) -> Result<(), String> {
    copy_to_clipboard(app, state, entry_path, Attribute::Named("UserName")).await
}

#[tauri::command]
pub async fn copy_totp(app: AppHandle, state: State<'_, AppState>, entry_path: String) -> Result<(), String> {
    copy_to_clipboard(app, state, entry_path, Attribute::Totp).await
}

#[tauri::command]
pub fn hide_main_window(app: AppHandle) {
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.hide();
    }
}
