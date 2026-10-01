//! Fixture access shared by the integration test crates.
//!
//! The files under `tests/fixtures/compat` were produced by a released build
//! and are never regenerated: they stand in for what existing users already
//! have on disk. The vault holds three throwaway test keys (Ed25519, RSA and
//! ECDSA), and the known hosts file pins the public halves of the same keys.

// Every test crate compiles this module on its own and uses only part of it.
#![allow(dead_code)]

use clavyn_core::keys::{generate_ed25519, parse_openssh_private};
use russh::keys::ssh_key::{LineEnding, PrivateKey};
use std::path::PathBuf;
use tempfile::TempDir;

/// Passphrase of the fixture vault.
pub const PASSPHRASE: &str = "compat-fixture-passphrase";

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/compat")
}

/// Copies a fixture into a temp dir so no test can rewrite the checked-in file.
pub fn copy_fixture(name: &str, as_name: &str) -> (TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("create temp dir");
    let path = dir.path().join(as_name);
    std::fs::copy(fixture_dir().join(name), &path).expect("copy fixture");
    (dir, path)
}

/// A fresh Ed25519 key encrypted with `passphrase`, and the fingerprint of the
/// key inside it.
pub fn passphrase_protected_ed25519(passphrase: &str) -> (String, String) {
    let (plain, _) = generate_ed25519().expect("generate key");
    let (meta, _) = parse_openssh_private(&plain, None).expect("parse key");
    let encrypted = PrivateKey::from_openssh(&plain)
        .expect("read key")
        .encrypt(&mut getrandom::SysRng, passphrase)
        .expect("encrypt key")
        .to_openssh(LineEnding::LF)
        .expect("serialize key")
        .to_string();
    (encrypted, meta.fingerprint)
}
