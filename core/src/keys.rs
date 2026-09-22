use crate::{CoreError, Result};
use base64::Engine;
use russh::keys::ssh_key::{self, Algorithm, HashAlg, PrivateKey, PublicKey};
use russh::keys::{decode_secret_key, PublicKeyBase64};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use zeroize::Zeroizing;

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
pub fn parse_openssh_private(
    openssh: &str,
    passphrase: Option<&str>,
) -> Result<(KeyMeta, PrivateKey)> {
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
            russh::keys::parse_public_key_base64(body)
                .map_err(|e| CoreError::Key(format!("public parse: {e}")))?
        }
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
/// The private half is wiped when the caller drops it.
pub fn generate_ed25519() -> Result<(Zeroizing<String>, String)> {
    let mut rng = getrandom::rand_core::UnwrapErr(getrandom::SysRng);
    let private = PrivateKey::random(&mut rng, Algorithm::Ed25519)
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
    Ok((pem, public_b64))
}

/// The private key text to store in the vault: `openssh` itself, or, when it
/// carries its own passphrase, the same key with that protection removed.
///
/// The vault is what protects stored keys, and the connect path reads a stored
/// key without a passphrase, so a key kept in its encrypted form imports
/// without complaint and can then never be used. An OpenSSH key is decrypted
/// and written back in OpenSSH form; an encrypted PEM key is decoded and
/// written as unencrypted PKCS#8.
pub fn unprotected_private_key(openssh: &str, passphrase: Option<&str>) -> Result<Zeroizing<String>> {
    let Some(passphrase) = passphrase.filter(|p| !p.is_empty()) else {
        return Ok(Zeroizing::new(openssh.to_owned()));
    };
    if let Ok(private) = PrivateKey::from_openssh(openssh) {
        if !private.is_encrypted() {
            return Ok(Zeroizing::new(openssh.to_owned()));
        }
        return private
            .decrypt(passphrase)
            .map_err(|e| CoreError::Key(format!("decrypt: {e}")))?
            .to_openssh(ssh_key::LineEnding::LF)
            .map_err(|e| CoreError::Key(format!("serialize: {e}")));
    }
    let pair = decode_secret_key(openssh, Some(passphrase))
        .map_err(|e| CoreError::Key(format!("parse: {e}")))?;
    let mut pem = Zeroizing::new(Vec::new());
    russh::keys::encode_pkcs8_pem(&pair, &mut *pem)
        .map_err(|e| CoreError::Key(format!("serialize: {e}")))?;
    String::from_utf8(pem.to_vec())
        .map(Zeroizing::new)
        .map_err(|e| CoreError::Key(format!("serialize: {e}")))
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
        let encrypted = protected_openssh(&plain, "key passphrase");
        assert!(parse_openssh_private(&encrypted, None).is_err());

        let (_, fingerprint, public_key_base64) = public_identity(&encrypted).expect("identity");

        assert_eq!(fingerprint, meta.fingerprint);
        assert_eq!(public_key_base64, meta.public_key_base64);
    }

    fn protected_openssh(plain: &str, passphrase: &str) -> String {
        PrivateKey::from_openssh(plain)
            .expect("read")
            .encrypt(&mut getrandom::SysRng, passphrase)
            .expect("encrypt")
            .to_openssh(ssh_key::LineEnding::LF)
            .expect("serialize")
            .to_string()
    }

    /// A key imported with its own passphrase is stored in a form the connect
    /// path, which decodes stored keys without a passphrase, can use.
    #[test]
    fn a_protected_openssh_key_is_stored_without_its_passphrase() {
        let (plain, _) = generate_ed25519().expect("generate");
        let (meta, _) = parse_openssh_private(&plain, None).expect("parse");
        let protected = protected_openssh(&plain, "key passphrase");

        let stored = unprotected_private_key(&protected, Some("key passphrase")).expect("unprotect");
        let (stored_meta, _) = parse_openssh_private(&stored, None).expect("usable without passphrase");
        assert_eq!(stored_meta.fingerprint, meta.fingerprint);

        assert!(unprotected_private_key(&protected, Some("wrong")).is_err());
    }

    #[test]
    fn a_protected_pem_key_is_stored_as_plain_pkcs8() {
        let (plain, _) = generate_ed25519().expect("generate");
        let (meta, pair) = parse_openssh_private(&plain, None).expect("parse");
        let mut protected = Vec::new();
        russh::keys::encode_pkcs8_pem_encrypted(&pair, b"key passphrase", 16, &mut protected)
            .expect("encrypt pem");
        let protected = String::from_utf8(protected).expect("utf8");

        let stored = unprotected_private_key(&protected, Some("key passphrase")).expect("unprotect");
        assert!(stored.contains("BEGIN PRIVATE KEY"), "{}", &*stored);
        let (stored_meta, _) = parse_openssh_private(&stored, None).expect("usable without passphrase");
        assert_eq!(stored_meta.fingerprint, meta.fingerprint);
    }

    #[test]
    fn an_unprotected_key_is_stored_as_given() {
        let (plain, _) = generate_ed25519().expect("generate");
        assert_eq!(*unprotected_private_key(&plain, None).expect("none"), *plain);
        assert_eq!(*unprotected_private_key(&plain, Some("unused")).expect("unused"), *plain);
    }

    #[test]
    fn text_that_is_not_a_key_reports_no_public_identity() {
        assert!(public_identity("not an openssh private key").is_err());
    }
}
