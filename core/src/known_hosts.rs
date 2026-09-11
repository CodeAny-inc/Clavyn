use crate::{CoreError, Result};
use russh_keys::key::PublicKey;
use russh_keys::PublicKeyBase64;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// Trust-on-first-use known_hosts store. Mismatches are NEVER auto-accepted;
/// callers must surface them to the user and call `trust_presented_key`
/// explicitly.
pub struct KnownHosts {
    path: PathBuf,
    entries: HashMap<String, KnownHostEntry>,
    /// Server keys that conflict with a pinned entry, held until the user
    /// reviews them. Never written to disk: a key nobody has trusted must not
    /// outlive the process that saw it.
    presented: HashMap<String, PublicKey>,
}

/// A server key that differs from the one pinned for its host and is waiting
/// for the user to accept or ignore it.
#[derive(Debug, Clone)]
pub struct HostKeyChange {
    pub host: String,
    pub key_type: String,
    pub pinned_fingerprint: String,
    pub presented_fingerprint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct KnownHostEntry {
    key_type: String,
    key_base64: String,
    fingerprint: String,
    /// Tombstone set by `remove`. The entry is hidden from `list` but its key is
    /// kept, so a connection presenting a different key is still classified as a
    /// change instead of a first contact. Defaults to false so a stored file
    /// without the field loads as a live pin, and is left out again when false
    /// so ordinary pins keep the shape they have always had on disk.
    #[serde(default, skip_serializing_if = "is_false")]
    removed: bool,
}

fn is_false(value: &bool) -> bool {
    !*value
}

impl KnownHosts {
    pub fn load(path: PathBuf) -> Result<Self> {
        let entries = if path.exists() {
            let data = std::fs::read_to_string(&path)?;
            // Fail closed. Treating an unreadable file as "no known hosts" would
            // silently downgrade every pinned host back to trust-on-first-use.
            serde_json::from_str(&data).map_err(|e| CoreError::CorruptState {
                path: path.display().to_string(),
                reason: e.to_string(),
            })?
        } else {
            HashMap::new()
        };
        Ok(Self {
            path,
            entries,
            presented: HashMap::new(),
        })
    }

    pub fn save(&self) -> Result<()> {
        let data = serde_json::to_string_pretty(&self.entries)?;
        crate::fs_util::write_private(&self.path, &data)
    }

    /// Apply a change to the pins and persist it, restoring the previous state
    /// if the write fails.
    ///
    /// Every mutator goes through here, because this is the one store whose
    /// in-memory copy answers a trust question directly: `verify` decides
    /// whether to accept a server key from `entries`, not from the file. A
    /// mutation that was applied and then failed to save would be honoured for
    /// the rest of the process and gone after a restart, so the same host would
    /// be trusted now and back to first use later, with an error already
    /// reported for the write that did not happen.
    ///
    /// `presented` is rolled back with `entries` even though it is never
    /// written. It is keyed the same way and read by the same decisions, so
    /// letting the two halves disagree would reintroduce the split this exists
    /// to prevent.
    fn commit<T>(&mut self, apply: impl FnOnce(&mut Self) -> T) -> Result<T> {
        let entries = self.entries.clone();
        let presented = self.presented.clone();
        let applied = apply(self);
        match self.save() {
            Ok(()) => Ok(applied),
            Err(e) => {
                self.entries = entries;
                self.presented = presented;
                Err(e)
            }
        }
    }

    /// Returns Ok(true) if trusted (matches or first-seen + recorded).
    /// Returns Ok(false) if a different key is already recorded (mismatch).
    ///
    /// A tombstoned entry whose key is presented again is restored, so a host
    /// removed by mistake becomes visible in `list` after the next connection.
    pub fn verify(&mut self, host: &str, port: u16, key: &PublicKey) -> Result<bool> {
        // `check_mismatch` is the only place a presented key is compared with a
        // recorded one, so the two callers cannot drift apart.
        if self.check_mismatch(host, port, key).is_err() {
            return Ok(false);
        }
        let k = key_path(host, port);
        // A live pin that already matches needs no write. The other two cases —
        // restoring a tombstone and recording a first-use — both end with the
        // same live entry, and `check_mismatch` has already established that the
        // presented key is the one that belongs there.
        if matches!(self.entries.get(&k), Some(existing) if !existing.removed) {
            return Ok(true);
        }
        let entry = KnownHostEntry {
            key_type: key.name().to_string(),
            key_base64: key.public_key_base64(),
            fingerprint: key.fingerprint(),
            removed: false,
        };
        self.commit(|known_hosts| {
            known_hosts.entries.insert(k, entry);
        })?;
        Ok(true)
    }

    /// Explicitly replace a host key after user confirmation of a mismatch.
    pub fn replace(&mut self, host: &str, port: u16, key: &PublicKey) -> Result<()> {
        let k = key_path(host, port);
        let entry = KnownHostEntry {
            key_type: key.name().to_string(),
            key_base64: key.public_key_base64(),
            fingerprint: key.fingerprint(),
            removed: false,
        };
        self.commit(|known_hosts| {
            known_hosts.entries.insert(k, entry);
        })
    }

    /// Tombstone a known host entry: it stops being listed as trusted, but its
    /// key is retained so that a server presenting a different key is still
    /// reported as a change. Dropping the key outright would let the next
    /// connection re-pin anything as a first contact.
    pub fn remove(&mut self, host: &str, port: u16) -> Result<()> {
        let k = key_path(host, port);
        self.commit(|known_hosts| {
            if let Some(existing) = known_hosts.entries.get_mut(&k) {
                existing.removed = true;
            }
            known_hosts.presented.remove(&k);
        })
    }

    /// List trusted known host entries as (host:port, key_type, fingerprint).
    pub fn list(&self) -> Vec<(String, String, String)> {
        self.collect(false)
    }

    /// List tombstoned entries in the same shape as `list`.
    ///
    /// A retained key is still a record of a host that is no longer trusted, so
    /// it has to be visible somewhere and erasable from there; otherwise the
    /// only way to drop one is to hand-edit `known_hosts.json`.
    pub fn removed(&self) -> Vec<(String, String, String)> {
        self.collect(true)
    }

    /// The fingerprint retained for a tombstoned host, if there is one.
    ///
    /// A live pin and an unknown host both answer `None`: this names the record
    /// `forget` would erase, so a caller can print it before asking.
    pub fn retained_fingerprint(&self, host: &str, port: u16) -> Option<String> {
        self.entries
            .get(&key_path(host, port))
            .filter(|e| e.removed)
            .map(|e| e.fingerprint.clone())
    }

    fn collect(&self, removed: bool) -> Vec<(String, String, String)> {
        self.entries
            .iter()
            .filter(|(_, e)| e.removed == removed)
            .map(|(k, e)| (k.clone(), e.key_type.clone(), e.fingerprint.clone()))
            .collect()
    }

    /// The fingerprint `forget` would erase, or the error it would refuse with.
    ///
    /// Read-only, so a caller that has to ask the user first can find out
    /// whether there is anything to ask about without the question itself being
    /// able to erase a key. Deciding and erasing then happen against one view of
    /// the store instead of two.
    pub fn forgettable_fingerprint(&self, host: &str, port: u16) -> Result<String> {
        let k = key_path(host, port);
        match self.entries.get(&k) {
            None => Err(CoreError::InvalidInput(format!(
                "{k} is not a removed known host"
            ))),
            Some(existing) if !existing.removed => Err(CoreError::InvalidInput(format!(
                "{k} is still trusted; remove it before forgetting the key it was pinned to"
            ))),
            Some(existing) => Ok(existing.fingerprint.clone()),
        }
    }

    /// Erase a tombstoned entry and the key it retained.
    ///
    /// This is the deliberate end of the retention `remove` starts, and it puts
    /// the host back on trust-on-first-use: the next connection pins whatever
    /// answers. A live pin is refused, so forgetting a host is always two
    /// decisions rather than one.
    pub fn forget(&mut self, host: &str, port: u16) -> Result<()> {
        self.forgettable_fingerprint(host, port)?;
        let k = key_path(host, port);
        self.commit(|known_hosts| {
            known_hosts.entries.remove(&k);
            known_hosts.presented.remove(&k);
        })
    }

    /// Returns the mismatch error for a host:port if the key doesn't match,
    /// carrying both fingerprints so the caller can show what changed.
    pub fn check_mismatch(&self, host: &str, port: u16, key: &PublicKey) -> Result<()> {
        let k = key_path(host, port);
        let key_b64 = key.public_key_base64();
        match self.entries.get(&k) {
            None => Ok(()),
            Some(existing) if existing.key_base64 == key_b64 => Ok(()),
            Some(existing) => Err(CoreError::HostKeyMismatch {
                host: k,
                pinned: existing.fingerprint.clone(),
                presented: key.fingerprint(),
            }),
        }
    }

    /// Hold a server key that conflicts with the pinned entry so the user can
    /// compare fingerprints and accept it deliberately, instead of unpinning the
    /// host and trusting whatever key answers the next connection.
    pub fn hold_presented_key(&mut self, host: &str, port: u16, key: &PublicKey) {
        self.presented.insert(key_path(host, port), key.clone());
    }

    /// Key changes waiting for the user to review them.
    pub fn pending_changes(&self) -> Vec<HostKeyChange> {
        self.presented
            .keys()
            .filter_map(|k| self.change_for(k))
            .collect()
    }

    /// The pending change for one host, if the server presented a key that
    /// conflicts with the pin. Reads the fingerprints out of the held key rather
    /// than out of a caller's argument, so what is shown is what arrived.
    fn change_for(&self, k: &str) -> Option<HostKeyChange> {
        let key = self.presented.get(k)?;
        let entry = self.entries.get(k)?;
        Some(HostKeyChange {
            host: k.to_string(),
            key_type: key.name().to_string(),
            pinned_fingerprint: entry.fingerprint.clone(),
            presented_fingerprint: key.fingerprint(),
        })
    }

    /// The change `trust_presented_key` would pin, or the error it would refuse
    /// with.
    ///
    /// Read-only, the counterpart of `forgettable_fingerprint`: a caller that
    /// has to ask the user first can find out whether there is anything to ask
    /// about without the question itself being able to pin a key.
    pub fn confirmable_change(
        &self,
        host: &str,
        port: u16,
        fingerprint: &str,
    ) -> Result<HostKeyChange> {
        self.confirmable(&key_path(host, port), fingerprint)
            .map(|(change, _)| change)
    }

    /// The change and the key behind it, so the check and the pin read the same
    /// held key rather than looking it up twice.
    fn confirmable(&self, k: &str, fingerprint: &str) -> Result<(HostKeyChange, PublicKey)> {
        let unheld = || {
            CoreError::InvalidInput(format!(
                "no unreviewed host key for {k}; reconnect to see the key the server presents"
            ))
        };
        let key = self.presented.get(k).cloned().ok_or_else(unheld)?;
        // A key is only a change against a recorded one. With no entry to
        // compare it with there is no pinned fingerprint to put in front of the
        // user, so there is nothing to confirm and nothing to replace either.
        let change = self.change_for(k).ok_or_else(unheld)?;
        if change.presented_fingerprint != fingerprint {
            return Err(CoreError::InvalidInput(format!(
                "the host key for {k} is not the one that was reviewed; reconnect and compare the fingerprints again"
            )));
        }
        Ok((change, key))
    }

    /// Pin a held key in place of the recorded one. The caller passes the
    /// fingerprint it showed the user, so a view rendered before another key
    /// arrived cannot trust a key nobody looked at.
    pub fn trust_presented_key(&mut self, host: &str, port: u16, fingerprint: &str) -> Result<()> {
        let k = key_path(host, port);
        let (_, key) = self.confirmable(&k, fingerprint)?;
        self.replace(host, port, &key)?;
        self.presented.remove(&k);
        Ok(())
    }
}

fn key_path(host: &str, port: u16) -> String {
    format!("{host}:{port}")
}

#[cfg(test)]
mod tests {
    use super::KnownHosts;
    use crate::CoreError;
    use russh_keys::key::{KeyPair, PublicKey};
    use russh_keys::PublicKeyBase64;

    const HOST: &str = "prod.example.com";
    const PORT: u16 = 22;

    fn server_key() -> PublicKey {
        KeyPair::generate_ed25519()
            .clone_public_key()
            .expect("public key")
    }

    fn empty_store(dir: &std::path::Path) -> KnownHosts {
        KnownHosts::load(dir.join("known_hosts.json")).expect("load")
    }

    #[test]
    fn a_second_key_for_a_pinned_host_is_not_accepted() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut hosts = empty_store(dir.path());
        assert!(hosts.verify(HOST, PORT, &server_key()).expect("first use"));

        assert!(!hosts
            .verify(HOST, PORT, &server_key())
            .expect("second key"));
    }

    #[test]
    fn a_changed_key_reports_both_fingerprints() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut hosts = empty_store(dir.path());
        let pinned = server_key();
        let presented = server_key();
        hosts.verify(HOST, PORT, &pinned).expect("first use");

        let error = match hosts.check_mismatch(HOST, PORT, &presented) {
            Ok(()) => panic!("a different host key was accepted as a match"),
            Err(error) => error,
        };
        match &error {
            CoreError::HostKeyMismatch {
                host,
                pinned: recorded,
                presented: offered,
            } => {
                assert_eq!(host, "prod.example.com:22");
                assert_eq!(recorded, &pinned.fingerprint());
                assert_eq!(offered, &presented.fingerprint());
            }
            other => panic!("unexpected error: {other}"),
        }
        let message = error.to_string();
        assert!(message.contains(&pinned.fingerprint()), "{message}");
        assert!(message.contains(&presented.fingerprint()), "{message}");
    }

    #[test]
    fn a_removed_host_does_not_fall_back_to_first_use() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut hosts = empty_store(dir.path());
        hosts.verify(HOST, PORT, &server_key()).expect("first use");
        hosts.remove(HOST, PORT).expect("remove");
        assert!(hosts.list().is_empty());

        let attacker = server_key();
        assert!(hosts
            .check_mismatch(HOST, PORT, &attacker)
            .is_err_and(|e| matches!(e, CoreError::HostKeyMismatch { .. })));
        assert!(!hosts.verify(HOST, PORT, &attacker).expect("verify"));
    }

    #[test]
    fn a_removed_host_is_restored_by_the_key_it_was_pinned_to() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut hosts = empty_store(dir.path());
        let key = server_key();
        hosts.verify(HOST, PORT, &key).expect("first use");
        hosts.remove(HOST, PORT).expect("remove");

        assert!(hosts.verify(HOST, PORT, &key).expect("reconnect"));
        assert_eq!(hosts.list().len(), 1);
    }

    #[test]
    fn a_stored_file_without_the_tombstone_field_keeps_its_pin() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("known_hosts.json");
        let pinned = server_key();
        std::fs::write(
            &path,
            format!(
                r#"{{"prod.example.com:22":{{"key_type":"{}","key_base64":"{}","fingerprint":"{}"}}}}"#,
                pinned.name(),
                pinned.public_key_base64(),
                pinned.fingerprint()
            ),
        )
        .expect("write known hosts");

        let mut hosts = KnownHosts::load(path).expect("load");
        assert_eq!(hosts.list().len(), 1);
        assert!(hosts.verify(HOST, PORT, &pinned).expect("pinned key"));
        assert!(!hosts.verify(HOST, PORT, &server_key()).expect("other key"));
    }

    #[test]
    fn a_held_key_is_trusted_only_against_the_reviewed_fingerprint() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut hosts = empty_store(dir.path());
        let pinned = server_key();
        let presented = server_key();
        hosts.verify(HOST, PORT, &pinned).expect("first use");
        hosts.hold_presented_key(HOST, PORT, &presented);

        let changes = hosts.pending_changes();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].pinned_fingerprint, pinned.fingerprint());
        assert_eq!(changes[0].presented_fingerprint, presented.fingerprint());

        let error = hosts
            .trust_presented_key(HOST, PORT, &pinned.fingerprint())
            .expect_err("a fingerprint the user never reviewed was accepted");
        assert!(error.to_string().contains("not the one that was reviewed"));

        hosts
            .trust_presented_key(HOST, PORT, &presented.fingerprint())
            .expect("trust reviewed key");
        assert!(hosts.verify(HOST, PORT, &presented).expect("verify"));
        assert!(hosts.pending_changes().is_empty());
    }

    #[test]
    fn trusting_without_a_held_key_is_rejected() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut hosts = empty_store(dir.path());
        let pinned = server_key();
        hosts.verify(HOST, PORT, &pinned).expect("first use");

        let error = hosts
            .trust_presented_key(HOST, PORT, "SHA256:whatever")
            .expect_err("a host key was replaced without the server presenting one");
        assert!(error.to_string().contains("no unreviewed host key"));
    }

    #[test]
    fn a_removed_host_is_listed_so_its_retained_key_can_be_erased() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut hosts = empty_store(dir.path());
        let key = server_key();
        hosts.verify(HOST, PORT, &key).expect("first use");
        hosts.remove(HOST, PORT).expect("remove");

        let removed = hosts.removed();
        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].0, "prod.example.com:22");
        assert_eq!(removed[0].2, key.fingerprint());
        assert!(hosts.list().is_empty());
    }

    /// Make the next save fail by putting a directory where the file belongs.
    /// The staged write lands beside it, and the rename onto a directory is
    /// refused, so this exercises a genuine persistence failure rather than a
    /// simulated one.
    fn block_writes(path: &std::path::Path) {
        std::fs::remove_file(path).expect("remove file");
        std::fs::create_dir(path).expect("create blocking directory");
    }

    #[test]
    fn a_forget_that_cannot_be_saved_keeps_the_retained_key() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("known_hosts.json");
        let mut hosts = KnownHosts::load(path.clone()).expect("load");
        hosts.verify(HOST, PORT, &server_key()).expect("first use");
        hosts.remove(HOST, PORT).expect("remove");
        let retained = hosts
            .forgettable_fingerprint(HOST, PORT)
            .expect("retained fingerprint");

        block_writes(&path);
        hosts
            .forget(HOST, PORT)
            .expect_err("forget must report the failed write");

        // The call failed, so the protection it would have dropped is still in
        // place. Without the rollback the tombstone would be gone from memory
        // while the file still described it, and this process would treat the
        // next key offered for that host as a first use.
        assert_eq!(
            hosts
                .forgettable_fingerprint(HOST, PORT)
                .expect("key still retained"),
            retained
        );
        assert_eq!(hosts.removed().len(), 1);
    }

    #[test]
    fn a_replace_that_cannot_be_saved_keeps_the_previous_pin() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("known_hosts.json");
        let mut hosts = KnownHosts::load(path.clone()).expect("load");
        let pinned = server_key();
        let presented = server_key();
        hosts.verify(HOST, PORT, &pinned).expect("first use");
        hosts.hold_presented_key(HOST, PORT, &presented);
        let shown = presented.fingerprint();

        block_writes(&path);
        hosts
            .trust_presented_key(HOST, PORT, &shown)
            .expect_err("trust must report the failed write");

        // Runtime trust must not move ahead of the file. Without the rollback
        // the presented key would be pinned in memory for the rest of the
        // process and absent after a restart, so the same host would be trusted
        // now and challenged later.
        assert!(hosts.verify(HOST, PORT, &pinned).expect("original pin"));
        assert!(hosts.check_mismatch(HOST, PORT, &presented).is_err());
    }

    #[test]
    fn forgetting_a_removed_host_erases_the_retained_key() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("known_hosts.json");
        let mut hosts = KnownHosts::load(path.clone()).expect("load");
        hosts.verify(HOST, PORT, &server_key()).expect("first use");
        hosts.remove(HOST, PORT).expect("remove");
        hosts.forget(HOST, PORT).expect("forget");

        assert!(hosts.removed().is_empty());
        assert!(hosts.list().is_empty());
        assert!(!std::fs::read_to_string(&path)
            .expect("read back")
            .contains("prod.example.com"));

        // The host is deliberately back on first use, which is the whole point
        // of forgetting it.
        let mut reloaded = KnownHosts::load(path).expect("reload");
        assert!(reloaded.verify(HOST, PORT, &server_key()).expect("verify"));
    }

    #[test]
    fn a_trusted_host_cannot_be_forgotten_in_one_call() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut hosts = empty_store(dir.path());
        let pinned = server_key();
        hosts.verify(HOST, PORT, &pinned).expect("first use");

        let error = hosts
            .forget(HOST, PORT)
            .expect_err("a live pin was erased without being removed first");
        assert!(error.to_string().contains("still trusted"), "{error}");
        assert_eq!(hosts.list().len(), 1);
        assert!(!hosts.verify(HOST, PORT, &server_key()).expect("other key"));
    }

    #[test]
    fn an_unknown_host_cannot_be_forgotten() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut hosts = empty_store(dir.path());
        let error = hosts
            .forget(HOST, PORT)
            .expect_err("forgetting a host that was never known reported success");
        assert!(error.to_string().contains("not a removed known host"), "{error}");
    }

    #[test]
    fn a_live_pin_is_written_without_the_tombstone_field() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("known_hosts.json");
        let mut hosts = KnownHosts::load(path.clone()).expect("load");
        hosts.verify(HOST, PORT, &server_key()).expect("first use");

        let live = std::fs::read_to_string(&path).expect("read back");
        assert!(!live.contains("removed"), "{live}");

        hosts.remove(HOST, PORT).expect("remove");
        let tombstoned = std::fs::read_to_string(&path).expect("read back");
        assert!(tombstoned.contains("\"removed\": true"), "{tombstoned}");
    }

    fn pinned_store(dir: &std::path::Path) -> std::path::PathBuf {
        let path = dir.join("known_hosts.json");
        std::fs::write(
            &path,
            r#"{"prod.example.com:22":{"key_type":"ssh-ed25519","key_base64":"AAAApinned","fingerprint":"SHA256:pinned"}}"#,
        )
        .expect("write known hosts");
        path
    }

    #[test]
    fn loads_pinned_entries() {
        let dir = tempfile::tempdir().expect("tempdir");
        let hosts = KnownHosts::load(pinned_store(dir.path())).expect("load");
        assert_eq!(hosts.list().len(), 1);
    }

    #[test]
    fn corrupt_file_is_rejected_instead_of_dropping_pinned_keys() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = pinned_store(dir.path());
        std::fs::write(&path, r#"{"prod.example.com:22":{"key_type":"ssh-ed2"#).expect("truncate");

        let error = match KnownHosts::load(path) {
            Ok(_) => panic!("a corrupt known_hosts file unexpectedly loaded as empty"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("is corrupt"), "unexpected error: {error}");
    }

    #[test]
    fn missing_file_starts_empty() {
        let dir = tempfile::tempdir().expect("tempdir");
        let hosts = KnownHosts::load(dir.path().join("absent.json")).expect("load");
        assert!(hosts.list().is_empty());
    }
}
