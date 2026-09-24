use crate::{CoreError, Result};
use base64::Engine;
use russh::keys::ssh_key::{self, Algorithm, HashAlg, PrivateKey, PublicKey};
use russh::keys::{decode_secret_key, PublicKeyBase64};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use zeroize::{Zeroize, Zeroizing};

/// Metadata about a stored key. The private material itself lives in the
/// encrypted vault, referenced by `id`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyMeta {
    pub id: Uuid,
    pub label: String,
    pub key_type: KeyType,
    pub fingerprint: String,
    pub public_key_base64: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum KeyType {
    Ed25519,
    Rsa,
    Ecdsa,
}

/// In-memory holder for decrypted private key material. Zeroizes on drop.
#[derive(Debug)]
pub struct PrivateKeyMaterial {
    pub id: Uuid,
    pub openssh: Vec<u8>,
}

impl Drop for PrivateKeyMaterial {
    fn drop(&mut self) {
        self.openssh.zeroize();
    }
}

/// SHA-256 fingerprint of a public key, as unpadded base64 with no `SHA256:`
/// prefix.
///
/// This exact form is stored in `known_hosts.json` and in vault key metadata,
/// and pins are compared against it, so it must not change shape.
pub fn fingerprint(key: &PublicKey) -> String {
    base64::engine::general_purpose::STANDARD_NO_PAD
        .encode(key.fingerprint(HashAlg::Sha256).as_bytes())
}

fn key_type_of(algorithm: &Algorithm) -> Result<KeyType> {
    match algorithm {
        Algorithm::Ed25519 => Ok(KeyType::Ed25519),
        Algorithm::Rsa { .. } => Ok(KeyType::Rsa),
        Algorithm::Ecdsa { .. } => Ok(KeyType::Ecdsa),
        other => Err(CoreError::Key(format!(
            "unsupported key type: {}",
            other.as_str()
        ))),
    }
}

/// Parse an OpenSSH-formatted private key string, returning metadata + the
/// private key. Does NOT persist anything.
///
/// A passphrase is refused for a key that has none. russh would ignore it, and
/// the import would then quietly accept a key the user believes is protected by
/// the passphrase they just typed.
pub fn parse_openssh_private(
    openssh: &str,
    passphrase: Option<&str>,
) -> Result<(KeyMeta, PrivateKey)> {
    if passphrase.is_some() && decode_secret_key(openssh, None).is_ok() {
        return Err(CoreError::Key(
            "parse: this key is not protected by a passphrase; leave the passphrase empty".into(),
        ));
    }
    let pair = decode_secret_key(openssh, passphrase)
        .map_err(|e| CoreError::Key(format!("parse: {e}")))?;
    let public = pair.public_key();
    let key_type = key_type_of(&public.algorithm())?;
    let fingerprint = fingerprint(public);
    let public_key_base64 = public.public_key_base64();
    let meta = KeyMeta {
        id: Uuid::new_v4(),
        label: String::new(),
        key_type,
        fingerprint,
        public_key_base64,
    };
    Ok((meta, pair))
}

/// Parse a private key for storage in the vault, returning its metadata and the
/// text to store.
///
/// The vault is the only encryption a stored key has: connecting reads it back
/// without a passphrase, and the key's own passphrase is not kept. A key given
/// with its passphrase is therefore stored in decrypted OpenSSH form; one
/// without a passphrase is stored exactly as given.
pub fn import_openssh_private(
    openssh: &str,
    passphrase: Option<&str>,
) -> Result<(KeyMeta, Zeroizing<String>)> {
    let (meta, private) = parse_openssh_private(openssh, passphrase)?;
    let stored = match passphrase {
        None => Zeroizing::new(openssh.to_string()),
        Some(_) => private
            .to_openssh(ssh_key::LineEnding::LF)
            .map_err(|e| CoreError::Key(format!("serialize: {e}")))?,
    };
    Ok((meta, stored))
}

/// The public half of a stored private key, as the key itself defines it:
/// `(key_type, fingerprint, public_key_base64)`.
///
/// Read from the key rather than from whatever is filed alongside it, so it can
/// be used to check that stored metadata still describes the key it names. The
/// OpenSSH private key format keeps the public key outside the encrypted
/// section, so a key carrying its own passphrase is covered too; a PEM key is
/// read through russh, which cannot open an encrypted one, and those report an
/// error rather than an unverified answer.
///
/// The values are produced by the same code that produced them when the key was
/// added, so they are comparable to what is on file rather than merely
/// equivalent.
pub fn public_identity(openssh: &str) -> Result<(KeyType, String, String)> {
    let public = match PrivateKey::from_openssh(openssh) {
        Ok(private) => private.public_key().clone(),
        // Not an OpenSSH-format key: a PEM one can still be read, but only
        // while it is not itself encrypted.
        Err(_) => decode_secret_key(openssh, None)
            .map_err(|e| CoreError::Key(format!("parse: {e}")))?
            .public_key()
            .clone(),
    };
    let key_type = key_type_of(&public.algorithm())?;
    Ok((key_type, fingerprint(&public), public.public_key_base64()))
}

/// Generate a new Ed25519 keypair. Returns (private OpenSSH PEM, public base64).
pub fn generate_ed25519() -> Result<(String, String)> {
    let mut rng = getrandom::rand_core::UnwrapErr(getrandom::SysRng);
    let private = PrivateKey::random(&mut rng, Algorithm::Ed25519)
        .map_err(|e| CoreError::Key(format!("generate: {e}")))?;
    let pem = private
        .to_openssh(ssh_key::LineEnding::LF)
        .map_err(|e| CoreError::Key(format!("serialize: {e}")))?;
    Ok((pem.to_string(), private.public_key().public_key_base64()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `public_identity` is only useful for checking stored metadata if it
    /// produces the same strings the metadata was written from, not merely
    /// equivalent ones.
    #[test]
    fn a_key_reports_the_same_public_identity_it_was_filed_under() {
        let (private, _) = generate_ed25519().expect("generate");
        let (meta, _) = parse_openssh_private(&private, None).expect("parse");

        let (key_type, fingerprint, public_key_base64) =
            public_identity(&private).expect("identity");

        assert_eq!(key_type, meta.key_type);
        assert_eq!(fingerprint, meta.fingerprint);
        assert_eq!(public_key_base64, meta.public_key_base64);
    }

    /// A vault can hold a key in its passphrase-protected form, so its identity
    /// has to be readable without the passphrase. The public half sits outside
    /// the encrypted section, so it is.
    #[test]
    fn a_passphrase_protected_key_still_reports_its_public_identity() {
        let (plain, _) = generate_ed25519().expect("generate");
        let (meta, _) = parse_openssh_private(&plain, None).expect("parse");
        let encrypted = PrivateKey::from_openssh(&plain)
            .expect("read")
            .encrypt(&mut getrandom::SysRng, "key passphrase")
            .expect("encrypt")
            .to_openssh(ssh_key::LineEnding::LF)
            .expect("serialize")
            .to_string();
        assert!(parse_openssh_private(&encrypted, None).is_err());

        let (_, fingerprint, public_key_base64) = public_identity(&encrypted).expect("identity");

        assert_eq!(fingerprint, meta.fingerprint);
        assert_eq!(public_key_base64, meta.public_key_base64);
    }

    #[test]
    fn text_that_is_not_a_key_reports_no_public_identity() {
        assert!(public_identity("not an openssh private key").is_err());
    }
}
