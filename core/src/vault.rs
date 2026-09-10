use crate::keys::KeyMeta;
use crate::{CoreError, Result};
use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use argon2::{Algorithm, Argon2, Params, Version};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use zeroize::Zeroizing;

/// Master key for a vault, derived from the user passphrase with Argon2id and
/// zeroized on drop. It is the only value that unlocks the payload; the
/// passphrase itself is not needed once this has been derived.
pub type VaultKey = Zeroizing<[u8; 32]>;

/// On-disk format this build writes.
///
/// Version 0 identifies a vault whose header carries no `version` field: its
/// nonce occupies the first twelve bytes of `ciphertext` and nothing outside
/// the ciphertext is authenticated. Version 1 gives the nonce its own field and
/// binds the whole header to the AES-GCM tag as associated data. Both are
/// readable; only version 1 is written.
const VAULT_FORMAT_VERSION: u32 = 1;

const NONCE_LEN: usize = 12;

/// Domain separator so the associated data of a vault can never collide with
/// authenticated bytes produced anywhere else.
const AAD_DOMAIN: &[u8] = b"clavyn.vault.header";

/// Encrypted-at-rest vault for private key material.
///
/// Layout on disk (JSON):
///   { "version": 1, "salt": "<b64>", "nonce": "<b64>", "epoch": <n>,
///     "keys_meta": [...], "ciphertext": "<b64>" }
///
/// The master key is derived from a user passphrase via Argon2id. The OS
/// keychain stores the passphrase (set by the UI layer), never the derived key.
/// Plaintext private keys only ever exist in memory and are zeroized on drop.
///
/// `keys_meta` sits in the header rather than inside the ciphertext because the
/// key list is rendered while the vault is locked, where no master key exists to
/// decrypt it. It is authenticated instead: the version, the salt, the epoch and
/// the serialized metadata are fed to AES-GCM as associated data, so editing any
/// of them makes the tag check fail and the vault refuses to open. Without that
/// binding, swapping a stored `public_key_base64` for an attacker's key leaves a
/// vault that still unlocks and hands the UI the attacker's key under the user's
/// own label.
///
/// Because the metadata is authenticated by its serialized form, any change to
/// the shape `KeyMeta` serializes to also changes the associated data of every
/// existing vault. Such a change needs a new `VAULT_FORMAT_VERSION` and a
/// migration that re-seals under the new shape.
///
/// `epoch` advances on every save and is authenticated, so it cannot be forged.
/// Recognizing a whole-file rollback additionally needs a high-water mark held
/// outside this file; nothing compares the epoch yet.
#[derive(Serialize, Deserialize, Clone)]
pub struct VaultFile {
    #[serde(default)]
    version: u32,
    salt: String,
    #[serde(default)]
    nonce: String,
    #[serde(default)]
    epoch: u64,
    pub keys_meta: Vec<KeyMeta>,
    ciphertext: String,
}

impl VaultFile {
    fn uninitialized() -> Self {
        Self {
            version: VAULT_FORMAT_VERSION,
            salt: String::new(),
            nonce: String::new(),
            epoch: 0,
            keys_meta: Vec::new(),
            ciphertext: String::new(),
        }
    }
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
            VaultFile::uninitialized()
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

    /// The on-disk format of the vault currently held. A vault written by an
    /// earlier release reports 0 until it is migrated.
    pub fn format_version(&self) -> u32 {
        self.file.version
    }

    /// Monotonic counter over saves of this vault generation.
    pub fn epoch(&self) -> u64 {
        self.file.epoch
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
        self.seal_and_persist(&key, base64(salt), 1, Vec::new(), &payload)?;
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
    /// any private-key material to the caller. This also checks the header,
    /// which is authenticated together with the payload.
    pub fn verify_key(&self, key: &VaultKey) -> Result<()> {
        if !self.is_initialized() {
            return Err(CoreError::Vault("vault not initialized".into()));
        }

        let plaintext = self.decrypt(key)?;
        serde_json::from_slice::<VaultPayload>(&plaintext)
            .map(|_| ())
            .map_err(CoreError::from)
    }

    /// Rewrite a vault held in an older on-disk format under the current one,
    /// reusing the already-derived master key. Reports whether a write happened.
    ///
    /// Only the framing and the authentication tag change; the decrypted payload
    /// and the metadata are carried across untouched, so no key material can be
    /// lost. The write is atomic and the in-memory vault is replaced only once it
    /// lands, so a failure leaves both the file and this vault on the format they
    /// already had and fully usable. Callers should therefore treat a failure as
    /// non-fatal rather than refusing the unlock it accompanies.
    pub fn migrate_to_current_format(&mut self, key: &VaultKey) -> Result<bool> {
        self.ensure_writable()?;
        if !self.is_initialized() || self.file.version == VAULT_FORMAT_VERSION {
            return Ok(false);
        }

        let plaintext = self.decrypt(key)?;
        let payload: VaultPayload = serde_json::from_slice(&plaintext)?;
        let salt = self.file.salt.clone();
        let epoch = self.next_epoch()?;
        let keys_meta = self.file.keys_meta.clone();
        self.seal_and_persist(key, salt, epoch, keys_meta, &payload)?;
        Ok(true)
    }

    pub fn add_key(&mut self, key: &VaultKey, meta: KeyMeta, private_openssh: &str) -> Result<()> {
        let plaintext = self.decrypt(key)?;
        let mut payload: VaultPayload = serde_json::from_slice(&plaintext)?;
        drop(plaintext);
        payload
            .keys
            .push((meta.id.to_string(), private_openssh.to_string()));

        let mut keys_meta = self.file.keys_meta.clone();
        keys_meta.push(meta);
        let salt = self.file.salt.clone();
        let epoch = self.next_epoch()?;
        self.seal_and_persist(key, salt, epoch, keys_meta, &payload)
    }

    pub fn remove_key(&mut self, key: &VaultKey, key_id: &str) -> Result<()> {
        let plaintext = self.decrypt(key)?;
        let mut payload: VaultPayload = serde_json::from_slice(&plaintext)?;
        drop(plaintext);
        payload.keys.retain(|(id, _)| id != key_id);

        let mut keys_meta = self.file.keys_meta.clone();
        keys_meta.retain(|m| m.id.to_string() != key_id);
        let salt = self.file.salt.clone();
        let epoch = self.next_epoch()?;
        self.seal_and_persist(key, salt, epoch, keys_meta, &payload)
    }

    /// Decrypt and return a single key's private material. Caller is responsible
    /// for the `PrivateKeyMaterial` (it zeroizes on drop).
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
    pub fn get_key_with(&self, key: &VaultKey, key_id: &str) -> Result<Vec<u8>> {
        let plaintext = self.decrypt(key)?;
        let payload: VaultPayload = serde_json::from_slice(&plaintext)?;
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
        self.file = VaultFile::uninitialized();

        if let Some(error) = removal.durability_error {
            return Err(error.into());
        }
        Ok(())
    }

    /// Decrypt the payload, checking the header along with it. The format
    /// version selects how the ciphertext is framed and what is authenticated.
    fn decrypt(&self, key: &VaultKey) -> Result<Zeroizing<Vec<u8>>> {
        let ciphertext = unbase64(&self.file.ciphertext)?;
        match self.file.version {
            0 => open_unbound(key, &ciphertext),
            VAULT_FORMAT_VERSION => {
                let nonce = unbase64(&self.file.nonce)?;
                let aad = associated_data(
                    self.file.version,
                    &self.file.salt,
                    self.file.epoch,
                    &self.file.keys_meta,
                )?;
                open_sealed(key, &nonce, &aad, &ciphertext)
            }
            other => Err(CoreError::Vault(format!(
                "vault format version {other} is not supported by this build; update Clavyn"
            ))),
        }
    }

    /// Seal `payload` under `key`, bind the header to it, and replace the file.
    ///
    /// The in-memory vault is published only after the atomic write lands, so a
    /// save that fails for any reason leaves this vault and the file agreeing on
    /// the last state that was successfully persisted.
    fn seal_and_persist(
        &mut self,
        key: &VaultKey,
        salt: String,
        epoch: u64,
        keys_meta: Vec<KeyMeta>,
        payload: &VaultPayload,
    ) -> Result<()> {
        self.ensure_writable()?;

        let plaintext = Zeroizing::new(serde_json::to_vec(payload)?);
        let aad = associated_data(VAULT_FORMAT_VERSION, &salt, epoch, &keys_meta)?;
        let (nonce, ciphertext) = seal(key, &aad, &plaintext)?;
        let candidate = VaultFile {
            version: VAULT_FORMAT_VERSION,
            salt,
            nonce: base64(nonce),
            epoch,
            keys_meta,
            ciphertext: base64(ciphertext),
        };

        persist(&self.path, &candidate)?;
        self.file = candidate;
        Ok(())
    }

    fn next_epoch(&self) -> Result<u64> {
        self.file
            .epoch
            .checked_add(1)
            .ok_or_else(|| CoreError::Vault("vault epoch would overflow".into()))
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
}

fn persist(path: &Path, file: &VaultFile) -> Result<()> {
    let data = serde_json::to_string_pretty(file)?;
    crate::fs_util::write_private(path, &data)
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

/// Build the associated data that ties a header to its ciphertext.
///
/// Every field is length-prefixed so no two different headers can produce the
/// same bytes by shifting a boundary. `keys_meta` is included through its
/// serialized form, which is what makes tampering with a stored public key or
/// fingerprint fail the tag check.
fn associated_data(
    version: u32,
    salt: &str,
    epoch: u64,
    keys_meta: &[KeyMeta],
) -> Result<Vec<u8>> {
    let meta = serde_json::to_vec(keys_meta)?;
    let mut aad = Vec::with_capacity(AAD_DOMAIN.len() + salt.len() + meta.len() + 32);
    aad.extend_from_slice(AAD_DOMAIN);
    aad.extend_from_slice(&version.to_be_bytes());
    push_length_prefixed(&mut aad, salt.as_bytes());
    aad.extend_from_slice(&epoch.to_be_bytes());
    push_length_prefixed(&mut aad, &meta);
    Ok(aad)
}

fn push_length_prefixed(out: &mut Vec<u8>, field: &[u8]) {
    out.extend_from_slice(&(field.len() as u64).to_be_bytes());
    out.extend_from_slice(field);
}

fn seal(key: &VaultKey, aad: &[u8], plaintext: &[u8]) -> Result<([u8; NONCE_LEN], Vec<u8>)> {
    let cipher = Aes256Gcm::new_from_slice(&key[..])
        .map_err(|e| CoreError::Vault(format!("aes init: {e}")))?;
    let mut nonce_bytes = [0u8; NONCE_LEN];
    use rand::RngCore;
    rand::rngs::OsRng.fill_bytes(&mut nonce_bytes);
    let ciphertext = cipher
        .encrypt(
            Nonce::from_slice(&nonce_bytes),
            Payload {
                msg: plaintext,
                aad,
            },
        )
        .map_err(|e| CoreError::Vault(format!("encrypt: {e}")))?;
    Ok((nonce_bytes, ciphertext))
}

fn open_sealed(
    key: &VaultKey,
    nonce: &[u8],
    aad: &[u8],
    ciphertext: &[u8],
) -> Result<Zeroizing<Vec<u8>>> {
    // `Nonce::from_slice` panics on a wrong length, and the nonce comes straight
    // from the file, so check it here rather than trusting the input.
    if nonce.len() != NONCE_LEN {
        return Err(CoreError::Vault(format!(
            "vault nonce must be {NONCE_LEN} bytes"
        )));
    }
    let cipher = Aes256Gcm::new_from_slice(&key[..])
        .map_err(|e| CoreError::Vault(format!("aes init: {e}")))?;
    cipher
        .decrypt(
            Nonce::from_slice(nonce),
            Payload {
                msg: ciphertext,
                aad,
            },
        )
        .map(Zeroizing::new)
        .map_err(|_| {
            CoreError::Vault(
                "decrypt failed: wrong passphrase, or the vault file was modified \
                 outside Clavyn"
                    .into(),
            )
        })
}

/// Read a version 0 vault, whose nonce is the first twelve bytes of the
/// ciphertext and whose tag covers nothing else.
fn open_unbound(key: &VaultKey, ciphertext: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
    if ciphertext.len() < NONCE_LEN {
        return Err(CoreError::Vault("ciphertext too short".into()));
    }
    let (nonce, body) = ciphertext.split_at(NONCE_LEN);
    open_sealed(key, nonce, &[], body)
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
    use super::{
        base64, derive_key, open_unbound, seal, unbase64, Vault, VaultKey, VaultPayload,
        VAULT_FORMAT_VERSION,
    };
    use crate::keys::KeyMeta;
    use std::path::Path;

    /// The header shape a build that predates the authenticated format reads.
    /// Serde ignores unknown fields, so this also stands in for that build when
    /// it is pointed at a current-format file.
    #[derive(serde::Serialize, serde::Deserialize)]
    struct UnboundVaultFile {
        salt: String,
        ciphertext: String,
        keys_meta: Vec<KeyMeta>,
    }

    async fn initialized(path: std::path::PathBuf, passphrase: &str) -> (Vault, VaultKey) {
        let mut vault = Vault::open(path).expect("open vault");
        let key = vault.initialize(passphrase).await.expect("initialize");
        (vault, key)
    }

    fn a_key() -> (KeyMeta, String) {
        let (private_pem, _) = crate::keys::generate_ed25519().expect("generate");
        let (meta, _) = crate::keys::parse_openssh_private(&private_pem, None).expect("parse");
        (meta, private_pem)
    }

    fn read_json(path: &Path) -> serde_json::Value {
        serde_json::from_str(&std::fs::read_to_string(path).expect("read vault"))
            .expect("parse vault json")
    }

    fn write_json(path: &Path, value: &serde_json::Value) {
        std::fs::write(path, serde_json::to_string_pretty(value).expect("serialize"))
            .expect("write vault");
    }

    /// Write a version 0 vault holding `keys`, and return the master key.
    fn write_unbound_vault(
        path: &Path,
        passphrase: &str,
        keys: Vec<(String, String)>,
        keys_meta: Vec<KeyMeta>,
    ) -> VaultKey {
        let mut salt = [0u8; 16];
        use rand::RngCore;
        rand::rngs::OsRng.fill_bytes(&mut salt);
        let key = derive_key(passphrase, &salt).expect("derive");
        let plaintext = serde_json::to_vec(&VaultPayload { keys }).expect("serialize payload");
        let (nonce, body) = seal(&key, &[], &plaintext).expect("seal");
        let mut ciphertext = Vec::with_capacity(nonce.len() + body.len());
        ciphertext.extend_from_slice(&nonce);
        ciphertext.extend_from_slice(&body);
        let file = UnboundVaultFile {
            salt: base64(salt),
            ciphertext: base64(ciphertext),
            keys_meta,
        };
        std::fs::write(
            path,
            serde_json::to_string_pretty(&file).expect("serialize vault"),
        )
        .expect("write vault");
        key
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
    async fn a_new_vault_is_written_in_the_current_format() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("vault.json");
        let (vault, _) = initialized(path.clone(), "correct horse battery staple").await;

        assert_eq!(vault.format_version(), VAULT_FORMAT_VERSION);
        assert_eq!(vault.epoch(), 1);
        let file = read_json(&path);
        assert_eq!(file["version"], serde_json::json!(VAULT_FORMAT_VERSION));
        assert_eq!(file["epoch"], serde_json::json!(1));
        assert_eq!(
            unbase64(file["nonce"].as_str().expect("nonce"))
                .expect("decode nonce")
                .len(),
            12
        );
    }

    #[tokio::test]
    async fn a_round_trip_recovers_exactly_what_went_in() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("vault.json");
        let (mut vault, key) = initialized(path.clone(), "correct horse battery staple").await;

        let (first_meta, first_private) = a_key();
        let (second_meta, second_private) = a_key();
        let first_id = first_meta.id.to_string();
        let second_id = second_meta.id.to_string();
        vault
            .add_key(&key, first_meta.clone(), &first_private)
            .expect("add first");
        vault
            .add_key(&key, second_meta.clone(), &second_private)
            .expect("add second");
        drop(vault);

        let reopened = Vault::open(path).expect("reopen");
        let reopened_key = reopened
            .verify_passphrase("correct horse battery staple")
            .await
            .expect("unlock");
        assert_eq!(
            reopened.get_key_with(&reopened_key, &first_id).expect("first"),
            first_private.as_bytes()
        );
        assert_eq!(
            reopened
                .get_key_with(&reopened_key, &second_id)
                .expect("second"),
            second_private.as_bytes()
        );
        let meta = reopened.keys_meta();
        assert_eq!(meta.len(), 2);
        assert_eq!(meta[0].public_key_base64, first_meta.public_key_base64);
        assert_eq!(meta[1].fingerprint, second_meta.fingerprint);
    }

    #[tokio::test]
    async fn substituting_a_stored_public_key_makes_the_vault_refuse_to_open() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("vault.json");
        let (mut vault, key) = initialized(path.clone(), "correct horse battery staple").await;
        let (mut meta, private) = a_key();
        meta.label = "prod-bastion".into();
        vault.add_key(&key, meta, &private).expect("add");
        drop(vault);

        let (attacker_meta, _) = a_key();
        let mut file = read_json(&path);
        file["keys_meta"][0]["public_key_base64"] =
            serde_json::json!(attacker_meta.public_key_base64);
        file["keys_meta"][0]["fingerprint"] = serde_json::json!(attacker_meta.fingerprint);
        write_json(&path, &file);

        let tampered = Vault::open(path).expect("reopen");
        let error = tampered
            .verify_passphrase("correct horse battery staple")
            .await
            .expect_err("a substituted public key must not unlock");
        assert!(
            error.to_string().contains("modified outside Clavyn"),
            "unexpected error: {error}"
        );
    }

    #[tokio::test]
    async fn editing_any_authenticated_header_field_is_detected() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (_, private) = a_key();

        for (field, value) in [
            ("epoch", serde_json::json!(99u64)),
            ("version", serde_json::json!(0u32)),
        ] {
            let path = dir.path().join(format!("vault-{field}.json"));
            let (mut vault, key) = initialized(path.clone(), "correct horse battery staple").await;
            let (meta, _) = a_key();
            vault.add_key(&key, meta, &private).expect("add");
            drop(vault);

            let mut file = read_json(&path);
            file[field] = value;
            write_json(&path, &file);

            let tampered = Vault::open(path).expect("reopen");
            assert!(
                tampered.verify_key(&key).is_err(),
                "editing {field} went undetected"
            );
        }
    }

    #[tokio::test]
    async fn dropping_a_key_from_the_metadata_is_detected() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("vault.json");
        let (mut vault, key) = initialized(path.clone(), "correct horse battery staple").await;
        let (meta, private) = a_key();
        vault.add_key(&key, meta, &private).expect("add");
        drop(vault);

        let mut file = read_json(&path);
        file["keys_meta"] = serde_json::json!([]);
        write_json(&path, &file);

        let tampered = Vault::open(path).expect("reopen");
        assert!(tampered.verify_key(&key).is_err());
    }

    #[tokio::test]
    async fn key_metadata_stays_readable_while_the_vault_is_locked() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("vault.json");
        let (mut vault, key) = initialized(path.clone(), "correct horse battery staple").await;
        let (mut meta, private) = a_key();
        meta.label = "prod-bastion".into();
        let public_key = meta.public_key_base64.clone();
        vault.add_key(&key, meta, &private).expect("add");
        drop(vault);

        // The key list is rendered before any unlock, so the metadata cannot
        // live inside the ciphertext; it is authenticated in place instead.
        let locked = Vault::open(path).expect("reopen");
        assert_eq!(locked.keys_meta().len(), 1);
        assert_eq!(locked.keys_meta()[0].label, "prod-bastion");
        assert_eq!(locked.keys_meta()[0].public_key_base64, public_key);
    }

    #[tokio::test]
    async fn a_vault_in_the_older_format_migrates_on_unlock_without_losing_keys() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("vault.json");
        let (meta, private) = a_key();
        let key_id = meta.id.to_string();
        let public_key = meta.public_key_base64.clone();
        write_unbound_vault(
            &path,
            "correct horse battery staple",
            vec![(key_id.clone(), private.clone())],
            vec![meta],
        );

        let mut vault = Vault::open(path.clone()).expect("open");
        assert_eq!(vault.format_version(), 0);
        let binding_id = vault.binding_id().expect("binding id").to_string();
        let key = vault
            .verify_passphrase("correct horse battery staple")
            .await
            .expect("older format still unlocks");
        assert!(vault.migrate_to_current_format(&key).expect("migrate"));

        // The binding id addresses the biometric Keychain credential and its
        // enrollment marker. Changing it would orphan both, so the migration
        // must keep the salt it is derived from.
        assert_eq!(vault.binding_id(), Some(binding_id.as_str()));
        assert_eq!(vault.format_version(), VAULT_FORMAT_VERSION);
        assert_eq!(vault.epoch(), 1);
        let reopened = Vault::open(path).expect("reopen");
        assert_eq!(reopened.format_version(), VAULT_FORMAT_VERSION);
        assert_eq!(reopened.keys_meta().len(), 1);
        assert_eq!(reopened.keys_meta()[0].public_key_base64, public_key);
        let reopened_key = reopened
            .verify_passphrase("correct horse battery staple")
            .await
            .expect("migrated vault unlocks with the same passphrase");
        assert_eq!(
            reopened.get_key_with(&reopened_key, &key_id).expect("key"),
            private.as_bytes()
        );
    }

    #[tokio::test]
    async fn a_migrated_vault_detects_metadata_substitution() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("vault.json");
        let (meta, private) = a_key();
        let key_id = meta.id.to_string();
        write_unbound_vault(
            &path,
            "correct horse battery staple",
            vec![(key_id, private)],
            vec![meta],
        );

        let mut vault = Vault::open(path.clone()).expect("open");
        let key = vault
            .verify_passphrase("correct horse battery staple")
            .await
            .expect("unlock");
        assert!(vault.migrate_to_current_format(&key).expect("migrate"));
        drop(vault);

        let (attacker_meta, _) = a_key();
        let mut file = read_json(&path);
        file["keys_meta"][0]["public_key_base64"] =
            serde_json::json!(attacker_meta.public_key_base64);
        write_json(&path, &file);

        let tampered = Vault::open(path).expect("reopen");
        assert!(tampered
            .verify_passphrase("correct horse battery staple")
            .await
            .is_err());
    }

    #[tokio::test]
    async fn migrating_a_current_format_vault_changes_nothing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("vault.json");
        let (mut vault, key) = initialized(path.clone(), "correct horse battery staple").await;
        let (meta, private) = a_key();
        vault.add_key(&key, meta, &private).expect("add");
        let before = std::fs::read(&path).expect("read");
        let epoch = vault.epoch();

        assert!(!vault.migrate_to_current_format(&key).expect("migrate"));

        assert_eq!(vault.epoch(), epoch);
        assert_eq!(std::fs::read(&path).expect("read"), before);
    }

    #[tokio::test]
    async fn a_migration_that_cannot_be_written_leaves_the_old_vault_usable() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("vault.json");
        let (meta, private) = a_key();
        let key_id = meta.id.to_string();
        write_unbound_vault(
            &path,
            "correct horse battery staple",
            vec![(key_id.clone(), private.clone())],
            vec![meta],
        );
        let before = std::fs::read(&path).expect("read");

        // The atomic write stages through a sibling temporary file. A directory
        // in its place fails the write reliably, before anything is renamed over
        // the vault.
        let staged = dir.path().join("vault.json.tmp");
        std::fs::create_dir(&staged).expect("block the staged write");

        let mut vault = Vault::open(path.clone()).expect("open");
        let key = vault
            .verify_passphrase("correct horse battery staple")
            .await
            .expect("unlock");
        assert!(vault.migrate_to_current_format(&key).is_err());

        // Both the file and the in-memory vault stay on the format they had.
        assert_eq!(std::fs::read(&path).expect("read"), before);
        assert_eq!(vault.format_version(), 0);
        assert_eq!(
            vault.get_key_with(&key, &key_id).expect("key still readable"),
            private.as_bytes()
        );

        std::fs::remove_dir(&staged).expect("unblock");
        assert!(vault.migrate_to_current_format(&key).expect("retry"));
        assert_eq!(vault.format_version(), VAULT_FORMAT_VERSION);
        let reopened = Vault::open(path).expect("reopen");
        assert_eq!(
            reopened
                .verify_passphrase("correct horse battery staple")
                .await
                .and_then(|k| reopened.get_key_with(&k, &key_id))
                .expect("key survives the retry"),
            private.as_bytes()
        );
    }

    #[tokio::test]
    async fn a_current_format_vault_fails_closed_for_a_reader_that_expects_the_old_one() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("vault.json");
        let (mut vault, key) = initialized(path.clone(), "correct horse battery staple").await;
        let (meta, private) = a_key();
        vault.add_key(&key, meta, &private).expect("add");
        drop(vault);

        // Serde ignores the fields the older shape does not declare, so parsing
        // succeeds; the decrypt is what must refuse.
        let raw = std::fs::read_to_string(&path).expect("read");
        let older: UnboundVaultFile = serde_json::from_str(&raw).expect("older reader parses");
        let ciphertext = unbase64(&older.ciphertext).expect("decode");
        assert!(
            open_unbound(&key, &ciphertext).is_err(),
            "an older reader must not decrypt a current-format vault"
        );
    }

    #[tokio::test]
    async fn a_vault_written_by_a_newer_build_is_refused() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("vault.json");
        let (_, key) = initialized(path.clone(), "correct horse battery staple").await;

        let mut file = read_json(&path);
        file["version"] = serde_json::json!(VAULT_FORMAT_VERSION + 1);
        write_json(&path, &file);

        let vault = Vault::open(path).expect("reopen");
        let error = vault.verify_key(&key).expect_err("must refuse");
        assert!(
            error.to_string().contains("not supported by this build"),
            "unexpected error: {error}"
        );
    }

    #[tokio::test]
    async fn a_truncated_vault_file_fails_with_an_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("vault.json");
        let (_, key) = initialized(path.clone(), "correct horse battery staple").await;

        let raw = std::fs::read_to_string(&path).expect("read");
        std::fs::write(&path, &raw[..raw.len() / 2]).expect("truncate");
        assert!(Vault::open(path.clone()).is_err());

        // A structurally valid file whose ciphertext has been cut short must
        // also fail rather than panic.
        let mut file: serde_json::Value = serde_json::from_str(&raw).expect("parse");
        let ciphertext = unbase64(file["ciphertext"].as_str().expect("ciphertext")).expect("decode");
        file["ciphertext"] = serde_json::json!(base64(&ciphertext[..ciphertext.len() / 2]));
        write_json(&path, &file);
        assert!(Vault::open(path).expect("reopen").verify_key(&key).is_err());
    }

    #[tokio::test]
    async fn a_malformed_nonce_is_rejected_instead_of_panicking() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("vault.json");
        let (_, key) = initialized(path.clone(), "correct horse battery staple").await;

        for nonce in ["", "AAAA", "bm90LWJhc2U2NCE/Pz8_"] {
            let mut file = read_json(&path);
            file["nonce"] = serde_json::json!(nonce);
            write_json(&path, &file);
            assert!(
                Vault::open(path.clone()).expect("reopen").verify_key(&key).is_err(),
                "nonce {nonce:?} was accepted"
            );
        }
    }

    #[tokio::test]
    async fn the_epoch_advances_on_every_save() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("vault.json");
        let (mut vault, key) = initialized(path, "correct horse battery staple").await;
        assert_eq!(vault.epoch(), 1);

        let (meta, private) = a_key();
        let key_id = meta.id.to_string();
        vault.add_key(&key, meta, &private).expect("add");
        assert_eq!(vault.epoch(), 2);
        vault.remove_key(&key, &key_id).expect("remove");
        assert_eq!(vault.epoch(), 3);
    }

    #[tokio::test]
    async fn a_failed_save_leaves_the_vault_on_its_last_persisted_state() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("vault.json");
        let (mut vault, key) = initialized(path.clone(), "correct horse battery staple").await;
        let before = std::fs::read(&path).expect("read");

        let staged = dir.path().join("vault.json.tmp");
        std::fs::create_dir(&staged).expect("block the staged write");

        let (meta, private) = a_key();
        assert!(vault.add_key(&key, meta, &private).is_err());

        assert_eq!(std::fs::read(&path).expect("read"), before);
        assert_eq!(vault.epoch(), 1);
        assert!(vault.keys_meta().is_empty());
        assert!(vault.verify_key(&key).is_ok());
    }

    #[tokio::test]
    async fn a_snapshot_reads_key_material_without_the_passphrase() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("vault.json");
        let (mut vault, key) = initialized(path, "correct horse battery staple").await;
        let (meta, private_pem) = a_key();
        let key_id = meta.id.to_string();
        vault.add_key(&key, meta, &private_pem).expect("add key");

        let snapshot = vault.snapshot(key);
        let cached = snapshot
            .get_key("this passphrase is never used", &key_id)
            .await
            .expect("snapshot reads the key without deriving");
        assert_eq!(cached, private_pem.as_bytes());

        let cold = vault
            .get_key("correct horse battery staple", &key_id)
            .await
            .expect("passphrase still works without a cached key");
        assert_eq!(cold, private_pem.as_bytes());
    }

    #[tokio::test]
    async fn a_snapshot_can_never_write_the_vault_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("vault.json");
        let (vault, key) = initialized(path, "correct horse battery staple").await;
        let mut snapshot = vault.snapshot(key.clone());
        let (meta, private_pem) = a_key();

        assert!(snapshot.add_key(&key, meta, &private_pem).is_err());
        assert!(snapshot.remove_key(&key, "any-key").is_err());
        assert!(snapshot.migrate_to_current_format(&key).is_err());
        assert!(snapshot.reset().is_err());
        assert!(snapshot
            .initialize("another passphrase")
            .await
            .is_err());
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
        assert_eq!(vault.epoch(), 0);
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
