use crate::keys::KeyMeta;
use crate::{CoreError, Result};
use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use argon2::{Algorithm, Argon2, Params, Version};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use zeroize::{Zeroize, Zeroizing};

/// Master key for a vault, derived from the user passphrase with Argon2id and
/// zeroized on drop. It is the only value that unlocks the payload; the
/// passphrase itself is not needed once this has been derived.
pub type VaultKey = Zeroizing<[u8; 32]>;

/// Encrypted-at-rest vault for private key material.
///
/// Layout on disk (JSON):
///   { "salt": "<b64>", "ciphertext": "<b64>", "keys_meta": [...] }
///
/// The master key is derived from a user passphrase via Argon2id. The OS
/// keychain stores the passphrase (set by the UI layer), never the derived key.
/// Plaintext private keys only ever exist in memory and are zeroized on drop.
#[derive(Serialize, Deserialize, Clone)]
pub struct VaultFile {
    salt: String,
    ciphertext: String,
    pub keys_meta: Vec<KeyMeta>,
}

pub struct Vault {
    path: PathBuf,
    file: VaultFile,
    /// Set only on a snapshot produced by [`Vault::snapshot`]. A snapshot
    /// carries the already-derived master key so key material can be read
    /// without running Argon2 again, and it refuses every write so a copy taken
    /// before a concurrent change can never overwrite the live vault file.
    session_key: Option<VaultKey>,
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
        Ok(Self {
            path,
            file,
            session_key: None,
        })
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

    /// Derive the master key from a passphrase.
    ///
    /// Argon2id at 64 MiB / t=3 costs roughly 100 ms in a release build, so the
    /// work runs on a blocking thread: doing it inline would park an async
    /// runtime worker — and with it every other command scheduled on that
    /// worker — for the whole derivation. This is the only entry point that
    /// runs the KDF, so no operation can reach it from a synchronous path.
    pub async fn derive_key(&self, passphrase: &str) -> Result<VaultKey> {
        let salt = unbase64(&self.file.salt)?;
        derive_key_off_thread(passphrase, salt).await
    }

    /// Create the vault from a fresh random salt and return the derived master
    /// key, so an unlock that follows initialization does not pay the KDF twice.
    pub async fn initialize(&mut self, passphrase: &str) -> Result<VaultKey> {
        self.ensure_writable()?;
        let mut salt = [0u8; 16];
        use rand::RngCore;
        rand::rngs::OsRng.fill_bytes(&mut salt);
        let key = derive_key_off_thread(passphrase, salt.to_vec()).await?;
        let payload = VaultPayload { keys: Vec::new() };
        let plaintext = Zeroizing::new(serde_json::to_vec(&payload)?);
        let ciphertext = seal(&key, &plaintext)?;
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
            session_key: None,
        };
        candidate.save()?;
        self.file = candidate.file;
        Ok(key)
    }

    /// Verify a passphrase and hand back the master key it derives, so a caller
    /// that goes on to use the vault does not run the KDF a second time.
    pub async fn verify_passphrase(&self, passphrase: &str) -> Result<VaultKey> {
        let key = self.derive_key(passphrase).await?;
        self.verify_key(&key)?;
        Ok(key)
    }

    /// Verify that a derived key can decrypt the vault payload without exposing
    /// any private-key material to the caller.
    pub fn verify_key(&self, key: &VaultKey) -> Result<()> {
        if !self.is_initialized() {
            return Err(CoreError::Vault("vault not initialized".into()));
        }

        let mut plaintext = open(key, &unbase64(&self.file.ciphertext)?)?;
        let parsed = serde_json::from_slice::<VaultPayload>(&plaintext)
            .map(|_| ())
            .map_err(CoreError::from);
        plaintext.zeroize();
        parsed
    }

    pub fn add_key(&mut self, key: &VaultKey, meta: KeyMeta, private_openssh: &str) -> Result<()> {
        let mut pt = open(key, &unbase64(&self.file.ciphertext)?)?;
        let mut payload: VaultPayload = serde_json::from_slice(&pt)?;
        payload
            .keys
            .push((meta.id.to_string(), private_openssh.to_string()));
        pt.zeroize();
        let new_pt = Zeroizing::new(serde_json::to_vec(&payload)?);
        let ct = seal(key, &new_pt)?;
        self.file.ciphertext = base64(ct);
        self.file.keys_meta.push(meta);
        self.save()
    }

    pub fn remove_key(&mut self, key: &VaultKey, key_id: &str) -> Result<()> {
        let mut pt = open(key, &unbase64(&self.file.ciphertext)?)?;
        let mut payload: VaultPayload = serde_json::from_slice(&pt)?;
        payload.keys.retain(|(id, _)| id != key_id);
        pt.zeroize();
        let new_pt = Zeroizing::new(serde_json::to_vec(&payload)?);
        let ct = seal(key, &new_pt)?;
        self.file.ciphertext = base64(ct);
        self.file
            .keys_meta
            .retain(|m| m.id.to_string() != key_id);
        self.save()
    }

    /// Decrypt and return a single key's private material as raw OpenSSH PEM
    /// bytes. The returned `Vec<u8>` does not wipe itself on drop: the caller
    /// owns zeroizing it, and anything it is copied into, once the key material
    /// is no longer needed.
    ///
    /// A snapshot taken from an unlocked vault already carries the master key
    /// and answers from a single AES-GCM decrypt; otherwise the key is derived
    /// from `passphrase`, which costs a full Argon2id pass.
    pub async fn get_key(&self, passphrase: &str, key_id: &str) -> Result<Vec<u8>> {
        let key = match &self.session_key {
            Some(key) => key.clone(),
            None => self.derive_key(passphrase).await?,
        };
        self.get_key_with(&key, key_id)
    }

    /// Decrypt a single key's private material with an already-derived key.
    /// The returned bytes carry the same caller obligation as [`Vault::get_key`].
    pub fn get_key_with(&self, key: &VaultKey, key_id: &str) -> Result<Vec<u8>> {
        let mut pt = open(key, &unbase64(&self.file.ciphertext)?)?;
        let payload: VaultPayload = serde_json::from_slice(&pt)?;
        pt.zeroize();
        payload
            .keys
            .into_iter()
            .find(|(id, _)| id == key_id)
            .map(|(_, k)| k.into_bytes())
            .ok_or_else(|| CoreError::Vault(format!("key {key_id} not found")))
    }

    /// Take a read-only copy of the encrypted vault carrying the master key.
    ///
    /// This lets a caller release the vault lock before a long network
    /// operation that still needs to read key material: one stalled peer then
    /// cannot keep every other vault operation — including locking the vault —
    /// waiting on the mutex.
    pub fn snapshot(&self, key: VaultKey) -> Self {
        Self {
            path: self.path.clone(),
            file: self.file.clone(),
            session_key: Some(key),
        }
    }

    /// Permanently destroy the vault: delete the authoritative on-disk file and
    /// any crash-leftover atomic-write temporary copy, then reset in-memory state
    /// to uninitialized. Every encrypted private key and credential is
    /// irrecoverably lost.
    ///
    /// The on-disk file stores a salt and key metadata alongside the
    /// AES-256-GCM ciphertext; private key material is encrypted and plaintext
    /// never touches disk. The caller owns authorization (verifying the master
    /// passphrase) and biometric credential cleanup before invoking this.
    pub fn reset(&mut self) -> Result<()> {
        self.ensure_writable()?;
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

    /// Reject any write through a snapshot: it holds a copy of the ciphertext
    /// taken at an arbitrary earlier moment, so persisting it could silently
    /// roll the vault file back over a newer change.
    fn ensure_writable(&self) -> Result<()> {
        if self.session_key.is_some() {
            return Err(CoreError::Vault(
                "vault snapshot is read-only".into(),
            ));
        }
        Ok(())
    }

    fn save(&self) -> Result<()> {
        self.ensure_writable()?;
        let data = serde_json::to_string_pretty(&self.file)?;
        crate::fs_util::write_private(&self.path, &data)
    }
}

async fn derive_key_off_thread(passphrase: &str, salt: Vec<u8>) -> Result<VaultKey> {
    let passphrase = Zeroizing::new(passphrase.to_owned());
    tokio::task::spawn_blocking(move || derive_key(&passphrase, &salt))
        .await
        .map_err(|e| CoreError::Vault(format!("kdf task: {e}")))?
}

fn derive_key(passphrase: &str, salt: &[u8]) -> Result<VaultKey> {
    let params = Params::new(64 * 1024, 3, 4, Some(32))
        .map_err(|e| CoreError::Vault(format!("argon2 params: {e}")))?;
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut out: VaultKey = Zeroizing::new([0u8; 32]);
    argon
        .hash_password_into(passphrase.as_bytes(), salt, &mut out[..])
        .map_err(|e| CoreError::Vault(format!("kdf: {e}")))?;
    Ok(out)
}

fn seal(key: &VaultKey, plaintext: &[u8]) -> Result<Vec<u8>> {
    let cipher = Aes256Gcm::new_from_slice(&key[..])
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

fn open(key: &VaultKey, ciphertext: &[u8]) -> Result<Vec<u8>> {
    if ciphertext.len() < 12 {
        return Err(CoreError::Vault("ciphertext too short".into()));
    }
    let (nonce_bytes, ct) = ciphertext.split_at(12);
    let cipher = Aes256Gcm::new_from_slice(&key[..])
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
    use super::{Vault, VaultKey};

    async fn initialized(path: std::path::PathBuf, passphrase: &str) -> (Vault, VaultKey) {
        let mut vault = Vault::open(path).expect("open vault");
        let key = vault.initialize(passphrase).await.expect("initialize");
        (vault, key)
    }

    #[tokio::test]
    async fn verify_passphrase_rejects_wrong_password() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("vault.json");
        let (vault, _) = initialized(path, "correct horse battery staple").await;

        assert!(vault
            .verify_passphrase("correct horse battery staple")
            .await
            .is_ok());
        assert!(vault.verify_passphrase("wrong passphrase").await.is_err());
    }

    #[tokio::test]
    async fn initialize_returns_the_key_that_unlocks_the_vault() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("vault.json");
        let (vault, key) = initialized(path, "correct horse battery staple").await;

        assert!(vault.verify_key(&key).is_ok());
        let derived = vault
            .derive_key("correct horse battery staple")
            .await
            .expect("derive");
        assert_eq!(&key[..], &derived[..]);
    }

    #[tokio::test]
    async fn a_snapshot_reads_key_material_without_the_passphrase() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("vault.json");
        let (mut vault, key) = initialized(path, "correct horse battery staple").await;
        let (private_pem, _) = crate::keys::generate_ed25519().expect("generate");
        let (meta, _) = crate::keys::parse_openssh_private(&private_pem, None).expect("parse");
        let key_id = meta.id.to_string();
        vault.add_key(&key, meta, &private_pem).expect("add key");

        let snapshot = vault.snapshot(key);
        // The snapshot carries the master key, so the passphrase argument is
        // never consulted: a wrong one still returns the key material.
        let material = snapshot
            .get_key("not the passphrase", &key_id)
            .await
            .expect("read key from snapshot");
        assert!(String::from_utf8(material)
            .expect("utf8")
            .contains("OPENSSH PRIVATE KEY"));
    }

    #[tokio::test]
    async fn without_a_snapshot_key_the_passphrase_still_decrypts() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("vault.json");
        let (mut vault, key) = initialized(path, "correct horse battery staple").await;
        let (private_pem, _) = crate::keys::generate_ed25519().expect("generate");
        let (meta, _) = crate::keys::parse_openssh_private(&private_pem, None).expect("parse");
        let key_id = meta.id.to_string();
        vault.add_key(&key, meta, &private_pem).expect("add key");

        assert!(vault
            .get_key("correct horse battery staple", &key_id)
            .await
            .is_ok());
        assert!(vault.get_key("wrong passphrase", &key_id).await.is_err());
    }

    #[tokio::test]
    async fn a_snapshot_can_never_write_the_vault_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("vault.json");
        let (vault, key) = initialized(path.clone(), "correct horse battery staple").await;
        let on_disk = std::fs::read(&path).expect("read vault");

        let mut snapshot = vault.snapshot(key.clone());
        let (private_pem, _) = crate::keys::generate_ed25519().expect("generate");
        let (meta, _) = crate::keys::parse_openssh_private(&private_pem, None).expect("parse");
        assert!(snapshot.add_key(&key, meta, &private_pem).is_err());
        assert!(snapshot.reset().is_err());

        assert_eq!(std::fs::read(&path).expect("reread vault"), on_disk);
    }

    #[tokio::test]
    async fn binding_id_is_stable_for_an_initialized_vault() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("vault.json");
        let mut vault = Vault::open(path.clone()).expect("open vault");
        assert!(vault.binding_id().is_none());

        vault
            .initialize("correct horse battery staple")
            .await
            .expect("initialize");
        let binding_id = vault.binding_id().expect("binding id").to_string();
        assert!(!binding_id.is_empty());
        drop(vault);

        let reopened = Vault::open(path).expect("reopen vault");
        assert_eq!(reopened.binding_id(), Some(binding_id.as_str()));
    }

    #[tokio::test]
    async fn reset_deletes_the_vault_file_and_clears_in_memory_state() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("vault.json");
        let (mut vault, _) = initialized(path.clone(), "correct horse battery staple").await;
        assert!(path.exists());
        assert!(vault.is_initialized());
        assert!(vault.binding_id().is_some());

        vault.reset().expect("reset vault");

        assert!(!path.exists());
        assert!(!vault.is_initialized());
        assert!(vault.binding_id().is_none());
        assert!(vault.keys_meta().is_empty());
    }

    #[tokio::test]
    async fn reset_clears_in_memory_state_even_when_directory_sync_fails() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("vault.json");
        let (mut vault, _) = initialized(path, "correct horse battery staple").await;

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

    #[tokio::test]
    async fn reset_deletes_a_crash_leftover_atomic_temp_copy() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("vault.json");
        let tmp = dir.path().join("vault.json.tmp");
        let (mut vault, _) = initialized(path.clone(), "correct horse battery staple").await;
        let encrypted_snapshot = std::fs::read(&path).expect("read encrypted vault");
        std::fs::write(&tmp, encrypted_snapshot).expect("seed crash temp copy");
        assert!(path.exists());
        assert!(tmp.exists());

        vault.reset().expect("reset vault");

        assert!(!path.exists());
        assert!(!tmp.exists());
        assert!(!vault.is_initialized());
    }

    #[tokio::test]
    async fn reset_allows_reinitializing_with_a_new_passphrase() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("vault.json");
        let (mut vault, _) = initialized(path, "first passphrase").await;
        let first_binding = vault.binding_id().expect("first binding").to_string();

        vault.reset().expect("reset vault");

        vault
            .initialize("second passphrase")
            .await
            .expect("initialize second");
        assert!(vault.is_initialized());
        assert!(vault.verify_passphrase("second passphrase").await.is_ok());
        assert!(vault.verify_passphrase("first passphrase").await.is_err());
        assert_ne!(vault.binding_id(), Some(first_binding.as_str()));
    }

    #[tokio::test]
    async fn reset_on_an_uninitialized_vault_is_a_noop() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("vault.json");
        let mut vault = Vault::open(path.clone()).expect("open vault");
        assert!(!vault.is_initialized());

        vault.reset().expect("reset uninitialized vault");

        assert!(!path.exists());
        assert!(!vault.is_initialized());
    }

    #[tokio::test]
    async fn failed_parent_creation_leaves_initialization_retryable() {
        let dir = tempfile::tempdir().expect("tempdir");
        let parent = dir.path().join("blocked-parent");
        std::fs::write(&parent, b"not a directory").expect("block parent");
        let path = parent.join("vault.json");
        let mut vault = Vault::open(path.clone()).expect("open vault");

        assert!(vault.initialize("first attempt passphrase").await.is_err());
        assert!(!vault.is_initialized());
        assert!(vault.binding_id().is_none());
        assert!(vault.keys_meta().is_empty());

        std::fs::remove_file(&parent).expect("repair parent");
        vault
            .initialize("retry passphrase")
            .await
            .expect("retry same vault");
        assert!(vault.is_initialized());
        assert!(vault.verify_passphrase("retry passphrase").await.is_ok());
        assert!(vault
            .verify_passphrase("first attempt passphrase")
            .await
            .is_err());
        let reopened = Vault::open(path).expect("reopen persisted vault");
        assert_eq!(reopened.binding_id(), vault.binding_id());
        assert!(reopened.verify_passphrase("retry passphrase").await.is_ok());
    }

    #[tokio::test]
    async fn failed_file_write_leaves_initialization_retryable() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("vault.json");
        let mut vault = Vault::open(path.clone()).expect("open vault");
        // Parent creation succeeds, but writing to a directory fails reliably
        // even when tests run with permission to write anywhere in the tempdir.
        std::fs::create_dir(&path).expect("block vault file");

        assert!(vault.initialize("first attempt passphrase").await.is_err());
        assert!(!vault.is_initialized());
        assert!(vault.binding_id().is_none());
        assert!(vault.keys_meta().is_empty());

        std::fs::remove_dir(&path).expect("repair vault path");
        vault
            .initialize("retry passphrase")
            .await
            .expect("retry same vault");
        let reopened = Vault::open(path).expect("reopen persisted vault");
        assert_eq!(reopened.binding_id(), vault.binding_id());
        assert!(reopened.verify_passphrase("retry passphrase").await.is_ok());
    }
}
