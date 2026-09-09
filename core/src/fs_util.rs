use crate::Result;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Write `contents` to `path` atomically, readable only by the owner.
///
/// The bytes go to a sibling temporary file that is created with mode 0600 on
/// Unix and flushed to disk, then renamed over the target. Two properties fall
/// out of that:
///
/// * A crash or power loss leaves either the previous file or the new one. A
///   plain `fs::write` truncates first, so an interrupted save can leave a
///   half-written file, and for `vault.json` that means losing every stored key.
/// * The finished file is never briefly world-readable, and a file left behind
///   at 0644 by an earlier version is replaced by a 0600 one on the next save.
///
/// Windows ignores the mode bits. Files under the user profile inherit its ACL
/// there, which is weaker than 0600 and needs a separate fix.
pub(crate) fn write_private(path: &Path, contents: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let tmp = temp_path(path);
    match write_temp(&tmp, contents).and_then(|()| std::fs::rename(&tmp, path)) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e.into())
        }
    }
}

/// Remove an atomically-written private file and any crash-leftover sibling
/// temporary copy. The temporary copy is deleted first so a failure there never
/// destroys the authoritative file while leaving an older encrypted snapshot
/// behind.
pub(crate) fn remove_private(path: &Path) -> Result<()> {
    remove_if_present(&temp_path(path))?;
    remove_if_present(path)?;
    Ok(())
}

fn remove_if_present(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn temp_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".tmp");
    path.with_file_name(name)
}

fn write_temp(tmp: &Path, contents: &str) -> std::io::Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }

    let mut file = options.open(tmp)?;
    file.write_all(contents.as_bytes())?;
    file.sync_all()
}

#[cfg(test)]
mod tests {
    use super::{remove_private, write_private};

    #[test]
    fn writes_and_then_replaces_the_target() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("state.json");

        write_private(&path, "{\"first\":true}").expect("first write");
        assert_eq!(std::fs::read_to_string(&path).expect("read"), "{\"first\":true}");

        write_private(&path, "{\"second\":true}").expect("second write");
        assert_eq!(std::fs::read_to_string(&path).expect("read"), "{\"second\":true}");
    }

    #[test]
    fn creates_the_parent_directory() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("nested").join("state.json");

        write_private(&path, "{}").expect("write");
        assert!(path.exists());
    }

    #[test]
    fn leaves_no_temporary_file_behind() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("state.json");

        write_private(&path, "{}").expect("write");

        let names: Vec<String> = std::fs::read_dir(dir.path())
            .expect("read dir")
            .map(|entry| entry.expect("entry").file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["state.json".to_string()]);
    }

    #[test]
    fn remove_private_deletes_authoritative_and_crash_temp_copies() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("vault.json");
        let tmp = dir.path().join("vault.json.tmp");
        std::fs::write(&path, "current ciphertext").expect("seed vault");
        std::fs::write(&tmp, "older ciphertext").expect("seed temp");

        remove_private(&path).expect("remove private file");

        assert!(!path.exists());
        assert!(!tmp.exists());
    }

    #[test]
    fn remove_private_is_idempotent_when_files_are_missing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("vault.json");

        remove_private(&path).expect("remove missing private file");
    }

    #[cfg(unix)]
    #[test]
    fn the_written_file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("state.json");

        // An existing world-readable file must not keep its mode.
        std::fs::write(&path, "{}").expect("seed");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).expect("chmod");

        write_private(&path, "{}").expect("write");

        let mode = std::fs::metadata(&path).expect("metadata").permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "unexpected mode {:o}", mode & 0o777);
    }
}
