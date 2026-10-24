use crate::{CoreError, Result};
use base64::Engine;
use russh::keys::ssh_key::{self, Algorithm, HashAlg, Kdf, PrivateKey, PublicKey};
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

/// Most bcrypt-pbkdf rounds an OpenSSH key may ask for.
///
/// The round count is read from the key file and all of it is paid before the
/// passphrase can even be checked, so it bounds the work an imported file can
/// demand: the format allows up to 2^32 - 1 rounds, which would never finish.
/// `ssh-keygen` uses 16 unless told otherwise with `-a`, and hardening guides
/// stay within a few hundred.
pub const MAX_BCRYPT_ROUNDS: u32 = 1000;

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
    let pair = match openssh_body(openssh) {
        Some(body) => decode_openssh(&body, passphrase)?,
        None => decode_other(openssh, passphrase)?,
    };
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

/// The base64 body of an OpenSSH-format private key, or `None` for any other
/// format.
///
/// Gathered the way `decode_secret_key` gathers it, so text russh would read as
/// an OpenSSH key is read as one here too and never reaches russh's own
/// decryption, which puts no limit on the KDF rounds.
fn openssh_body(text: &str) -> Option<String> {
    if text.trim().starts_with("PuTTY-User-Key-File-") {
        return None;
    }
    let mut started = false;
    let mut openssh = false;
    let mut body = String::new();
    for line in text.lines() {
        if started {
            if line.starts_with("-----END ") {
                break;
            }
            if line
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '/' || c == '=')
            {
                body.push_str(line);
            }
        }
        match line {
            "-----BEGIN OPENSSH PRIVATE KEY-----" => {
                started = true;
                openssh = true;
            }
            "-----BEGIN RSA PRIVATE KEY-----"
            | "-----BEGIN ENCRYPTED PRIVATE KEY-----"
            | "-----BEGIN PRIVATE KEY-----"
            | "-----BEGIN EC PRIVATE KEY-----" => {
                started = true;
                openssh = false;
            }
            _ => {}
        }
    }
    openssh.then_some(body)
}

/// Decode an OpenSSH key body, decrypting it with `passphrase` when it is
/// protected. The KDF rounds are checked before any key derivation runs.
fn decode_openssh(body: &str, passphrase: Option<&str>) -> Result<PrivateKey> {
    // As lenient as russh's MIME decoder, so no key it reads is refused here.
    const BODY: base64::engine::GeneralPurpose = base64::engine::GeneralPurpose::new(
        &base64::alphabet::STANDARD,
        base64::engine::GeneralPurposeConfig::new()
            .with_decode_allow_trailing_bits(true)
            .with_decode_padding_mode(base64::engine::DecodePaddingMode::Indifferent),
    );
    let bytes = Zeroizing::new(
        BODY.decode(body)
            .map_err(|e| CoreError::Key(format!("parse: {e}")))?,
    );
    let key = PrivateKey::from_bytes(&bytes).map_err(|e| CoreError::Key(format!("parse: {e}")))?;
    if let Kdf::Bcrypt { rounds, .. } = key.kdf() {
        if *rounds > MAX_BCRYPT_ROUNDS {
            return Err(CoreError::InvalidInput(format!(
                "this key's passphrase protection asks for {rounds} bcrypt rounds; \
                 at most {MAX_BCRYPT_ROUNDS} are accepted"
            )));
        }
    }
    match (key.is_encrypted(), passphrase) {
        (false, None) => Ok(key),
        (false, Some(_)) => Err(not_protected()),
        (true, Some(passphrase)) => key
            .decrypt(passphrase)
            .map_err(|e| CoreError::Key(format!("parse: {e}"))),
        (true, None) => Err(CoreError::Key("parse: the key is encrypted".into())),
    }
}

/// Decode a PEM or PuTTY key through russh.
fn decode_other(text: &str, passphrase: Option<&str>) -> Result<PrivateKey> {
    if passphrase.is_some() && decode_secret_key(text, None).is_ok() {
        return Err(not_protected());
    }
    decode_secret_key(text, passphrase).map_err(|e| CoreError::Key(format!("parse: {e}")))
}

fn not_protected() -> CoreError {
    CoreError::Key(
        "parse: this key is not protected by a passphrase; leave the passphrase empty".into(),
    )
}

/// Parse a private key for storage in the vault, returning its metadata and the
/// text to store.
///
/// The vault is the only encryption a stored key has: connecting reads it back
/// without a passphrase, and the key's own passphrase is not kept. A key given
/// with its passphrase is therefore stored with that protection removed; one
/// without a passphrase is stored exactly as given.
///
/// Every decryption repeats the key's KDF, so the key is decrypted once and both
/// the metadata and the stored text come from that one decrypted key.
pub fn import_openssh_private(
    openssh: &str,
    passphrase: Option<&str>,
) -> Result<(KeyMeta, Zeroizing<String>)> {
    let (meta, decrypted) = parse_openssh_private(openssh, passphrase)?;
    // Parsing refuses a passphrase for a key that has none, so a key that
    // parsed with one was decrypted with it.
    let stored = match passphrase {
        None => Zeroizing::new(openssh.to_owned()),
        Some(_) => unprotected_form(openssh, &decrypted)?,
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
/// The private half is wiped when the caller drops it.
pub fn generate_ed25519() -> Result<(Zeroizing<String>, String)> {
    let mut rng = getrandom::rand_core::UnwrapErr(getrandom::SysRng);
    let private = PrivateKey::random(&mut rng, Algorithm::Ed25519)
        .map_err(|e| CoreError::Key(format!("generate: {e}")))?;
    let pem = private
        .to_openssh(ssh_key::LineEnding::LF)
        .map_err(|e| CoreError::Key(format!("serialize: {e}")))?;
    Ok((pem, private.public_key().public_key_base64()))
}

/// The text to store for a key that carried its own passphrase, written from
/// the already decrypted key.
///
/// The vault is what protects stored keys, and the connect path reads a stored
/// key without a passphrase, so a key kept in its encrypted form would import
/// without complaint and could then never be used. An OpenSSH key is written
/// back in OpenSSH form; a PEM or PuTTY key is written as unencrypted PKCS#8.
fn unprotected_form(original: &str, decrypted: &PrivateKey) -> Result<Zeroizing<String>> {
    if openssh_body(original).is_some() {
        return decrypted
            .to_openssh(ssh_key::LineEnding::LF)
            .map_err(|e| CoreError::Key(format!("serialize: {e}")));
    }
    let mut pem = Zeroizing::new(Vec::new());
    russh::keys::encode_pkcs8_pem(decrypted, &mut *pem)
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

    /// A vault can hold a key in its passphrase-protected form, so its identity
    /// has to be readable without the passphrase. The public half sits outside
    /// the encrypted section, so it is.
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

    /// `protected` with its bcrypt round count replaced. The count sits outside
    /// the encrypted section and nothing authenticates it, so the result still
    /// parses as a key.
    fn with_rounds(protected: &str, rounds: u32) -> String {
        let key = PrivateKey::from_openssh(protected).expect("read");
        let Kdf::Bcrypt { salt, .. } = key.kdf() else {
            panic!("not a bcrypt-protected key");
        };
        let mut bytes = key.to_bytes().expect("encode").to_vec();
        let at = bytes
            .windows(salt.len())
            .position(|window| window == salt.as_slice())
            .expect("salt")
            + salt.len();
        bytes[at..at + 4].copy_from_slice(&rounds.to_be_bytes());
        PrivateKey::from_bytes(&bytes)
            .expect("reparse")
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

        let (imported, stored) =
            import_openssh_private(&protected, Some("key passphrase")).expect("import");
        assert_eq!(imported.fingerprint, meta.fingerprint);
        let (stored_meta, _) = parse_openssh_private(&stored, None).expect("usable without passphrase");
        assert_eq!(stored_meta.fingerprint, meta.fingerprint);

        assert!(import_openssh_private(&protected, Some("wrong")).is_err());
    }

    /// Storing a key still encrypted would file a key the connect path can
    /// never open, so a missing passphrase is an error rather than stored text.
    #[test]
    fn a_protected_key_without_its_passphrase_is_an_error_not_stored() {
        let (plain, _) = generate_ed25519().expect("generate");
        let protected = protected_openssh(&plain, "key passphrase");
        assert!(import_openssh_private(&protected, None).is_err());
        assert!(import_openssh_private(&protected, Some("")).is_err());
    }

    #[test]
    fn a_protected_pem_key_is_stored_as_plain_pkcs8() {
        let (plain, _) = generate_ed25519().expect("generate");
        let (meta, pair) = parse_openssh_private(&plain, None).expect("parse");
        let mut protected = Vec::new();
        russh::keys::encode_pkcs8_pem_encrypted(&pair, b"key passphrase", 16, &mut protected)
            .expect("encrypt pem");
        let protected = String::from_utf8(protected).expect("utf8");
        assert!(import_openssh_private(&protected, None).is_err());

        let (_, stored) =
            import_openssh_private(&protected, Some("key passphrase")).expect("import");
        assert!(stored.contains("BEGIN PRIVATE KEY"), "{}", &*stored);
        let (stored_meta, _) = parse_openssh_private(&stored, None).expect("usable without passphrase");
        assert_eq!(stored_meta.fingerprint, meta.fingerprint);
    }

    #[test]
    fn an_unprotected_key_is_stored_as_given() {
        let (plain, _) = generate_ed25519().expect("generate");
        assert_eq!(
            *import_openssh_private(&plain, None).expect("none").1,
            *plain
        );
        assert!(import_openssh_private(&plain, Some("unused")).is_err());
    }

    /// The round count comes from the file, and one large enough never
    /// finishes, so it is refused before any key derivation runs — including
    /// for a key behind leading text, which russh reads as a key all the same.
    #[test]
    fn a_key_asking_for_too_many_kdf_rounds_is_refused_without_deriving() {
        let (plain, _) = generate_ed25519().expect("generate");
        let crafted = with_rounds(&protected_openssh(&plain, "key passphrase"), 1 << 31);
        let behind_text = format!("exported key\n{crafted}");

        let (done, results) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for text in [crafted, behind_text] {
                for passphrase in [Some("key passphrase"), None] {
                    let result = import_openssh_private(&text, passphrase).map(|_| ());
                    let _ = done.send(result);
                }
            }
        });
        for _ in 0..4 {
            let result = results
                .recv_timeout(std::time::Duration::from_secs(2))
                .expect("the import ran the KDF instead of refusing the key");
            assert!(
                matches!(result, Err(CoreError::InvalidInput(_))),
                "{result:?}"
            );
        }
    }

    /// Every decryption repeats the KDF, so an import that decrypted the key
    /// more than once would take a multiple of one decryption.
    #[test]
    fn importing_a_protected_key_runs_its_kdf_once() {
        let (plain, _) = generate_ed25519().expect("generate");
        let protected = protected_openssh(&plain, "key passphrase");
        let encrypted = PrivateKey::from_openssh(&protected).expect("read");

        // The fastest of several runs, so a busy machine only ever slows a
        // run down and cannot make the import look cheaper than it is.
        let mut one_pass = std::time::Duration::MAX;
        let mut import = std::time::Duration::MAX;
        for _ in 0..3 {
            let started = std::time::Instant::now();
            encrypted.decrypt("key passphrase").expect("decrypt");
            one_pass = one_pass.min(started.elapsed());

            let started = std::time::Instant::now();
            import_openssh_private(&protected, Some("key passphrase")).expect("import");
            import = import.min(started.elapsed());
        }
        assert!(
            import.as_secs_f64() < one_pass.as_secs_f64() * 1.5,
            "import took {import:?} against {one_pass:?} for one decryption"
        );
    }

    #[test]
    fn text_that_is_not_a_key_reports_no_public_identity() {
        assert!(public_identity("not an openssh private key").is_err());
    }
}
