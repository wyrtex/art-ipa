//! Start-up glue and shared plumbing for running the ArtCraft Crafting Apps on iOS / iPadOS.
//!
//! * [`init`]: the desktop apps treat every Unix that is not macOS like Linux: settings go to
//!   `$XDG_CONFIG_HOME` (or `~/.config`), caches to `$XDG_CACHE_HOME`, and "your files" means
//!   `$HOME`. In the iOS sandbox only `Documents`, `Library` and `tmp` inside the app container
//!   are writable, and only `Documents` is visible to the user (in the Files app). So settings
//!   and caches go to `Library`, and `$HOME` and the working directory become `Documents`.
//! * Picked files: iOS file pickers can't block the UI thread (winit's iOS event loop aborts on
//!   a nested run loop), so the `rfd` shim shows the picker, returns at once, and hands the
//!   chosen files over here; the `egui-winit` shim delivers them to the app on the next frame
//!   as files dropped on the window, which every Craft app already opens or imports.

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

/// Environment variable holding the app's `Documents` folder once [`init`] ran.
pub const DOCUMENTS_ENV: &str = "ART_IPA_DOCUMENTS";

/// Call first thing in `main`, before any other thread starts.
pub fn init() {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else { return };
    if std::env::var_os(DOCUMENTS_ENV).is_some() {
        return; // already done
    }
    let support = home.join("Library/Application Support");
    let caches = home.join("Library/Caches");
    let documents = home.join("Documents");
    for dir in [&support, &caches, &documents] {
        let _ = std::fs::create_dir_all(dir);
    }
    // Single-threaded at this point (see above).
    unsafe {
        set_default("XDG_CONFIG_HOME", &support);
        set_default("XDG_DATA_HOME", &support);
        set_default("XDG_STATE_HOME", &support);
        set_default("XDG_CACHE_HOME", &caches);
        std::env::set_var(DOCUMENTS_ENV, &documents);
        // "Home" in the apps (default folders of path fields, recent places) is what the user
        // can see: the app's folder in the Files app.
        std::env::set_var("HOME", &documents);
    }
    let _ = std::env::set_current_dir(&documents);
}

/// The folder the Files app shows for this app.
pub fn documents_dir() -> PathBuf {
    if let Some(d) = std::env::var_os(DOCUMENTS_ENV) {
        return PathBuf::from(d);
    }
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    if home.ends_with("Documents") { home } else { home.join("Documents") }
}

unsafe fn set_default(key: &str, value: &std::path::Path) {
    if std::env::var_os(key).is_none() {
        unsafe { std::env::set_var(key, value) };
    }
}

// ---------------------------------------------------------------------------------------------
// Files picked outside the frame, delivered as dropped files
// ---------------------------------------------------------------------------------------------

static PICKED: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());
static WAKER: OnceLock<Box<dyn Fn() + Send + Sync>> = OnceLock::new();

/// Queue files for the app; they arrive on the next frame as dropped files.
pub fn deliver_files(paths: Vec<PathBuf>) {
    if paths.is_empty() {
        return;
    }
    PICKED.lock().unwrap_or_else(|e| e.into_inner()).extend(paths);
    if let Some(wake) = WAKER.get() {
        wake();
    }
}

/// Files queued by [`deliver_files`] since the last call.
pub fn take_files() -> Vec<PathBuf> {
    std::mem::take(&mut *PICKED.lock().unwrap_or_else(|e| e.into_inner()))
}

/// How to ask the UI for a new frame (set once, by the `egui-winit` shim).
pub fn set_waker(wake: Box<dyn Fn() + Send + Sync>) {
    let _ = WAKER.set(wake);
}

// ---------------------------------------------------------------------------------------------
// Fitting desktop UIs on a phone / tablet screen
// ---------------------------------------------------------------------------------------------

/// The Craft apps are laid out for desktop windows of 1280+ points (their menu bars and
/// toolbars overlap below that). On screens narrower than this (after the safe area), the UI is
/// drawn at a smaller scale so that much fits.
pub const MIN_LOGICAL_WIDTH: f32 = 1280.0;

/// Smallest scale applied, so text stays readable on the smallest phones.
pub const MIN_UI_SCALE: f32 = 0.58;

/// The factor to multiply the display's scale with, for a window `long_side_points` wide (its
/// longer side, in UIKit points, minus the safe area). `ART_IPA_UI_SCALE` overrides it.
pub fn ui_scale(long_side_points: f32) -> f32 {
    if let Some(s) = std::env::var("ART_IPA_UI_SCALE").ok().and_then(|s| s.parse::<f32>().ok()).filter(|s| *s > 0.1) {
        return s;
    }
    if long_side_points <= 0.0 {
        return 1.0;
    }
    (long_side_points / MIN_LOGICAL_WIDTH).clamp(MIN_UI_SCALE, 1.0)
}
