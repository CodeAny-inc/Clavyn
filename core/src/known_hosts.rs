use crate::{CoreError, Result};
use crate::keys::fingerprint;
use russh::keys::{PublicKey, PublicKeyBase64};
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
            let stored = serde_json::from_str(&data).map_err(|e| CoreError::CorruptState {
                path: path.display().to_string(),
                reason: e.to_string(),
            })?;
            normalize_entries(stored)
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
        // Never write pins this same build could not load again: `load` fails
        // closed, so an unreadable file would stop the app from starting.
        serde_json::from_str::<HashMap<String, KnownHostEntry>>(&data).map_err(|e| {
            CoreError::UnwritableState {
                path: self.path.display().to_string(),
                reason: e.to_string(),
            }
        })?;
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
            key_type: key.algorithm().as_str().to_string(),
            key_base64: key.public_key_base64(),
            fingerprint: fingerprint(key),
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
            key_type: key.algorithm().as_str().to_string(),
            key_base64: key.public_key_base64(),
            fingerprint: fingerprint(key),
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
                presented: fingerprint(key),
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
            key_type: key.algorithm().as_str().to_string(),
            pinned_fingerprint: entry.fingerprint.clone(),
            presented_fingerprint: fingerprint(key),
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

/// The key a host's pin is stored under.
///
/// The host is normalized first, so every spelling of one machine shares one
/// pin. Without that, `PROD.example.com`, `prod.example.com.` and the several
/// textual forms of an IPv6 address would each be a separate first contact,
/// and connecting through a new spelling would pin whatever key answered.
fn key_path(host: &str, port: u16) -> String {
    format!("{}:{port}", normalize_host(host))
}

/// Lowercase a host name and strip a trailing root dot, or canonicalize an IP
/// literal (with or without the brackets an IPv6 literal is written in).
fn normalize_host(host: &str) -> String {
    let host = host.trim();
    let host = host
        .strip_prefix('[')
        .and_then(|inner| inner.strip_suffix(']'))
        .unwrap_or(host);
    let host = host.trim_end_matches('.');
    match host.parse::<std::net::IpAddr>() {
        Ok(ip) => ip.to_string(),
        Err(_) => host.to_ascii_lowercase(),
    }
}

/// `key_type` of an entry built from pins that disagree.
const CONFLICTING_PINS: &str = "conflicting pins";

/// Re-key a stored file under normalized host names.
///
/// A file written before names were normalized can hold one machine under
/// several spellings. Variants that pin the same key merge into one entry, live
/// if any of them was. Variants that pin different keys cannot be merged
/// safely: either could be the one a first-use through the other spelling
/// wrongly accepted. They become one entry that pins no key at all, so every
/// key the server presents is reported as a change carrying all the recorded
/// fingerprints, and the user picks deliberately. Neither variant is dropped
/// silently and the host never falls back to first use.
///
/// Only memory changes here; the file is rewritten by the next save.
fn normalize_entries(stored: HashMap<String, KnownHostEntry>) -> HashMap<String, KnownHostEntry> {
    let mut grouped: HashMap<String, Vec<KnownHostEntry>> = HashMap::new();
    for (stored_key, entry) in stored {
        let key = stored_key
            .rsplit_once(':')
            .and_then(|(host, port)| port.parse::<u16>().ok().map(|port| key_path(host, port)))
            .unwrap_or(stored_key);
        grouped.entry(key).or_default().push(entry);
    }
    grouped
        .into_iter()
        .map(|(key, variants)| (key, merge_variants(variants)))
        .collect()
}

fn merge_variants(mut variants: Vec<KnownHostEntry>) -> KnownHostEntry {
    let removed = variants.iter().all(|v| v.removed);
    let agree = variants
        .windows(2)
        .all(|pair| pair[0].key_base64 == pair[1].key_base64);
    if agree {
        let mut entry = variants.swap_remove(0);
        entry.removed = removed;
        return entry;
    }
    let mut fingerprints: Vec<String> = variants.into_iter().map(|v| v.fingerprint).collect();
    fingerprints.sort();
    fingerprints.dedup();
    KnownHostEntry {
        key_type: CONFLICTING_PINS.to_string(),
        // Matches no presented key, so `check_mismatch` refuses every one.
        key_base64: String::new(),
        fingerprint: fingerprints.join(" or "),
        removed,
    }
}

#[cfg(test)]
mod tests {
    use super::KnownHosts;
    use crate::keys::fingerprint;
    use crate::CoreError;
    use russh::keys::{Algorithm, PrivateKey, PublicKey, PublicKeyBase64};

    const HOST: &str = "prod.example.com";
    const PORT: u16 = 22;

    fn server_key() -> PublicKey {
        let mut rng = getrandom::rand_core::UnwrapErr(getrandom::SysRng);
        PrivateKey::random(&mut rng, Algorithm::Ed25519)
            .expect("generate")
            .public_key()
            .clone()
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
                assert_eq!(recorded, &fingerprint(&pinned));
                assert_eq!(offered, &fingerprint(&presented));
            }
            other => panic!("unexpected error: {other}"),
        }
        let message = error.to_string();
        assert!(message.contains(&fingerprint(&pinned)), "{message}");
        assert!(message.contains(&fingerprint(&presented)), "{message}");
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
                pinned.algorithm().as_str(),
                pinned.public_key_base64(),
                fingerprint(&pinned)
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
        assert_eq!(changes[0].pinned_fingerprint, fingerprint(&pinned));
        assert_eq!(changes[0].presented_fingerprint, fingerprint(&presented));

        let error = hosts
            .trust_presented_key(HOST, PORT, &fingerprint(&pinned))
            .expect_err("a fingerprint the user never reviewed was accepted");
        assert!(error.to_string().contains("not the one that was reviewed"));

        hosts
            .trust_presented_key(HOST, PORT, &fingerprint(&presented))
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
        assert_eq!(removed[0].2, fingerprint(&key));
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
        let shown = fingerprint(&presented);

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

#[cfg(test)]
mod normalization_tests {
    use super::{normalize_host, KnownHosts, CONFLICTING_PINS};
    use crate::keys::fingerprint;
    use crate::CoreError;
    use russh::keys::{Algorithm, PrivateKey, PublicKey, PublicKeyBase64};

    fn server_key() -> PublicKey {
        let mut rng = getrandom::rand_core::UnwrapErr(getrandom::SysRng);
        PrivateKey::random(&mut rng, Algorithm::Ed25519)
            .expect("generate")
            .public_key()
            .clone()
    }

    fn pin_json(host_port: &str, key: &PublicKey, removed: bool) -> String {
        format!(
            r#""{host_port}":{{"key_type":"ssh-ed25519","key_base64":"{}","fingerprint":"{}","removed":{removed}}}"#,
            key.public_key_base64(),
            fingerprint(key)
        )
    }

    fn load_file(dir: &std::path::Path, pins: &[String]) -> KnownHosts {
        let path = dir.join("known_hosts.json");
        std::fs::write(&path, format!("{{{}}}", pins.join(","))).expect("write pins");
        KnownHosts::load(path).expect("load")
    }

    #[test]
    fn spellings_of_one_host_normalize_to_one_name() {
        assert_eq!(normalize_host("PROD.Example.COM"), "prod.example.com");
        assert_eq!(normalize_host("prod.example.com."), "prod.example.com");
        assert_eq!(normalize_host(" prod.example.com "), "prod.example.com");
        assert_eq!(normalize_host("2001:DB8:0:0:0:0:0:1"), "2001:db8::1");
        assert_eq!(normalize_host("[2001:db8::1]"), "2001:db8::1");
        assert_eq!(normalize_host("192.168.1.10"), "192.168.1.10");
    }

    #[test]
    fn a_variant_spelling_is_checked_against_the_existing_pin() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut hosts = KnownHosts::load(dir.path().join("known_hosts.json")).expect("load");
        let pinned = server_key();
        hosts.verify("prod.example.com", 22, &pinned).expect("first use");

        for spelling in ["PROD.example.com", "prod.example.com.", " Prod.Example.Com "] {
            assert!(hosts.check_mismatch(spelling, 22, &pinned).is_ok(), "{spelling}");
            assert!(
                !hosts.verify(spelling, 22, &server_key()).expect("verify"),
                "{spelling} gave a different key a first use"
            );
        }
        assert_eq!(hosts.list().len(), 1);
    }

    #[test]
    fn ipv6_spellings_share_one_pin() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut hosts = KnownHosts::load(dir.path().join("known_hosts.json")).expect("load");
        hosts.verify("2001:db8::1", 22, &server_key()).expect("first use");

        assert!(!hosts
            .verify("[2001:DB8:0:0:0:0:0:1]", 22, &server_key())
            .expect("verify"));
    }

    #[test]
    fn stored_variants_with_one_key_merge_into_a_live_pin() {
        let dir = tempfile::tempdir().expect("tempdir");
        let key = server_key();
        let mut hosts = load_file(
            dir.path(),
            &[
                pin_json("PROD.example.com:22", &key, true),
                pin_json("prod.example.com.:22", &key, false),
            ],
        );

        let list = hosts.list();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].0, "prod.example.com:22");
        assert!(hosts.verify("prod.example.com", 22, &key).expect("verify"));
    }

    /// Two spellings pinned to different keys: either could be the wrong one,
    /// so no key is accepted until the user reviews one, and the host is not
    /// treated as unknown either.
    #[test]
    fn stored_variants_with_different_keys_accept_no_key_until_reviewed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let first = server_key();
        let second = server_key();
        let mut hosts = load_file(
            dir.path(),
            &[
                pin_json("prod.example.com:22", &first, false),
                pin_json("PROD.example.com:22", &second, false),
            ],
        );

        let list = hosts.list();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].1, CONFLICTING_PINS);
        for key in [&first, &second, &server_key()] {
            match hosts.check_mismatch("prod.example.com", 22, key) {
                Err(CoreError::HostKeyMismatch { pinned, .. }) => {
                    assert!(pinned.contains(&fingerprint(&first)), "{pinned}");
                    assert!(pinned.contains(&fingerprint(&second)), "{pinned}");
                }
                other => panic!("expected a mismatch, got {other:?}"),
            }
            assert!(!hosts.verify("prod.example.com", 22, key).expect("verify"));
        }

        // The usual review settles it: hold the presented key, confirm it.
        hosts.hold_presented_key("prod.example.com", 22, &first);
        hosts
            .trust_presented_key("prod.example.com", 22, &fingerprint(&first))
            .expect("trust reviewed key");
        assert!(hosts.verify("PROD.example.com", 22, &first).expect("verify"));
        assert!(!hosts.verify("prod.example.com", 22, &second).expect("verify"));
    }
}
