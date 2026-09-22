//! On-disk compatibility with state files written by earlier builds.
//!
//! The files under `tests/fixtures/compat` were produced by a released build
//! and are never regenerated: they stand in for what existing users already
//! have on disk. The vault holds three throwaway test keys (Ed25519, RSA and
//! ECDSA), and the known hosts file pins the public halves of the same keys.

use clavyn_core::keys::{parse_openssh_private, KeyType};
use clavyn_core::vault::Vault;
use std::path::PathBuf;
use tempfile::TempDir;

const PASSPHRASE: &str = "compat-fixture-passphrase";

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/compat")
}

/// Copies a fixture into a temp dir so no test can rewrite the checked-in file.
fn copy_fixture(name: &str, as_name: &str) -> (TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("create temp dir");
    let path = dir.path().join(as_name);
    std::fs::copy(fixture_dir().join(name), &path).expect("copy fixture");
    (dir, path)
}

#[tokio::test]
async fn existing_vault_unlocks_and_its_keys_still_match_their_metadata() {
    let (_dir, path) = copy_fixture("vault-v1.json", "vault.json");
    let vault = Vault::open(path).expect("open vault fixture");
    let key = vault
        .verify_passphrase(PASSPHRASE)
        .await
        .expect("existing vault must unlock with its passphrase");

    let mut types: Vec<KeyType> = vault.keys_meta().iter().map(|m| m.key_type).collect();
    types.sort_by_key(|t| format!("{t:?}"));
    assert_eq!(types, [KeyType::Ecdsa, KeyType::Ed25519, KeyType::Rsa]);

    for meta in vault.keys_meta() {
        let stored = vault
            .get_key_with(&key, &meta.id.to_string())
            .expect("read stored key");
        let openssh = String::from_utf8(stored).expect("stored key is text");
        let (parsed, _) = parse_openssh_private(&openssh, None)
            .unwrap_or_else(|e| panic!("{} no longer parses: {e}", meta.label));
        // Fingerprints and public keys stored by earlier builds are what users
        // compare against servers, so they must come out byte-identical.
        assert_eq!(parsed.key_type, meta.key_type, "{}", meta.label);
        assert_eq!(parsed.fingerprint, meta.fingerprint, "{}", meta.label);
        assert_eq!(parsed.public_key_base64, meta.public_key_base64, "{}", meta.label);
    }
}

#[tokio::test]
async fn existing_vault_rejects_a_wrong_passphrase() {
    let (_dir, path) = copy_fixture("vault-v1.json", "vault.json");
    let vault = Vault::open(path).expect("open vault fixture");
    assert!(vault.verify_passphrase("not the passphrase").await.is_err());
}
