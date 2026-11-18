//! iOS sandbox-compliant path resolution.
//!
//! The `dirs` crate returns paths outside the app sandbox on iOS,
//! causing `EPERM` on file operations. This module caches the
//! real sandbox paths (Documents, Caches) passed from Swift at
//! startup and provides fallback resolution.

use std::path::PathBuf;
use std::sync::OnceLock;

static IOS_CACHE_DIR: OnceLock<PathBuf> = OnceLock::new();
static IOS_DOCUMENTS_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Set the iOS Caches directory (called from Swift before `initialize`).
pub fn set_cache_dir(path: PathBuf) {
    let _ = IOS_CACHE_DIR.set(path);
}

/// Set the iOS Documents directory (called from Swift before `initialize`).
pub fn set_documents_dir(path: PathBuf) {
    let _ = IOS_DOCUMENTS_DIR.set(path);
}

/// Return the iOS Caches directory, or a sensible platform-appropriate fallback.
pub fn cache_dir() -> PathBuf {
    IOS_CACHE_DIR
        .get()
        .cloned()
        .or_else(|| {
            if cfg!(target_os = "ios") {
                None
            } else {
                dirs::cache_dir()
            }
        })
        .unwrap_or_else(std::env::temp_dir)
}

/// Return the iOS Documents directory, or a sensible platform-appropriate fallback.
pub fn documents_dir() -> PathBuf {
    IOS_DOCUMENTS_DIR
        .get()
        .cloned()
        .or_else(|| {
            if cfg!(target_os = "ios") {
                None
            } else {
                dirs::document_dir()
            }
        })
        .unwrap_or_else(std::env::temp_dir)
}
