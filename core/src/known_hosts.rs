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
    /// without the field loads as a live pin.
    #[serde(default)]
    removed: bool,
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

    /// Returns Ok(true) if trusted (matches or first-seen + recorded).
    /// Returns Ok(false) if a different key is already recorded (mismatch).
    ///
    /// A tombstoned entry whose key is presented again is restored, so a host
    /// removed by mistake becomes visible in `list` after the next connection.
    pub fn verify(&mut self, host: &str, port: u16, key: &PublicKey) -> Result<bool> {
        let k = key_path(host, port);
        let key_b64 = key.public_key_base64();
        // Some(true) marks a tombstone waiting to be restored, Some(false) a
        // live pin that already matches.
        let tombstoned = match self.entries.get(&k) {
            None => None,
            Some(existing) if existing.key_base64 == key_b64 => Some(existing.removed),
            Some(_) => return Ok(false),
        };
        match tombstoned {
            None => {
                self.entries.insert(
                    k,
                    KnownHostEntry {
                        key_type: key.name().to_string(),
                        key_base64: key_b64,
                        fingerprint: key.fingerprint(),
                        removed: false,
                    },
                );
                self.save()?;
            }
            Some(true) => {
                if let Some(existing) = self.entries.get_mut(&k) {
                    existing.removed = false;
                }
                self.save()?;
            }
            Some(false) => {}
        }
        Ok(true)
    }

    /// Explicitly replace a host key after user confirmation of a mismatch.
    pub fn replace(&mut self, host: &str, port: u16, key: &PublicKey) -> Result<()> {
        let k = key_path(host, port);
        self.entries.insert(
            k,
            KnownHostEntry {
                key_type: key.name().to_string(),
                key_base64: key.public_key_base64(),
                fingerprint: key.fingerprint(),
                removed: false,
            },
        );
        self.save()
    }

    /// Tombstone a known host entry: it stops being listed as trusted, but its
    /// key is retained so that a server presenting a different key is still
    /// reported as a change. Dropping the key outright would let the next
    /// connection re-pin anything as a first contact.
    pub fn remove(&mut self, host: &str, port: u16) -> Result<()> {
        let k = key_path(host, port);
        if let Some(existing) = self.entries.get_mut(&k) {
            existing.removed = true;
        }
        self.presented.remove(&k);
        self.save()
    }

    /// List trusted known host entries as (host:port, key_type, fingerprint).
    pub fn list(&self) -> Vec<(String, String, String)> {
        self.entries
            .iter()
            .filter(|(_, e)| !e.removed)
            .map(|(k, e)| (k.clone(), e.key_type.clone(), e.fingerprint.clone()))
            .collect()
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
            .iter()
            .filter_map(|(k, key)| {
                let entry = self.entries.get(k)?;
                Some(HostKeyChange {
                    host: k.clone(),
                    key_type: key.name().to_string(),
                    pinned_fingerprint: entry.fingerprint.clone(),
                    presented_fingerprint: key.fingerprint(),
                })
            })
            .collect()
    }

    /// Pin a held key in place of the recorded one. The caller passes the
    /// fingerprint it showed the user, so a view rendered before another key
    /// arrived cannot trust a key nobody looked at.
    pub fn trust_presented_key(&mut self, host: &str, port: u16, fingerprint: &str) -> Result<()> {
        let k = key_path(host, port);
        let key = self.presented.get(&k).cloned().ok_or_else(|| {
            CoreError::InvalidInput(format!(
                "no unreviewed host key for {k}; reconnect to see the key the server presents"
            ))
        })?;
        if key.fingerprint() != fingerprint {
            return Err(CoreError::InvalidInput(format!(
                "the host key for {k} is not the one that was reviewed; reconnect and compare the fingerprints again"
            )));
        }
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
