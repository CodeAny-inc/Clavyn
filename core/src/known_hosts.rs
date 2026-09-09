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
            serde_json::from_str(&data).unwrap_or_default()
        } else {
            HashMap::new()
        };
        Ok(Self { path, entries })
    }

    pub fn save(&self) -> Result<()> {
        let data = serde_json::to_string_pretty(&self.entries)?;
        crate::fs_util::write_private(&self.path, &data)
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
