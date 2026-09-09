use crate::{CoreError, Result};
use russh_keys::key::PublicKey;
use russh_keys::PublicKeyBase64;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// Trust-on-first-use known_hosts store. Mismatches are NEVER auto-accepted;
/// callers must surface them to the user and call `replace` explicitly.
pub struct KnownHosts {
    path: PathBuf,
    entries: HashMap<String, KnownHostEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct KnownHostEntry {
    key_type: String,
    key_base64: String,
    fingerprint: String,
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
        Ok(Self { path, entries })
    }

    pub fn save(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let data = serde_json::to_string_pretty(&self.entries)?;
        std::fs::write(&self.path, data)?;
        Ok(())
    }

    /// Returns Ok(true) if trusted (matches or first-seen + recorded).
    /// Returns Ok(false) if a different key is already recorded (mismatch).
    pub fn verify(&mut self, host: &str, port: u16, key: &PublicKey) -> Result<bool> {
        let k = key_path(host, port);
        let key_b64 = key.public_key_base64();
        let fingerprint = key.fingerprint();
        let key_type = key.name().to_string();
        match self.entries.get(&k) {
            None => {
                self.entries.insert(
                    k,
                    KnownHostEntry {
                        key_type,
                        key_base64: key_b64,
                        fingerprint,
                    },
                );
                self.save()?;
                Ok(true)
            }
            Some(existing) if existing.key_base64 == key_b64 => Ok(true),
            Some(_) => Ok(false),
        }
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
            },
        );
        self.save()
    }

    /// Remove a known host entry.
    pub fn remove(&mut self, host: &str, port: u16) -> Result<()> {
        let k = key_path(host, port);
        self.entries.remove(&k);
        self.save()
    }

    /// List all known host entries as (host:port, key_type, fingerprint).
    pub fn list(&self) -> Vec<(String, String, String)> {
        self.entries
            .iter()
            .map(|(k, e)| (k.clone(), e.key_type.clone(), e.fingerprint.clone()))
            .collect()
    }

    /// Returns the mismatch error for a host:port if the key doesn't match.
    pub fn check_mismatch(&self, host: &str, port: u16, key: &PublicKey) -> Result<()> {
        let k = key_path(host, port);
        let key_b64 = key.public_key_base64();
        match self.entries.get(&k) {
            None => Ok(()),
            Some(existing) if existing.key_base64 == key_b64 => Ok(()),
            Some(_) => Err(CoreError::HostKeyMismatch {
                host: k,
                reason: "recorded key differs from server-presented key".into(),
            }),
        }
    }
}

fn key_path(host: &str, port: u16) -> String {
    format!("{host}:{port}")
}

#[cfg(test)]
mod tests {
    use super::KnownHosts;

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
