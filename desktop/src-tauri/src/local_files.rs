//! Access to local files, limited to files the user picked in a native dialog.
//!
//! The webview never names a local path. Every command that reads or writes a
//! local file opens the dialog itself, from Rust, so the only paths that reach
//! the filesystem are ones the user chose in an OS-drawn window the page cannot
//! script. A path handed over from the page would be indistinguishable from
//! one forged by script running in it.
//!
//! Where the choice and the transfer happen in separate steps (an upload waits
//! on an overwrite question in between), the page gets an opaque single-use
//! token for the picked file rather than the path.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager, Runtime, State};
use tauri_plugin_dialog::{DialogExt, FileDialogBuilder, FilePath};

type ApiResult<T> = std::result::Result<T, String>;

/// Private key files are a few KiB; anything past this is not one.
const MAX_KEY_FILE_BYTES: u64 = 256 * 1024;

/// How long a picked file stays redeemable. Long enough for the user to answer
/// an overwrite question, short enough that a forgotten grant does not linger.
const GRANT_LIFETIME: Duration = Duration::from_secs(10 * 60);

/// Upper bound on outstanding grants, so repeated picks cannot grow the map
/// without limit. The oldest grant is dropped first.
const MAX_GRANTS: usize = 32;

/// Files the user picked for a later step, keyed by an unguessable token.
#[derive(Default)]
pub struct LocalFileGrants {
    grants: Mutex<HashMap<String, (PathBuf, Instant)>>,
}

impl LocalFileGrants {
    /// Record a picked file and return the token that redeems it.
    pub fn issue(&self, path: PathBuf) -> String {
        self.issue_at(path, Instant::now())
    }

    /// Hand back the file behind `token`, once. An unknown, already redeemed or
    /// expired token yields nothing.
    pub fn redeem(&self, token: &str) -> Option<PathBuf> {
        self.redeem_at(token, Instant::now())
    }

    fn issue_at(&self, path: PathBuf, now: Instant) -> String {
        let mut grants = self.lock();
        grants.retain(|_, (_, issued)| now.duration_since(*issued) < GRANT_LIFETIME);
        while grants.len() >= MAX_GRANTS {
            let oldest = grants
                .iter()
                .min_by_key(|(_, (_, issued))| *issued)
                .map(|(token, _)| token.clone());
            match oldest {
                Some(token) => grants.remove(&token),
                None => break,
            };
        }
        let token = uuid::Uuid::new_v4().to_string();
        grants.insert(token.clone(), (path, now));
        token
    }

    fn redeem_at(&self, token: &str, now: Instant) -> Option<PathBuf> {
        let (path, issued) = self.lock().remove(token)?;
        (now.duration_since(issued) < GRANT_LIFETIME).then_some(path)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, (PathBuf, Instant)>> {
        // A poisoned map still holds valid grants; a panic elsewhere is no
        // reason to refuse every later transfer.
        self.grants.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// A native file dialog owned by the main window, so it cannot open behind it.
pub fn file_dialog<R: Runtime>(app: &AppHandle<R>) -> FileDialogBuilder<R> {
    let mut builder = app.dialog().file();
    if let Some(window) = app.get_webview_window("main") {
        builder = builder.set_parent(&window);
    }
    builder
}

fn into_local_path(picked: Option<FilePath>) -> ApiResult<Option<PathBuf>> {
    picked
        .map(|file| file.into_path().map_err(|e| e.to_string()))
        .transpose()
}

/// Ask for an existing file. `None` means the user cancelled.
///
/// The dialog runs on the main thread and answers through a callback, so the
/// command's runtime thread is not blocked while it is open.
pub async fn pick_file<R: Runtime>(builder: FileDialogBuilder<R>) -> ApiResult<Option<PathBuf>> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    builder.pick_file(move |picked| {
        let _ = tx.send(picked);
    });
    into_local_path(rx.await.map_err(|_| "the file dialog could not be shown".to_string())?)
}

/// Ask where to save a file. `None` means the user cancelled. The OS dialog
/// asks for confirmation itself when the chosen file already exists.
pub async fn save_file<R: Runtime>(builder: FileDialogBuilder<R>) -> ApiResult<Option<PathBuf>> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    builder.save_file(move |picked| {
        let _ = tx.send(picked);
    });
    into_local_path(rx.await.map_err(|_| "the file dialog could not be shown".to_string())?)
}

fn pick_key_dialog<R: Runtime>(app: &AppHandle<R>) -> FileDialogBuilder<R> {
    file_dialog(app)
        .add_filter(
            "SSH Private Keys",
            &["pem", "key", "id_rsa", "id_ed25519", "id_ecdsa", "id_dsa"],
        )
        .add_filter("All Files", &["*"])
}

fn read_key_file(path: &std::path::Path) -> ApiResult<String> {
    let metadata = std::fs::metadata(path).map_err(|e| e.to_string())?;
    if !metadata.is_file() {
        return Err(format!("Not a file: {}", path.display()));
    }
    if metadata.len() > MAX_KEY_FILE_BYTES {
        return Err("File too large (max 256KB)".to_string());
    }
    std::fs::read_to_string(path).map_err(|e| e.to_string())
}

/// Let the user pick a private key file and return its contents, or `None`
/// when the dialog was cancelled.
#[tauri::command]
pub async fn pick_key_file(app: AppHandle) -> ApiResult<Option<String>> {
    match pick_file(pick_key_dialog(&app)).await? {
        Some(path) => read_key_file(&path).map(Some),
        None => Ok(None),
    }
}

/// A file picked for upload: the token that redeems it, and its name for the
/// remote side.
#[derive(serde::Serialize)]
pub struct PickedUpload {
    pub token: String,
    pub name: String,
}

/// Let the user pick a local file to upload. The page receives a token and the
/// file name, never the path.
#[tauri::command]
pub async fn sftp_pick_upload_file(
    app: AppHandle,
    grants: State<'_, LocalFileGrants>,
) -> ApiResult<Option<PickedUpload>> {
    let Some(path) = pick_file(file_dialog(&app)).await? else {
        return Ok(None);
    };
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("unsupported file name: {}", path.display()))?
        .to_string();
    let token = grants.issue(path);
    Ok(Some(PickedUpload { token, name }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_grant_redeems_once() {
        let grants = LocalFileGrants::default();
        let token = grants.issue(PathBuf::from("picked.txt"));
        assert_eq!(grants.redeem(&token), Some(PathBuf::from("picked.txt")));
        assert_eq!(grants.redeem(&token), None);
    }

    #[test]
    fn an_unknown_token_redeems_nothing() {
        let grants = LocalFileGrants::default();
        grants.issue(PathBuf::from("picked.txt"));
        assert_eq!(grants.redeem("C:\\Users\\victim\\.ssh\\id_ed25519"), None);
        assert_eq!(grants.redeem(""), None);
    }

    #[test]
    fn an_expired_grant_redeems_nothing() {
        let grants = LocalFileGrants::default();
        let issued = Instant::now();
        let token = grants.issue_at(PathBuf::from("picked.txt"), issued);
        assert_eq!(grants.redeem_at(&token, issued + GRANT_LIFETIME), None);
    }

    #[test]
    fn outstanding_grants_are_capped_oldest_first() {
        let grants = LocalFileGrants::default();
        let start = Instant::now();
        let first = grants.issue_at(PathBuf::from("0"), start);
        let mut last = String::new();
        for i in 1..=MAX_GRANTS {
            last = grants.issue_at(PathBuf::from(i.to_string()), start + Duration::from_millis(i as u64));
        }
        assert_eq!(grants.lock().len(), MAX_GRANTS);
        assert_eq!(grants.redeem(&first), None);
        assert!(grants.redeem(&last).is_some());
    }

    #[test]
    fn key_files_over_the_limit_are_refused() {
        let dir = tempfile::tempdir().expect("temp dir");
        let big = dir.path().join("big");
        std::fs::write(&big, vec![b'a'; MAX_KEY_FILE_BYTES as usize + 1]).expect("write");
        assert!(read_key_file(&big).is_err());
        assert!(read_key_file(dir.path()).is_err());

        let key = dir.path().join("id_ed25519");
        std::fs::write(&key, "key text").expect("write");
        assert_eq!(read_key_file(&key).expect("read"), "key text");
    }
}
