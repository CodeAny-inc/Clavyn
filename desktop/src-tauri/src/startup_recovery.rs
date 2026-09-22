//! Starting when a state file cannot be loaded.
//!
//! The state files load fail closed: an unreadable `store.json`, `vault.json`
//! or `known_hosts.json` is refused rather than treated as empty, because
//! starting empty would overwrite it on the next save. Refusing it must not end
//! startup before a window exists, so the app opens with only this module's
//! commands available, says which file could not be read and why, and lets the
//! user move that file aside and restart.
//!
//! The file that gets moved is the one recorded at startup, never a path the
//! page supplies, and it is renamed rather than deleted, so whatever it held is
//! still there to recover.

use clavyn_core::CoreError;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tauri::{AppHandle, State};

type ApiResult<T> = std::result::Result<T, String>;

/// Why startup stopped short of loading the app state.
struct StartupFailure {
    /// The unreadable file, when the failure is one file that could not be
    /// parsed. Other failures (an unreadable directory, say) name no file and
    /// offer no move.
    file: Option<PathBuf>,
    reason: String,
}

/// Whether the app state loaded, and if not, what stopped it.
pub struct StartupStatus {
    failure: Mutex<Option<StartupFailure>>,
}

impl StartupStatus {
    pub fn ready() -> Self {
        Self {
            failure: Mutex::new(None),
        }
    }

    pub fn failed(error: &CoreError) -> Self {
        let failure = match error {
            CoreError::CorruptState { path, reason } => StartupFailure {
                file: Some(PathBuf::from(path)),
                reason: reason.clone(),
            },
            other => StartupFailure {
                file: None,
                reason: other.to_string(),
            },
        };
        Self {
            failure: Mutex::new(Some(failure)),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Option<StartupFailure>> {
        self.failure.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[derive(Serialize)]
pub struct StartupFailureView {
    /// Full path of the unreadable file, if the failure is one file.
    pub file: Option<String>,
    /// Its name alone, for the heading.
    pub file_name: Option<String>,
    pub reason: String,
}

/// What stopped startup, or `None` when the app state loaded normally.
#[tauri::command]
pub fn startup_failure(status: State<'_, StartupStatus>) -> Option<StartupFailureView> {
    status.lock().as_ref().map(|failure| StartupFailureView {
        file: failure.file.as_ref().map(|f| f.display().to_string()),
        file_name: failure
            .file
            .as_ref()
            .and_then(|f| f.file_name())
            .map(|name| name.to_string_lossy().into_owned()),
        reason: failure.reason.clone(),
    })
}

/// Rename the unreadable file out of the way and return where it went. The
/// next start then finds no file there and begins without it.
#[tauri::command]
pub fn set_aside_unreadable_file(status: State<'_, StartupStatus>) -> ApiResult<String> {
    let mut failure = status.lock();
    let file = failure
        .as_ref()
        .and_then(|f| f.file.clone())
        .ok_or("there is no unreadable file to move aside")?;
    let moved = set_aside(&file, unix_seconds()).map_err(|e| {
        format!("could not move {} aside: {e}", file.display())
    })?;
    // The file is no longer there, so a second call has nothing to move.
    if let Some(failure) = failure.as_mut() {
        failure.file = None;
    }
    Ok(moved.display().to_string())
}

/// Restart the app so it loads its state again, with the unreadable file out
/// of the way or its cause fixed.
#[tauri::command]
pub fn restart_app(app: AppHandle) {
    app.request_restart();
}

fn unix_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Rename `file` to `<name>.unreadable-<seconds>` beside it, never replacing
/// an existing file.
fn set_aside(file: &Path, seconds: u64) -> std::io::Result<PathBuf> {
    let name = file
        .file_name()
        .ok_or_else(|| std::io::Error::other("the path names no file"))?
        .to_string_lossy()
        .into_owned();
    for attempt in 0..100u32 {
        let suffix = match attempt {
            0 => format!("{name}.unreadable-{seconds}"),
            n => format!("{name}.unreadable-{seconds}-{n}"),
        };
        let target = file.with_file_name(suffix);
        if target.exists() {
            continue;
        }
        std::fs::rename(file, &target)?;
        return Ok(target);
    }
    Err(std::io::Error::other("no free name to move the file to"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_parse_failure_names_its_file() {
        let status = StartupStatus::failed(&CoreError::CorruptState {
            path: "/data/vault.json".into(),
            reason: "expected value".into(),
        });
        let failure = status.lock();
        let failure = failure.as_ref().expect("failure");
        assert_eq!(failure.file.as_deref(), Some(Path::new("/data/vault.json")));
        assert_eq!(failure.reason, "expected value");
    }

    #[test]
    fn other_failures_name_no_file() {
        let status = StartupStatus::failed(&CoreError::Io(std::io::Error::other("denied")));
        assert!(status.lock().as_ref().expect("failure").file.is_none());
        assert!(StartupStatus::ready().lock().is_none());
    }

    #[test]
    fn the_file_is_renamed_beside_itself_and_kept() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("store.json");
        std::fs::write(&file, "{ broken").expect("seed");

        let moved = set_aside(&file, 1700000000).expect("set aside");

        assert!(!file.exists());
        assert_eq!(moved, dir.path().join("store.json.unreadable-1700000000"));
        assert_eq!(std::fs::read_to_string(&moved).expect("read"), "{ broken");
    }

    #[test]
    fn an_earlier_copy_is_never_replaced() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("vault.json");
        let earlier = dir.path().join("vault.json.unreadable-5");
        std::fs::write(&earlier, "earlier copy").expect("seed");
        std::fs::write(&file, "{ broken").expect("seed");

        let moved = set_aside(&file, 5).expect("set aside");

        assert_eq!(moved, dir.path().join("vault.json.unreadable-5-1"));
        assert_eq!(std::fs::read_to_string(&earlier).expect("read"), "earlier copy");
    }
}
