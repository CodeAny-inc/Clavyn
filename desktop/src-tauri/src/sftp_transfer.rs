use crate::local_files::{file_dialog, save_file, LocalFileGrants};
use crate::state::AppState;
use std::sync::Arc;
use tauri::{AppHandle, State};

type ApiResult<T> = std::result::Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// A local file name to offer for a remote path, safe to put in a save dialog.
///
/// The remote side chooses this name, and a Linux SFTP server may legally serve
/// `C:\Users\Public\...\Startup\x.bat`, `..\..\x` or `\\attacker\share\x`.
/// Windows `IFileSaveDialog` resolves a drive-qualified, backslash-relative or
/// UNC name typed into its name box, so a user who just clicks Save would write
/// outside the folder the dialog is showing, and a UNC name would also start an
/// outbound SMB — and NTLM — attempt. So the name is taken after both
/// separators, the characters no Windows file name may hold are replaced, and
/// anything that does not name a file falls back to `download`.
fn remote_file_name(remote_path: &str) -> String {
    let last = remote_path
        .rsplit(['/', '\\'])
        .find(|segment| !segment.is_empty())
        .unwrap_or_default();
    let cleaned: String = last
        .chars()
        .map(|c| match c {
            ':' | '*' | '?' | '"' | '<' | '>' | '|' | '/' | '\\' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    match cleaned.as_str() {
        "" | "." | ".." => "download".to_string(),
        _ => cleaned,
    }
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

    /// The server picks this name, and these are all legal entry names on a
    /// Linux SFTP server. The save dialog must not be handed one that resolves
    /// anywhere but the folder it is showing.
    #[test]
    fn a_name_the_server_chose_cannot_steer_the_save_dialog() {
        assert_eq!(
            remote_file_name(r"C:\Users\Public\Start Menu\Programs\Startup\x.bat"),
            "x.bat"
        );
        assert_eq!(remote_file_name(r"..\..\x"), "x");
        assert_eq!(remote_file_name(r"\\attacker\share\x"), "x");
        assert_eq!(remote_file_name("C:x.bat"), "C_x.bat");
        assert_eq!(remote_file_name("a?b*c|d\"e<f>g.txt"), "a_b_c_d_e_f_g.txt");
        assert_eq!(remote_file_name("name\u{7}with\u{1b}controls"), "name_with_controls");
        assert_eq!(remote_file_name("/srv/.."), "download");
        assert_eq!(remote_file_name("/srv/."), "download");
        assert_eq!(remote_file_name(r"\"), "download");
    }
}
