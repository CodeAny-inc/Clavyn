use crate::keys::KeyMeta;
use crate::{CoreError, Result};
use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use argon2::{Algorithm, Argon2, Params, Version};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use zeroize::Zeroize;

/// Encrypted-at-rest vault for private key material.
///
/// Layout on disk (JSON):
///   { "salt": "<b64>", "ciphertext": "<b64>", "keys_meta": [...] }
///
/// The master key is derived from a user passphrase via Argon2id. The OS
/// keychain stores the passphrase (set by the UI layer), never the derived key.
/// Plaintext private keys only ever exist in memory and are zeroized on drop.
#[derive(Serialize, Deserialize)]
pub struct VaultFile {
    salt: String,
    ciphertext: String,
    pub keys_meta: Vec<KeyMeta>,
}

pub struct Vault {
    path: PathBuf,
    file: VaultFile,
}

#[derive(Serialize, Deserialize)]
struct VaultPayload {
    keys: Vec<(String, String)>, // (key_id, openssh private)
}

impl Vault {
    pub fn open(path: PathBuf) -> Result<Self> {
        let file = if path.exists() {
            let data = std::fs::read_to_string(&path)?;
            serde_json::from_str(&data)?
        } else {
            VaultFile {
                salt: String::new(),
                ciphertext: String::new(),
                keys_meta: Vec::new(),
            }
        };
        Ok(Self { path, file })
    }

    pub fn is_initialized(&self) -> bool {
        !self.file.ciphertext.is_empty()
    }

    pub fn keys_meta(&self) -> &[KeyMeta] {
        &self.file.keys_meta
    }

    /// Return a non-secret identifier that is stable for the lifetime of this
    /// vault generation and changes whenever a brand-new vault is initialized.
    ///
    /// The random KDF salt already has exactly those properties, so using it as
    /// an external credential-binding identifier avoids adding another piece of
    /// persisted state. Callers must not treat this value as secret material.
    pub fn binding_id(&self) -> Option<&str> {
        self.is_initialized().then_some(self.file.salt.as_str())
    }

    pub fn initialize(&mut self, passphrase: &str) -> Result<()> {
        let mut salt = [0u8; 16];
        use rand::RngCore;
        rand::rngs::OsRng.fill_bytes(&mut salt);
        let payload = VaultPayload { keys: Vec::new() };
        let plaintext = serde_json::to_vec(&payload)?;
        let ciphertext = seal(passphrase, &salt, &plaintext)?;
        // Do not publish an initialized in-memory vault until persistence has
        // succeeded. Otherwise a failed save makes the command's existing-vault
        // guard reject every retry, even after the filesystem problem is fixed.
        let candidate = Self {
            path: self.path.clone(),
            file: VaultFile {
                salt: base64(salt),
                ciphertext: base64(ciphertext),
                keys_meta: Vec::new(),
            },
        };
        candidate.save()?;
        self.file = candidate.file;
        Ok(())
    }

    /// Verify that a passphrase can decrypt the vault payload without exposing
    /// any private-key material to the caller.
    pub fn verify_passphrase(&self, passphrase: &str) -> Result<()> {
        if !self.is_initialized() {
            return Err(CoreError::Vault("vault not initialized".into()));
        }

        let salt = unbase64(&self.file.salt)?;
        let mut plaintext = open(passphrase, &salt, &unbase64(&self.file.ciphertext)?)?;
        let parsed = serde_json::from_slice::<VaultPayload>(&plaintext)
            .map(|_| ())
            .map_err(CoreError::from);
        plaintext.zeroize();
        parsed
    }

    pub fn add_key(
        &mut self,
        passphrase: &str,
        meta: KeyMeta,
        private_openssh: &str,
    ) -> Result<()> {
        let salt = unbase64(&self.file.salt)?;
        let mut pt = open(passphrase, &salt, &unbase64(&self.file.ciphertext)?)?;
        let mut payload: VaultPayload = serde_json::from_slice(&pt)?;
        payload
            .keys
            .push((meta.id.to_string(), private_openssh.to_string()));
        pt.zeroize();
        let new_pt = serde_json::to_vec(&payload)?;
        let ct = seal(passphrase, &salt, &new_pt)?;
        self.file.ciphertext = base64(ct);
        self.file.keys_meta.push(meta);
        self.save()
    }

    pub fn remove_key(&mut self, passphrase: &str, key_id: &str) -> Result<()> {
        let salt = unbase64(&self.file.salt)?;
        let mut pt = open(passphrase, &salt, &unbase64(&self.file.ciphertext)?)?;
        let mut payload: VaultPayload = serde_json::from_slice(&pt)?;
        payload.keys.retain(|(id, _)| id != key_id);
        pt.zeroize();
        let new_pt = serde_json::to_vec(&payload)?;
        let ct = seal(passphrase, &salt, &new_pt)?;
        self.file.ciphertext = base64(ct);
        self.file.keys_meta.push(meta);
        self.save()
    }

    pub fn remove_key(&mut self, passphrase: &str, key_id: &str) -> Result<()> {
        let salt = unbase64(&self.file.salt)?;
        let mut pt = open(passphrase, &salt, &unbase64(&self.file.ciphertext)?)?;
        let mut payload: VaultPayload = serde_json::from_slice(&pt)?;
        payload.keys.retain(|(id, _)| id != key_id);
        pt.zeroize();
        let new_pt = serde_json::to_vec(&payload)?;
        let ct = seal(passphrase, &salt, &new_pt)?;
        self.file.ciphertext = base64(ct);
        self.file
            .keys_meta
            .retain(|m| m.id.to_string() != key_id);
        self.save()
    }

    /// Decrypt and return a single key's private material. Caller is responsible
    /// for the `PrivateKeyMaterial` (it zeroizes on drop).
    pub fn get_key(&self, passphrase: &str, key_id: &str) -> Result<Vec<u8>> {
        let salt = unbase64(&self.file.salt)?;
        let pt = open(passphrase, &salt, &unbase64(&self.file.ciphertext)?)?;
        let payload: VaultPayload = serde_json::from_slice(&pt)?;
        payload
            .keys
            .into_iter()
            .find(|(id, _)| id == key_id)
            .map(|(_, k)| k.into_bytes())
            .ok_or_else(|| CoreError::Vault(format!("key {key_id} not found")))
    }

    /// Permanently destroy the vault: delete the authoritative on-disk file and
    /// any crash-leftover atomic-write temporary copy, then reset in-memory state
    /// to uninitialized. Every encrypted private key and credential is
    /// irrecoverably lost.
    ///
    /// The on-disk files hold only AES-256-GCM ciphertext; plaintext never
    /// touches disk. The caller owns authorization (verifying the master
    /// passphrase) and biometric credential cleanup before invoking this.
    pub fn reset(&mut self) -> Result<()> {
        let removal = crate::fs_util::remove_private(&self.path)?;
        self.finish_reset(removal)
    }

    fn finish_reset(&mut self, removal: crate::fs_util::RemovePrivateOutcome) -> Result<()> {
        self.file = VaultFile {
            salt: String::new(),
            ciphertext: String::new(),
            keys_meta: Vec::new(),
        };

        if let Some(error) = removal.durability_error {
            return Err(error.into());
        }
        Ok(())
    }

    fn save(&self) -> Result<()> {
        let data = serde_json::to_string_pretty(&self.file)?;
        crate::fs_util::write_private(&self.path, &data)
    }
}

fn derive_key(passphrase: &str, salt: &[u8]) -> Result<[u8; 32]> {
    let params = Params::new(64 * 1024, 3, 4, Some(32))
        .map_err(|e| CoreError::Vault(format!("argon2 params: {e}")))?;
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut out = [0u8; 32];
    argon
        .hash_password_into(passphrase.as_bytes(), salt, &mut out)
        .map_err(|e| CoreError::Vault(format!("kdf: {e}")))?;
    Ok(out)
}

fn seal(passphrase: &str, salt: &[u8], plaintext: &[u8]) -> Result<Vec<u8>> {
    let key = derive_key(passphrase, salt)?;
    let cipher = Aes256Gcm::new_from_slice(&key)
        .map_err(|e| CoreError::Vault(format!("aes init: {e}")))?;
    let mut nonce_bytes = [0u8; 12];
    use rand::RngCore;
    rand::rngs::OsRng.fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ct = cipher
        .encrypt(nonce, plaintext)
        .map_err(|e| CoreError::Vault(format!("encrypt: {e}")))?;
    let mut out = Vec::with_capacity(12 + ct.len());
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&ct);
    Ok(out)
}

fn open(passphrase: &str, salt: &[u8], ciphertext: &[u8]) -> Result<Vec<u8>> {
    if ciphertext.len() < 12 {
        return Err(CoreError::Vault("ciphertext too short".into()));
    }
    let (nonce_bytes, ct) = ciphertext.split_at(12);
    let key = derive_key(passphrase, salt)?;
    let cipher = Aes256Gcm::new_from_slice(&key)
        .map_err(|e| CoreError::Vault(format!("aes init: {e}")))?;
    cipher
        .decrypt(Nonce::from_slice(nonce_bytes), ct)
        .map_err(|_| CoreError::Vault("decrypt failed (wrong passphrase or corrupted)".into()))
}

fn base64(b: impl AsRef<[u8]>) -> String {
    use base64::{engine::general_purpose::STANDARD, Engine};
    STANDARD.encode(b.as_ref())
}

fn unbase64(s: &str) -> Result<Vec<u8>> {
    use base64::{engine::general_purpose::STANDARD, Engine};
    STANDARD.decode(s).map_err(|e| CoreError::Vault(format!("b64: {e}")))
}

#[cfg(test)]
mod tests {
    use super::Vault;

    #[test]
    fn verify_passphrase_rejects_wrong_password() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("vault.json");
        let mut vault = Vault::open(path).expect("open vault");
        vault.initialize("correct horse battery staple").expect("initialize");

        assert!(vault.verify_passphrase("correct horse battery staple").is_ok());
        assert!(vault.verify_passphrase("wrong passphrase").is_err());
    }

    #[test]
    fn binding_id_is_stable_for_an_initialized_vault() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("vault.json");
        let mut vault = Vault::open(path.clone()).expect("open vault");
        assert!(vault.binding_id().is_none());

        vault.initialize("correct horse battery staple").expect("initialize");
        let binding_id = vault.binding_id().expect("binding id").to_string();
        assert!(!binding_id.is_empty());
        drop(vault);

        let reopened = Vault::open(path).expect("reopen vault");
        assert_eq!(reopened.binding_id(), Some(binding_id.as_str()));
    }

    #[test]
    fn reset_deletes_the_vault_file_and_clears_in_memory_state() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("vault.json");
        let mut vault = Vault::open(path.clone()).expect("open vault");
        vault.initialize("correct horse battery staple").expect("initialize");
        assert!(path.exists());
        assert!(vault.is_initialized());
        assert!(vault.binding_id().is_some());

        vault.reset().expect("reset vault");

        assert!(!path.exists());
        assert!(!vault.is_initialized());
        assert!(vault.binding_id().is_none());
        assert!(vault.keys_meta().is_empty());
    }

    #[test]
    fn reset_clears_in_memory_state_even_when_directory_sync_fails() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("vault.json");
        let mut vault = Vault::open(path).expect("open vault");
        vault.initialize("correct horse battery staple").expect("initialize");

        let result = vault.finish_reset(crate::fs_util::RemovePrivateOutcome {
            durability_error: Some(std::io::Error::new(
                std::io::ErrorKind::Other,
                "directory sync failed",
            )),
        });

        assert!(result.is_err());
        assert!(!vault.is_initialized());
        assert!(vault.binding_id().is_none());
        assert!(vault.keys_meta().is_empty());
    }

    #[test]
    fn reset_deletes_a_crash_leftover_atomic_temp_copy() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("vault.json");
        let tmp = dir.path().join("vault.json.tmp");
        let mut vault = Vault::open(path.clone()).expect("open vault");
        vault.initialize("correct horse battery staple").expect("initialize");
        let encrypted_snapshot = std::fs::read(&path).expect("read encrypted vault");
        std::fs::write(&tmp, encrypted_snapshot).expect("seed crash temp copy");
        assert!(path.exists());
        assert!(tmp.exists());

        vault.reset().expect("reset vault");

        assert!(!path.exists());
        assert!(!tmp.exists());
        assert!(!vault.is_initialized());
    }

    #[test]
    fn reset_allows_reinitializing_with_a_new_passphrase() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("vault.json");
        let mut vault = Vault::open(path.clone()).expect("open vault");
        vault.initialize("first passphrase").expect("initialize first");
        let first_binding = vault.binding_id().expect("first binding").to_string();

        vault.reset().expect("reset vault");

        vault.initialize("second passphrase").expect("initialize second");
        assert!(vault.is_initialized());
        assert!(vault.verify_passphrase("second passphrase").is_ok());
        assert!(vault.verify_passphrase("first passphrase").is_err());
        assert_ne!(vault.binding_id(), Some(first_binding.as_str()));
    }

    #[test]
    fn reset_on_an_uninitialized_vault_is_a_noop() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("vault.json");
        let mut vault = Vault::open(path.clone()).expect("open vault");
        assert!(!vault.is_initialized());

        vault.reset().expect("reset uninitialized vault");

        assert!(!path.exists());
        assert!(!vault.is_initialized());
    }

    #[test]
    fn failed_parent_creation_leaves_initialization_retryable() {
        let dir = tempfile::tempdir().expect("tempdir");
        let parent = dir.path().join("blocked-parent");
        std::fs::write(&parent, b"not a directory").expect("block parent");
        let path = parent.join("vault.json");
        let mut vault = Vault::open(path.clone()).expect("open vault");

        assert!(vault.initialize("first attempt passphrase").is_err());
        assert!(!vault.is_initialized());
        assert!(vault.binding_id().is_none());
        assert!(vault.keys_meta().is_empty());

        std::fs::remove_file(&parent).expect("repair parent");
        vault.initialize("retry passphrase").expect("retry same vault");
        assert!(vault.is_initialized());
        assert!(vault.verify_passphrase("retry passphrase").is_ok());
        assert!(vault.verify_passphrase("first attempt passphrase").is_err());
        let reopened = Vault::open(path).expect("reopen persisted vault");
        assert_eq!(reopened.binding_id(), vault.binding_id());
        assert!(reopened.verify_passphrase("retry passphrase").is_ok());
    }

    #[test]
    fn failed_file_write_leaves_initialization_retryable() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("vault.json");
        let mut vault = Vault::open(path.clone()).expect("open vault");
        // Parent creation succeeds, but writing to a directory fails reliably
        // even when tests run with permission to write anywhere in the tempdir.
        std::fs::create_dir(&path).expect("block vault file");

        assert!(vault.initialize("first attempt passphrase").is_err());
        assert!(!vault.is_initialized());
        assert!(vault.binding_id().is_none());
        assert!(vault.keys_meta().is_empty());

        std::fs::remove_dir(&path).expect("repair vault path");
        vault.initialize("retry passphrase").expect("retry same vault");
        let reopened = Vault::open(path).expect("reopen persisted vault");
        assert_eq!(reopened.binding_id(), vault.binding_id());
        assert!(reopened.verify_passphrase("retry passphrase").is_ok());
    }
}
