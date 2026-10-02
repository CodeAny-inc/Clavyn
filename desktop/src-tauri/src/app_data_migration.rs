//! Keeps persisted state in the non-roaming app data directory.
//!
//! On Windows `app_data_dir` is under `%APPDATA%` (Roaming), which a domain
//! with roaming profiles or folder redirection copies to a file server at
//! logoff and back at logon, so the vault's salt and ciphertext would end up on
//! a server, in its backups and on every machine the user signs in to. State
//! lives under `app_local_data_dir` (`%LOCALAPPDATA%`) instead, and files an
//! earlier build left in the roaming directory are moved across on startup. On
//! macOS and Linux both directories are the same one and nothing moves.
//!
//! The move is built so that no outcome loses a file:
//!
//! 1. Every file that still needs moving is copied, owner-only, and each copy
//!    is read back and compared. Nothing in the source is touched yet.
//! 2. Only once every copy is verified are the sources removed, and a marker is
//!    written to say the directory is settled.
//!
//! If a copy fails, the copies made in that run are removed and the source
//! directory stays in use for the run, exactly as before. The marker is what
//! tells a half-finished move from a finished one: until it exists, a
//! destination copy that differs from the source is a leftover of a run that
//! did not finish, and the source wins; once it exists, a file that reappears
//! in the source was written by something else — an older build, most likely —
//! and neither copy is overwritten. The source copy is still taken out of the
//! roaming profile, because leaving vault ciphertext there is the exposure this
//! module exists to end.
//!
//! Two reads of the destination are deliberately avoided. A source file that
//! is not there is settled without the destination being looked at, so a
//! finished move does no I/O that could fail; and a failed move only falls back
//! to the source directory while the destination is untouched. Falling back
//! after files have moved would start the app on an empty directory, which the
//! fail-closed loaders cannot tell from a fresh install — the vault would look
//! gone and every pinned host key with it.
//!
//! This runs before any state is loaded. In a release build the single-instance
//! guard has already refused a second instance, so nothing else is writing these
//! files; a debug build skips that guard, so a `tauri dev` build started beside
//! an installed one can still race it.

use clavyn_core::{remove_private, write_private_bytes};
use std::path::{Path, PathBuf};

/// Every file the app persists in its data directory.
const STATE_FILES: [&str; 4] = [
    "store.json",
    "vault.json",
    "known_hosts.json",
    "vault-biometric-binding",
];

/// Written in the destination once every state file is settled there.
const SETTLED_MARKER: &str = "state-settled";

/// The recovery screen renames a file it cannot read to `<name>.unreadable-<ts>`
/// beside itself. Those copies hold the same secrets as the file they came
/// from, so they move too rather than being left in a roaming profile.
const UNREADABLE_INFIX: &str = ".unreadable-";

/// Where one state file stands between the two directories.
#[derive(Debug, PartialEq, Eq)]
enum Plan {
    /// Only in the destination, or in neither: nothing to do.
    Settled,
    /// Only in the source, or in both with the destination copy left behind by
    /// a move that did not finish: copy, then remove the source.
    Copy,
    /// In both, identical: remove the source.
    RemoveSource,
    /// In both, different, after the destination was settled: leave the
    /// destination alone and take the source copy out of the roaming profile.
    Conflict,
}

fn plan(from: &Path, to: &Path, settled: bool) -> std::io::Result<Plan> {
    // Reading the destination only once the source is there keeps a settled
    // directory out of the I/O entirely: after the move the source is gone, so
    // this answers Settled without a read that could fail.
    let Some(source) = read_if_present(from)? else {
        return Ok(Plan::Settled);
    };
    Ok(match read_if_present(to)? {
        None => Plan::Copy,
        Some(destination) if destination == source => Plan::RemoveSource,
        // Before the marker exists the destination copy is this move's own
        // half-finished work, and the source is the copy the app has been
        // running on.
        Some(_) if !settled => Plan::Copy,
        Some(_) => Plan::Conflict,
    })
}

fn read_if_present(path: &Path) -> std::io::Result<Option<Vec<u8>>> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// Copy `from` to `to` owner-only and confirm the copy matches byte for byte.
///
/// The bytes are copied as bytes. Requiring valid UTF-8 would fail the copy of
/// exactly the files a user most needs moved — a state file hand-edited in an
/// ANSI editor, or one a flipped bit got to — and because the failure repeats
/// on every start it would block the whole move for good. What is and is not
/// loadable is for the loaders to decide, from the new location.
fn copy_verified(from: &Path, to: &Path) -> Result<(), String> {
    let contents =
        std::fs::read(from).map_err(|e| format!("could not read {}: {e}", from.display()))?;
    write_private_bytes(to, &contents).map_err(|e| e.to_string())?;
    let written =
        std::fs::read(to).map_err(|e| format!("could not read back {}: {e}", to.display()))?;
    if written != contents {
        return Err(format!("{} does not match {}", to.display(), from.display()));
    }
    Ok(())
}

/// Move state from `from` to `to` and return the directory to load it from.
pub fn settle_state_dir(from: &Path, to: &Path) -> PathBuf {
    match same_directory(from, to) {
        Ok(true) => return to.to_path_buf(),
        Ok(false) => {}
        Err(error) => {
            // The move ends by deleting the source files. Deleting them through
            // a junction or a symlink that resolves to the destination would
            // take the only copy of the vault, the pinned host keys and the
            // store with it, with nothing left to recover from. Unless the two
            // directories are provably distinct, nothing moves.
            tracing::warn!("state stays in {}: {error}", from.display());
            return from.to_path_buf();
        }
    }
    match migrate(from, to) {
        Ok(()) => to.to_path_buf(),
        Err(error) => {
            if move_has_started(to) {
                // Files are already in the destination. Loading from the source
                // would come up as a fresh install and write this session's
                // changes back into the roaming profile, so the destination
                // stays in use and the loaders report what went wrong.
                tracing::error!("state stays in {}: {error}", to.display());
                to.to_path_buf()
            } else {
                tracing::warn!("state stays in {} for this run: {error}", from.display());
                from.to_path_buf()
            }
        }
    }
}

/// Whether anything of the app's state is in `dir` already.
fn move_has_started(dir: &Path) -> bool {
    if dir.join(SETTLED_MARKER).exists() {
        return true;
    }
    STATE_FILES.iter().any(|name| dir.join(name).exists())
}

/// Whether `from` and `to` name the same directory.
///
/// `Err` means the question could not be answered, which callers must treat as
/// "possibly the same". Comparing the path text alone is not enough: a junction
/// or symlink between the two, or User Shell Folders entries pointing both
/// known folders at one directory, give one directory two spellings, and every
/// file then reads identical and is classified as an already-moved source.
fn same_directory(from: &Path, to: &Path) -> std::io::Result<bool> {
    if from == to {
        return Ok(true);
    }
    // A source that is not there has nothing to move and nothing to delete.
    if !from.exists() {
        return Ok(false);
    }
    // A reparse point or symlink can be removed as itself while its target
    // keeps the files, or resolve somewhere this module never inspected.
    if std::fs::symlink_metadata(from)?.file_type().is_symlink() {
        return Err(std::io::Error::other(format!(
            "{} is a link, not a directory",
            from.display()
        )));
    }
    if !to.exists() {
        return Ok(false);
    }
    // Resolves links and spelling differences on both platforms; on Windows it
    // also resolves junctions and returns a verbatim path, so two spellings of
    // one directory compare equal here.
    if std::fs::canonicalize(from)? == std::fs::canonicalize(to)? {
        return Ok(true);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        // Catches what canonicalization cannot: two paths that are the same
        // directory through a bind mount.
        let (a, b) = (std::fs::metadata(from)?, std::fs::metadata(to)?);
        if (a.dev(), a.ino()) == (b.dev(), b.ino()) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Every file to move out of `from`: the state files, plus the copies the
/// recovery screen set aside beside them.
fn files_to_move(from: &Path) -> Vec<String> {
    let mut names: Vec<String> = STATE_FILES.iter().map(|name| name.to_string()).collect();
    let Ok(entries) = std::fs::read_dir(from) else {
        return names;
    };
    for entry in entries.filter_map(|entry| entry.ok()) {
        let Some(name) = entry.file_name().to_str().map(|name| name.to_string()) else {
            continue;
        };
        let is_set_aside_state_file = STATE_FILES
            .iter()
            .any(|state| name.starts_with(&format!("{state}{UNREADABLE_INFIX}")));
        if is_set_aside_state_file {
            names.push(name);
        }
    }
    names
}

/// `Err` means the move did not finish. Every source file is untouched when the
/// destination has not been written to yet; once it has, the destination is the
/// directory to use.
fn migrate(from: &Path, to: &Path) -> Result<(), String> {
    let settled = to.join(SETTLED_MARKER).exists();
    let mut plans = Vec::new();
    for name in files_to_move(from) {
        let (source, destination) = (from.join(&name), to.join(&name));
        let plan = plan(&source, &destination, settled)
            .map_err(|e| format!("could not compare {name}: {e}"))?;
        plans.push((name, source, destination, plan));
    }

    let mut copied: Vec<PathBuf> = Vec::new();
    for (_, source, destination, plan) in &plans {
        if *plan != Plan::Copy {
            continue;
        }
        if let Err(error) = copy_verified(source, destination) {
            // Nothing in the source has been touched. The copies made in
            // this run are removed so a later start does not mistake them
            // for a finished move.
            for made in &copied {
                let _ = remove_private(made);
            }
            let _ = remove_private(destination);
            return Err(error);
        }
        copied.push(destination.clone());
    }

    for (name, source, destination, plan) in &plans {
        match plan {
            Plan::Copy | Plan::RemoveSource => {
                // Every copy is verified, so a source that fails to go away is
                // only a leftover: the next start sees it identical and removes
                // it.
                if let Err(error) = remove_private(source) {
                    tracing::warn!("{name} was moved but its old copy remains: {error}");
                }
            }
            Plan::Conflict => set_aside_conflict(name, source, destination, to),
            Plan::Settled => {}
        }
    }

    // The marker goes in last, so it only ever means every file is across.
    if let Err(error) = write_private_bytes(&to.join(SETTLED_MARKER), b"") {
        tracing::warn!("state moved to {} but was not marked settled: {error}", to.display());
    }
    let _ = std::fs::remove_dir(from);
    Ok(())
}

/// Take a conflicting source copy out of the roaming profile without
/// overwriting the destination copy.
///
/// Leaving it where it is keeps the vault's ciphertext syncing to a file
/// server for good, which is the exposure the move exists to end; overwriting
/// the destination with it would discard whatever the app has been using. So it
/// is renamed into the destination directory, where it is out of the roaming
/// profile, and is still there to be compared or put back by hand.
fn set_aside_conflict(name: &str, source: &Path, destination: &Path, to: &Path) {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let kept = to.join(format!("{name}.roaming-copy-{seconds}"));
    let moved = read_if_present(source)
        .map_err(|e| e.to_string())
        .and_then(|bytes| match bytes {
            None => Ok(()),
            Some(bytes) => write_private_bytes(&kept, &bytes).map_err(|e| e.to_string()),
        })
        .and_then(|()| remove_private(source).map(|_| ()).map_err(|e| e.to_string()));
    match moved {
        Ok(()) => tracing::error!(
            "{name} differs between {} and {}; the second is in use and the first was kept as {}",
            source.display(),
            destination.display(),
            kept.display()
        ),
        Err(error) => tracing::error!(
            "{name} differs between {} and {}; the second is in use and the first could not be taken out of the roaming profile: {error}",
            source.display(),
            destination.display()
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Dirs {
        _root: tempfile::TempDir,
        roaming: PathBuf,
        local: PathBuf,
    }

    fn dirs() -> Dirs {
        let root = tempfile::tempdir().expect("tempdir");
        let roaming = root.path().join("Roaming").join("com.clavyn.app");
        let local = root.path().join("Local").join("com.clavyn.app");
        std::fs::create_dir_all(&roaming).expect("roaming");
        Dirs { _root: root, roaming, local }
    }

    fn put(dir: &Path, name: &str, contents: &str) {
        std::fs::create_dir_all(dir).expect("dir");
        std::fs::write(dir.join(name), contents).expect("write");
    }

    fn read(dir: &Path, name: &str) -> Option<String> {
        std::fs::read_to_string(dir.join(name)).ok()
    }

    fn names_in(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .expect("read dir")
            .map(|entry| entry.expect("entry").file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn the_same_directory_is_left_alone() {
        let d = dirs();
        put(&d.roaming, "vault.json", "vault");
        assert_eq!(settle_state_dir(&d.roaming, &d.roaming), d.roaming);
        assert_eq!(read(&d.roaming, "vault.json").as_deref(), Some("vault"));
    }

    #[test]
    fn a_fresh_install_uses_the_local_directory() {
        let d = dirs();
        std::fs::remove_dir(&d.roaming).expect("no roaming dir");
        assert_eq!(settle_state_dir(&d.roaming, &d.local), d.local);
    }

    #[test]
    fn every_state_file_moves_together() {
        let d = dirs();
        for name in STATE_FILES {
            put(&d.roaming, name, &format!("{name} contents"));
        }
        put(&d.roaming, "vault.json.tmp", "older ciphertext");

        assert_eq!(settle_state_dir(&d.roaming, &d.local), d.local);

        for name in STATE_FILES {
            assert_eq!(read(&d.local, name), Some(format!("{name} contents")), "{name}");
        }
        assert!(!d.roaming.exists(), "the roaming directory should be empty and gone");
        assert!(d.local.join(SETTLED_MARKER).exists());
    }

    /// The recovery screen leaves these beside the file it could not read, and
    /// they hold the same secrets, so they must not stay in a roaming profile.
    #[test]
    fn files_set_aside_by_the_recovery_screen_move_too() {
        let d = dirs();
        put(&d.roaming, "store.json", "store");
        put(&d.roaming, "vault.json.unreadable-1700000000", "unreadable vault");

        assert_eq!(settle_state_dir(&d.roaming, &d.local), d.local);

        assert_eq!(
            read(&d.local, "vault.json.unreadable-1700000000").as_deref(),
            Some("unreadable vault")
        );
        assert!(!d.roaming.exists(), "the roaming directory should be empty and gone");
    }

    /// A state file that is not valid UTF-8 has to move like any other, or the
    /// copy fails on every start and the whole move is blocked for good.
    #[test]
    fn a_state_file_that_is_not_utf8_still_moves() {
        let d = dirs();
        std::fs::write(d.roaming.join("store.json"), [0xff, 0xfe, 0x00]).expect("write");

        assert_eq!(settle_state_dir(&d.roaming, &d.local), d.local);

        assert_eq!(
            std::fs::read(d.local.join("store.json")).expect("read"),
            vec![0xff, 0xfe, 0x00]
        );
        assert!(!d.roaming.join("store.json").exists());
    }

    #[test]
    fn an_interrupted_move_is_finished() {
        let d = dirs();
        put(&d.roaming, "store.json", "store");
        put(&d.roaming, "vault.json", "vault");
        // An earlier run copied the vault and stopped before removing it.
        put(&d.local, "vault.json", "vault");

        assert_eq!(settle_state_dir(&d.roaming, &d.local), d.local);

        assert_eq!(read(&d.local, "store.json").as_deref(), Some("store"));
        assert_eq!(read(&d.local, "vault.json").as_deref(), Some("vault"));
        assert!(read(&d.roaming, "vault.json").is_none());
        assert!(read(&d.roaming, "store.json").is_none());
    }

    /// Without the marker a destination copy is this move's own unfinished
    /// work, and the source is what the app has been running on, so the source
    /// wins. Treating it as a conflict would leave the app on a stale copy.
    #[test]
    fn a_leftover_of_an_unfinished_move_is_overwritten_from_the_source() {
        let d = dirs();
        put(&d.local, "store.json", "copy from a run that then failed");
        put(&d.roaming, "store.json", "what the app has been using");

        assert_eq!(settle_state_dir(&d.roaming, &d.local), d.local);

        assert_eq!(
            read(&d.local, "store.json").as_deref(),
            Some("what the app has been using")
        );
        assert!(read(&d.roaming, "store.json").is_none());
    }

    /// Once the destination is settled, a file back in the source came from
    /// something else. Neither copy is overwritten, and the source copy still
    /// leaves the roaming profile.
    #[test]
    fn a_conflict_after_settling_keeps_both_and_stops_the_roaming_copy() {
        let d = dirs();
        put(&d.local, SETTLED_MARKER, "");
        put(&d.local, "vault.json", "moved vault");
        put(&d.roaming, "vault.json", "vault an older build wrote");
        put(&d.roaming, "store.json", "store");

        assert_eq!(settle_state_dir(&d.roaming, &d.local), d.local);

        assert_eq!(read(&d.local, "vault.json").as_deref(), Some("moved vault"));
        assert!(
            !d.roaming.join("vault.json").exists(),
            "the conflicting copy stayed in the roaming profile"
        );
        let kept: Vec<String> = names_in(&d.local)
            .into_iter()
            .filter(|name| name.starts_with("vault.json.roaming-copy-"))
            .collect();
        assert_eq!(kept.len(), 1, "{:?}", names_in(&d.local));
        assert_eq!(read(&d.local, &kept[0]).as_deref(), Some("vault an older build wrote"));
        // A file with no counterpart still moves, or the local directory
        // would come up without it.
        assert_eq!(read(&d.local, "store.json").as_deref(), Some("store"));
        assert!(read(&d.roaming, "store.json").is_none());
    }

    #[test]
    fn a_failed_copy_keeps_the_roaming_directory_in_use_and_intact() {
        let d = dirs();
        put(&d.roaming, "store.json", "store");
        // A directory where vault.json's name has to be, so comparing the two
        // sides fails and the run stops before anything is copied.
        std::fs::create_dir_all(d.roaming.join("vault.json")).expect("seed");

        assert_eq!(settle_state_dir(&d.roaming, &d.local), d.roaming);

        assert_eq!(read(&d.roaming, "store.json").as_deref(), Some("store"));
        assert!(d.roaming.join("vault.json").exists());
        assert!(read(&d.local, "store.json").is_none(), "a partial copy was left behind");
        assert!(!d.local.join(SETTLED_MARKER).exists());
    }

    /// After the move, a destination read that fails must not send the app back
    /// to an empty roaming directory: the fail-closed loaders cannot tell that
    /// from a fresh install, so the vault and every pinned host key would look
    /// gone and this session's changes would be written back to the profile
    /// that roams.
    #[test]
    fn a_failure_after_the_move_keeps_the_local_directory_in_use() {
        let d = dirs();
        put(&d.local, SETTLED_MARKER, "");
        put(&d.local, "store.json", "store");
        put(&d.local, "vault.json", "vault");
        // Back in the source and unreadable, so comparing it fails.
        std::fs::create_dir_all(d.roaming.join("known_hosts.json")).expect("seed");
        std::fs::create_dir_all(d.roaming.join("known_hosts.json/inner")).expect("seed");

        assert_eq!(settle_state_dir(&d.roaming, &d.local), d.local);
    }

    /// Two spellings of one directory must not be "migrated" into themselves:
    /// every file would read identical, each source would be removed through
    /// the link, and the vault, the host keys and the store would all be gone.
    #[test]
    #[cfg(unix)]
    fn a_link_to_the_destination_is_refused() {
        let d = dirs();
        std::fs::create_dir_all(&d.local).expect("local");
        put(&d.local, "vault.json", "the only copy");
        std::fs::remove_dir(&d.roaming).expect("clear roaming");
        std::os::unix::fs::symlink(&d.local, &d.roaming).expect("link");

        assert_eq!(settle_state_dir(&d.roaming, &d.local), d.roaming);

        assert_eq!(read(&d.local, "vault.json").as_deref(), Some("the only copy"));
    }
}
