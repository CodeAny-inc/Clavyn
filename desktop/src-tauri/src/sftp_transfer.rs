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
///
/// The name is also what the user reads before clicking Save, so it has to
/// read as what will be written. Bidirectional controls and the line and
/// paragraph separators are replaced, so `report\u{202E}fdp.exe` cannot display
/// as `reportexe.pdf`. Padding is collapsed (see `collapse_padding`), so blank
/// characters cannot push the real extension out of the name box.
fn remote_file_name(remote_path: &str) -> String {
    suggested_name(remote_path, cfg!(windows))
}

/// `remote_file_name` with its Windows-only rules switched on or off by
/// `windows`, so both variants run under test on every platform.
///
/// Windows strips trailing dots and spaces, and opens device names such as
/// `CON` or `aux.txt` as the device instead of creating a file. There, the
/// trailing dots and spaces are trimmed and a device name gets a leading
/// underscore. Elsewhere those are ordinary names and are offered as served.
fn suggested_name(remote_path: &str, windows: bool) -> String {
    let last = remote_path
        .rsplit(['/', '\\'])
        .find(|segment| !segment.is_empty())
        .unwrap_or_default();
    let cleaned: Vec<char> = last
        .chars()
        .map(|c| match c {
            ':' | '*' | '?' | '"' | '<' | '>' | '|' | '/' | '\\' => '_',
            c if c.is_control() => '_',
            '\u{061C}' | '\u{200E}' | '\u{200F}' | '\u{2028}'..='\u{202E}'
            | '\u{2066}'..='\u{2069}' => '_',
            c => c,
        })
        .collect();
    let mut name = collapse_padding(&cleaned);
    if windows {
        name.truncate(name.trim_end_matches(['.', ' ']).len());
    }
    match name.as_str() {
        "" | "." | ".." => "download".to_string(),
        _ if windows && is_windows_device_name(&name) => format!("_{name}"),
        _ => name,
    }
}

/// Characters that render as empty space: Unicode white space, the Hangul
/// fillers, which fonts commonly draw with a width, and the braille blank.
fn is_blank(c: char) -> bool {
    c.is_whitespace() || matches!(c, '\u{115F}' | '\u{1160}' | '\u{3164}' | '\u{FFA0}' | '\u{2800}')
}

/// Default-ignorable characters, which render as nothing at all. The
/// bidirectional controls among them never reach this check, because
/// `suggested_name` has already replaced them.
fn is_invisible(c: char) -> bool {
    matches!(
        c,
        '\u{00AD}'
            | '\u{034F}'
            | '\u{17B4}'
            | '\u{17B5}'
            | '\u{180B}'..='\u{180F}'
            | '\u{200B}'..='\u{200D}'
            | '\u{2060}'..='\u{2065}'
            | '\u{206A}'..='\u{206F}'
            | '\u{FE00}'..='\u{FE0F}'
            | '\u{FEFF}'
            | '\u{FFF0}'..='\u{FFF8}'
            | '\u{1BCA0}'..='\u{1BCA3}'
            | '\u{1D173}'..='\u{1D17A}'
            | '\u{E0000}'..='\u{E0FFF}'
    )
}

/// Collapse padding to a single space. Padding is a run of blank and
/// invisible characters holding more than one blank; everything from its first
/// blank to its last becomes one space, so interleaving zero-width characters
/// with spaces cannot rebuild it. A lone blank is kept as served, and so are
/// invisible characters outside the blanks, such as the joiners Persian and
/// emoji sequences need or a variation selector after an emoji.
fn collapse_padding(chars: &[char]) -> String {
    let is_gap = |c: char| is_blank(c) || is_invisible(c);
    let mut out = String::with_capacity(chars.len());
    for run in chars.chunk_by(|&a, &b| is_gap(a) == is_gap(b)) {
        let first = run.iter().position(|&c| is_blank(c));
        let last = run.iter().rposition(|&c| is_blank(c));
        match (first, last) {
            (Some(first), Some(last)) if first < last => {
                out.extend(&run[..first]);
                out.push(' ');
                out.extend(&run[last + 1..]);
            }
            _ => out.extend(run),
        }
    }
    out
}

/// Whether Windows treats `name` as a device rather than a file. The check
/// applies to the part before the first dot, without its trailing spaces, in
/// any letter case. The superscript digits count as digits for `COM` and
/// `LPT`, and `0` is included because the Windows naming rules list `COM0` and
/// `LPT0` among the reserved names.
fn is_windows_device_name(name: &str) -> bool {
    let stem = name.split_once('.').map_or(name, |(stem, _)| stem);
    let stem = stem.trim_end_matches(' ').to_ascii_uppercase();
    match stem.as_str() {
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$" => true,
        _ => {
            let Some(rest) = stem.strip_prefix("COM").or_else(|| stem.strip_prefix("LPT")) else {
                return false;
            };
            let mut digits = rest.chars();
            matches!(
                (digits.next(), digits.next()),
                (Some('0'..='9' | '\u{00B9}' | '\u{00B2}' | '\u{00B3}'), None)
            )
        }
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
    use super::{remote_file_name, suggested_name};

    /// The name `suggested_name` offers on Windows and on every other platform.
    fn on_both(remote_path: &str) -> [String; 2] {
        [suggested_name(remote_path, true), suggested_name(remote_path, false)]
    }

    fn same_on_both(name: &str) -> [String; 2] {
        [name.to_string(), name.to_string()]
    }

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

    #[test]
    fn a_name_cannot_reorder_itself() {
        assert_eq!(on_both("report\u{202E}fdp.exe"), same_on_both("report_fdp.exe"));
        assert_eq!(on_both("a\u{2066}b\u{2069}c\u{200F}.txt"), same_on_both("a_b_c_.txt"));
        assert_eq!(
            on_both("x\u{061C}\u{200E}\u{202A}\u{202B}\u{202C}\u{202D}\u{2067}\u{2068}y"),
            same_on_both("x________y")
        );
        assert_eq!(on_both("invoice.pdf\u{2028}\u{2029}.exe"), same_on_both("invoice.pdf__.exe"));
    }

    /// A name box shows only the start of a long name, so a run of blanks
    /// between a decoy extension and the real one hides the real one.
    #[test]
    fn padding_cannot_push_the_real_extension_out_of_view() {
        let spaces = format!("invoice.pdf{}.exe", " ".repeat(80));
        let fillers = format!("invoice.pdf{}.exe", "\u{3164}".repeat(40));
        assert_eq!(on_both(&spaces), same_on_both("invoice.pdf .exe"));
        assert_eq!(on_both(&fillers), same_on_both("invoice.pdf .exe"));
        assert_eq!(on_both("invoice.pdf\u{2800}\u{00A0}\u{3000}.exe"), same_on_both("invoice.pdf .exe"));
        assert_eq!(
            on_both("invoice.pdf \u{200B} \u{2060} \u{FEFF} \u{200D} .exe"),
            same_on_both("invoice.pdf .exe")
        );
        assert_eq!(on_both("two  spaces.txt"), same_on_both("two spaces.txt"));
    }

    #[test]
    fn characters_that_read_correctly_are_kept() {
        for name in [
            "meeting notes.txt",
            "\u{8CC7}\u{6599}\u{3000}2024.pdf",
            "\u{645}\u{6CC}\u{200C}\u{62E}\u{648}\u{627}\u{647}\u{645}.txt",
            "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}.png",
            "\u{2764}\u{FE0F} notes.txt",
            "soft\u{00AD}hyphen.md",
        ] {
            assert_eq!(on_both(name), same_on_both(name), "{name:?}");
        }
    }

    #[test]
    fn trailing_dots_and_spaces_are_trimmed_only_where_windows_strips_them() {
        assert_eq!(
            on_both("invoice.pdf.exe. . "),
            ["invoice.pdf.exe".to_string(), "invoice.pdf.exe. . ".to_string()]
        );
        assert_eq!(on_both("..."), ["download".to_string(), "...".to_string()]);
        assert_eq!(on_both(". ."), ["download".to_string(), ". .".to_string()]);
    }

    #[test]
    fn a_windows_device_name_is_saved_as_a_file_on_windows() {
        for device in [
            "CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$", "COM0", "COM1", "COM9", "LPT0",
            "LPT1", "LPT9", "COM\u{00B9}", "COM\u{00B2}", "LPT\u{00B3}",
        ] {
            for name in [device.to_string(), device.to_lowercase(), format!("{device}.txt")] {
                assert_eq!(on_both(&name), [format!("_{name}"), name.clone()], "{name:?}");
            }
        }
        assert_eq!(on_both("con.tar.gz"), ["_con.tar.gz".to_string(), "con.tar.gz".to_string()]);
        assert_eq!(on_both("CON .txt"), ["_CON .txt".to_string(), "CON .txt".to_string()]);
        assert_eq!(on_both("NUL. ."), ["_NUL".to_string(), "NUL. .".to_string()]);
    }

    #[test]
    fn a_name_that_only_starts_like_a_device_is_kept() {
        for name in [
            "console.log",
            "com10.txt",
            "nullable.rs",
            "LPT",
            "COMA",
            "aux_.c",
            "CON\u{3000}.txt",
            "NUL\u{00A0}",
        ] {
            assert_eq!(on_both(name), same_on_both(name), "{name:?}");
        }
    }
}
