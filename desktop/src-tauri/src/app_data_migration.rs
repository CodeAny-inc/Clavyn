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
//! 2. Only once every copy is verified are the sources removed. A crash here
//!    leaves identical copies in both places, which the next start finishes.
//!
//! If a copy fails, the copies made in that run are removed and the roaming
//! directory stays in use for the run, exactly as before. If both directories
//! hold a file and the two differ, the move already happened and something
//! (an older build, most likely) wrote the roaming copy afterwards; which one
//! is current cannot be told, so neither copy of that file is touched, the
//! conflict is logged, and the other files still move.
//!
//! This runs after the single-instance guard and before any state is loaded,
//! so no other writer can race it.

use clavyn_core::fs_util::{remove_private, write_private};
use std::path::{Path, PathBuf};

/// Every file the app persists in its data directory.
const STATE_FILES: [&str; 4] = [
    "store.json",
    "vault.json",
    "known_hosts.json",
    "vault-biometric-binding",
];

/// Where one state file stands between the two directories.
#[derive(Debug, PartialEq, Eq)]
enum Plan {
    /// Only in the destination, or in neither: nothing to do.
    Settled,
    /// Only in the source: copy, then remove the source.
    Copy,
    /// In both, identical: an earlier run copied it; remove the source.
    RemoveSource,
    /// In both, different: leave both alone.
    Conflict,
}

fn plan(from: &Path, to: &Path) -> std::io::Result<Plan> {
    let source = read_if_present(from)?;
    let destination = read_if_present(to)?;
    Ok(match (source, destination) {
        (None, _) => Plan::Settled,
        (Some(_), None) => Plan::Copy,
        (Some(a), Some(b)) if a == b => Plan::RemoveSource,
        (Some(_), Some(_)) => Plan::Conflict,
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
fn copy_verified(from: &Path, to: &Path) -> Result<(), String> {
    let contents = std::fs::read_to_string(from)
        .map_err(|e| format!("could not read {}: {e}", from.display()))?;
    write_private(to, &contents).map_err(|e| e.to_string())?;
    let written = std::fs::read(to).map_err(|e| format!("could not read back {}: {e}", to.display()))?;
    if written != contents.as_bytes() {
        return Err(format!("{} does not match {}", to.display(), from.display()));
    }
    Ok(())
}

/// Move state from `from` to `to` and return the directory to load it from.
pub fn settle_state_dir(from: &Path, to: &Path) -> PathBuf {
    if from == to {
        return to.to_path_buf();
    }
    match migrate(from, to) {
        Ok(()) => to.to_path_buf(),
        Err(error) => {
            tracing::warn!(
                "state stays in {} for this run: {error}",
                from.display()
            );
            from.to_path_buf()
        }
    }
}

/// `Err` means the move did not happen and `from` must stay in use; every
/// source file is untouched in that case.
fn migrate(from: &Path, to: &Path) -> Result<(), String> {
    let mut plans = Vec::new();
    for name in STATE_FILES {
        let (source, destination) = (from.join(name), to.join(name));
        let plan = plan(&source, &destination).map_err(|e| format!("could not compare {name}: {e}"))?;
        plans.push((name, source, destination, plan));
    }

    for (name, source, destination, plan) in &plans {
        if *plan == Plan::Conflict {
            tracing::warn!(
                "{name} differs between {} and {}; using the second and leaving both in place",
                source.display(),
                destination.display()
            );
        }
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

    for (name, source, _, plan) in &plans {
        if matches!(plan, Plan::Copy | Plan::RemoveSource) {
            // Every copy is verified, so a source that fails to go away is only
            // a leftover: the next start sees it identical and removes it.
            if let Err(error) = remove_private(source) {
                tracing::warn!("{name} was moved but its old copy remains: {error}");
            }
        }
    }
    let _ = std::fs::remove_dir(from);
    Ok(())
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

    #[test]
    fn differing_copies_are_both_kept() {
        let d = dirs();
        put(&d.local, "vault.json", "moved vault");
        put(&d.roaming, "vault.json", "vault an older build wrote");
        put(&d.roaming, "store.json", "store");

        assert_eq!(settle_state_dir(&d.roaming, &d.local), d.local);

        assert_eq!(read(&d.local, "vault.json").as_deref(), Some("moved vault"));
        assert_eq!(read(&d.roaming, "vault.json").as_deref(), Some("vault an older build wrote"));
        // A file with no counterpart still moves, or the local directory
        // would come up without it.
        assert_eq!(read(&d.local, "store.json").as_deref(), Some("store"));
        assert!(read(&d.roaming, "store.json").is_none());
    }

    #[test]
    fn a_failed_copy_keeps_the_roaming_directory_in_use_and_intact() {
        let d = dirs();
        put(&d.roaming, "store.json", "store");
        // Not valid UTF-8, so the copy fails after store.json was copied.
        std::fs::write(d.roaming.join("vault.json"), [0xff, 0xfe, 0x00]).expect("write");

        assert_eq!(settle_state_dir(&d.roaming, &d.local), d.roaming);

        assert_eq!(read(&d.roaming, "store.json").as_deref(), Some("store"));
        assert!(d.roaming.join("vault.json").exists());
        assert!(read(&d.local, "store.json").is_none(), "a partial copy was left behind");
    }
}
