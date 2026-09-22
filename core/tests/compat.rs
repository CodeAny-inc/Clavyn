//! On-disk compatibility with state files written by earlier builds.
//!
//! The files under `tests/fixtures/compat` were produced by a released build
//! and are never regenerated: they stand in for what existing users already
//! have on disk. The vault holds three throwaway test keys (Ed25519, RSA and
//! ECDSA), and the known hosts file pins the public halves of the same keys.

use clavyn_core::keys::{fingerprint, parse_openssh_private, KeyMeta, KeyType};
use clavyn_core::known_hosts::KnownHosts;
use clavyn_core::vault::Vault;
use clavyn_core::CoreError;
use russh::keys::parse_public_key_base64;
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
        let openssh = std::str::from_utf8(&stored).expect("stored key is text");
        let (parsed, _) = parse_openssh_private(openssh, None)
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

/// Public halves of the vault fixture's keys, which is what the known hosts
/// fixture pins: `host0` is the Ed25519 key, `host1` RSA and `host2` ECDSA.
fn pinned_keys() -> Vec<(String, KeyMeta)> {
    let (_dir, path) = copy_fixture("vault-v1.json", "vault.json");
    let vault = Vault::open(path).expect("open vault fixture");
    let order = [KeyType::Ed25519, KeyType::Rsa, KeyType::Ecdsa];
    order
        .iter()
        .enumerate()
        .map(|(i, key_type)| {
            let meta = vault
                .keys_meta()
                .iter()
                .find(|m| m.key_type == *key_type)
                .expect("key in fixture")
                .clone();
            (format!("host{i}.example.com"), meta)
        })
        .collect()
}

#[test]
fn existing_host_key_pins_still_match_the_same_keys() {
    let (_dir, path) = copy_fixture("known_hosts-v1.json", "known_hosts.json");
    let before = std::fs::read(&path).expect("read pins");
    let mut known_hosts = KnownHosts::load(path.clone()).expect("load pins");

    for (host, meta) in pinned_keys() {
        let key = parse_public_key_base64(&meta.public_key_base64).expect("public key");
        assert_eq!(fingerprint(&key), meta.fingerprint, "{host}");
        known_hosts
            .check_mismatch(&host, 22, &key)
            .unwrap_or_else(|e| panic!("{host} no longer matches its pin: {e}"));
        assert!(known_hosts.verify(&host, 22, &key).expect("verify"), "{host}");
    }

    // A matching pin is never rewritten.
    assert_eq!(std::fs::read(&path).expect("read pins"), before);
}

#[test]
fn existing_host_key_pins_still_reject_a_different_key() {
    let (_dir, path) = copy_fixture("known_hosts-v1.json", "known_hosts.json");
    let known_hosts = KnownHosts::load(path).expect("load pins");
    let keys = pinned_keys();
    let (ed25519_host, ed25519_meta) = &keys[0];
    let (_, rsa_meta) = &keys[1];
    let wrong = parse_public_key_base64(&rsa_meta.public_key_base64).expect("public key");

    match known_hosts.check_mismatch(ed25519_host, 22, &wrong) {
        Err(CoreError::HostKeyMismatch { pinned, presented, .. }) => {
            assert_eq!(pinned, ed25519_meta.fingerprint);
            assert_eq!(presented, rsa_meta.fingerprint);
        }
        other => panic!("expected a host key mismatch, got {other:?}"),
    }
}
