use crate::keepass::Entry;
use std::sync::atomic::AtomicBool;
use std::sync::Mutex;
use std::time::Instant;
use tauri::menu::MenuItem;
use tauri::Wry;
use zeroize::Zeroize;

#[derive(Default, Clone)]
pub struct Session {
    pub password: Option<String>,
    pub entries: Vec<Entry>,
    pub last_activity: Option<Instant>,
}

impl Session {
    pub fn touch(&mut self) {
        self.last_activity = Some(Instant::now());
    }

    /// Overwrites the master password in memory before dropping it, instead of
    /// just releasing the allocation with the bytes still intact.
    pub fn lock(&mut self) {
        if let Some(mut pw) = self.password.take() {
            pw.zeroize();
        }
        self.entries.clear();
        self.last_activity = None;
    }
}

/// Menu items whose state follows the lock status.
pub struct TrayItems {
    pub status: MenuItem<Wry>,
    pub lock: MenuItem<Wry>,
}

#[derive(Default)]
pub struct AppState {
    pub tray: Mutex<Option<TrayItems>>,
    pub session: Mutex<Session>,
    /// Set while a native dialog (e.g. the file picker) is open, so the
    /// window's blur handler doesn't hide the window out from under it.
    pub dialog_open: AtomicBool,
}
