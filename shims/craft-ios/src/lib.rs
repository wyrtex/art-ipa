//! Start-up glue for running the ArtCraft Crafting Apps on iOS / iPadOS.
//!
//! The desktop apps treat every Unix that is not macOS like Linux: settings go to
//! `$XDG_CONFIG_HOME` (or `~/.config`), caches to `$XDG_CACHE_HOME`, and relative paths are
//! resolved against the working directory. In the iOS sandbox only `Documents`, `Library` and
//! `tmp` inside the app container are writable, so [`init`] points those variables there before
//! the app reads them, and makes `Documents` (shown in the Files app) the working directory.

use std::path::{Path, PathBuf};

/// Call first thing in `main`, before any other thread starts.
pub fn init() {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else { return };
    let support = home.join("Library/Application Support");
    let caches = home.join("Library/Caches");
    let documents = home.join("Documents");
    for dir in [&support, &caches, &documents] {
        let _ = std::fs::create_dir_all(dir);
    }
    set_default("XDG_CONFIG_HOME", &support);
    set_default("XDG_DATA_HOME", &support);
    set_default("XDG_STATE_HOME", &support);
    set_default("XDG_CACHE_HOME", &caches);
    let _ = std::env::set_current_dir(&documents);
}

/// The folder the Files app shows for this app.
pub fn documents_dir() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default().join("Documents")
}

fn set_default(key: &str, value: &Path) {
    if std::env::var_os(key).is_none() {
        // Single-threaded at this point (see `init`).
        unsafe { std::env::set_var(key, value) };
    }
}
