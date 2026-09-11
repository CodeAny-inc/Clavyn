use crate::{CoreError, Result};
use russh_keys::key;
use russh_keys::{decode_secret_key, PublicKeyBase64};
use serde::{Deserialize, Serialize};
use ssh_key::PrivateKey;
use uuid::Uuid;
use zeroize::Zeroize;

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

/// Parse an OpenSSH-formatted private key string, returning metadata + the
/// russh keypair. Does NOT persist anything.
pub fn parse_openssh_private(
    openssh: &str,
    passphrase: Option<&str>,
) -> Result<(KeyMeta, key::KeyPair)> {
    let pair = decode_secret_key(openssh, passphrase)
        .map_err(|e| CoreError::Key(format!("parse: {e}")))?;
    let public = pair
        .clone_public_key()
        .map_err(|e| CoreError::Key(format!("clone public: {e}")))?;
    let fingerprint = public.fingerprint();
    let public_key_base64 = public.public_key_base64();
    let key_type = match &pair {
        key::KeyPair::Ed25519 { .. } => KeyType::Ed25519,
        key::KeyPair::RSA { .. } => KeyType::Rsa,
        key::KeyPair::EC { .. } => KeyType::Ecdsa,
    };
    let meta = KeyMeta {
        id: Uuid::new_v4(),
        label: String::new(),
        key_type,
        fingerprint,
        public_key_base64,
    };
    Ok((meta, pair))
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
        Ok(private) => {
            let line = private
                .public_key()
                .to_openssh()
                .map_err(|e| CoreError::Key(format!("public serialize: {e}")))?;
            let body = line
                .split_whitespace()
                .nth(1)
                .ok_or_else(|| CoreError::Key("public key has no body".to_string()))?;
            russh_keys::parse_public_key_base64(body)
                .map_err(|e| CoreError::Key(format!("public parse: {e}")))?
        }
        // Not an OpenSSH-format key: a PEM one can still be read, but only
        // while it is not itself encrypted.
        Err(_) => decode_secret_key(openssh, None)
            .map_err(|e| CoreError::Key(format!("parse: {e}")))?
            .clone_public_key()
            .map_err(|e| CoreError::Key(format!("clone public: {e}")))?,
    };
    let key_type = match &public {
        key::PublicKey::Ed25519(_) => KeyType::Ed25519,
        key::PublicKey::RSA { .. } => KeyType::Rsa,
        key::PublicKey::EC { .. } => KeyType::Ecdsa,
    };
    Ok((key_type, public.fingerprint(), public.public_key_base64()))
}

/// Generate a new Ed25519 keypair. Returns (private OpenSSH PEM, public base64).
pub fn generate_ed25519() -> Result<(String, String)> {
    let private = PrivateKey::random(&mut rand::rngs::OsRng, ssh_key::Algorithm::Ed25519)
        .map_err(|e| CoreError::Key(format!("generate: {e}")))?;
    let pem = private
        .to_openssh(ssh_key::LineEnding::LF)
        .map_err(|e| CoreError::Key(format!("serialize: {e}")))?;
    let public_b64 = private
        .public_key()
        .to_openssh()
        .map_err(|e| CoreError::Key(format!("public serialize: {e}")))?;
    // Extract just the base64 part from "ssh-ed25519 AAAA... comment"
    let public_b64 = public_b64
        .split_whitespace()
        .nth(1)
        .unwrap_or(&public_b64)
        .to_string();
    Ok((pem.to_string(), public_b64))
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

    /// The vault stores whichever text was imported, and for a key carrying its
    /// own passphrase that is the encrypted form. Its public half sits outside
    /// the encrypted section, so it can still be read.
    #[test]
    fn a_passphrase_protected_key_still_reports_its_public_identity() {
        let (plain, _) = generate_ed25519().expect("generate");
        let (meta, _) = parse_openssh_private(&plain, None).expect("parse");
        let encrypted = PrivateKey::from_openssh(&plain)
            .expect("read")
            .encrypt(&mut rand::rngs::OsRng, "key passphrase")
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
