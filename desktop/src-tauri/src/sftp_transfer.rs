use crate::local_files::{file_dialog, save_file, LocalFileGrants};
use crate::state::AppState;
use std::sync::Arc;
use tauri::{AppHandle, State};

type ApiResult<T> = std::result::Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// The last path segment of a remote path, offered as the local file name.
fn remote_file_name(remote_path: &str) -> &str {
    remote_path
        .rsplit('/')
        .find(|segment| !segment.is_empty())
        .unwrap_or("download")
}

/// Ask where to save a remote SFTP file, then stream it straight there. File
/// bytes never cross the frontend IPC boundary, and the local path is chosen
/// in the native save dialog rather than named by the page.
///
/// Returns `false` when the user cancelled the dialog. Choosing an existing
/// file is the overwrite decision: the OS save dialog asks for confirmation
/// before it returns such a path.
#[tauri::command]
pub async fn sftp_download_to_local(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    session_id: String,
    remote_path: String,
) -> ApiResult<bool> {
    let dialog = file_dialog(&app).set_file_name(remote_file_name(&remote_path));
    let Some(local_path) = save_file(dialog).await? else {
        return Ok(false);
    };
    state
        .sftp
        .download_to_local(&session_id, &remote_path, &local_path, true)
        .await
        .map_err(err)?;
    Ok(true)
}

/// Stream a file picked with `sftp_pick_upload_file` to the remote SFTP server.
/// Overwrite must be an explicit user choice; otherwise an existing remote file
/// is rejected by the core.
#[tauri::command]
pub async fn sftp_upload_from_local(
    state: State<'_, Arc<AppState>>,
    grants: State<'_, LocalFileGrants>,
    session_id: String,
    upload_token: String,
    remote_path: String,
    overwrite: bool,
) -> ApiResult<()> {
    let local_path = grants
        .redeem(&upload_token)
        .ok_or("the picked file is no longer available; choose it again")?;
    state
        .sftp
        .upload_from_local(&session_id, &local_path, &remote_path, overwrite)
        .await
        .map_err(err)
}

#[cfg(test)]
mod tests {
    use super::remote_file_name;

    #[test]
    fn the_saved_name_is_the_last_remote_segment() {
        assert_eq!(remote_file_name("/srv/app/logs/today.log"), "today.log");
        assert_eq!(remote_file_name("README.md"), "README.md");
        assert_eq!(remote_file_name("/srv/app/"), "app");
        assert_eq!(remote_file_name("/"), "download");
    }
}
