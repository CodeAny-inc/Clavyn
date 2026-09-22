//! End-to-end SSH against an in-process russh server on loopback.
//!
//! These drive `connection::connect` exactly as the app does — key exchange,
//! host key pinning, and password and vault-backed public key authentication —
//! against a real server, so a transport regression fails here rather than on a
//! user's first connection.

use clavyn_core::connection::connect;
use clavyn_core::host::Host;
use clavyn_core::keys::{generate_ed25519, parse_openssh_private, KeyType};
use clavyn_core::known_hosts::KnownHosts;
use clavyn_core::vault::Vault;
use clavyn_core::CoreError;
use russh::keys::{parse_public_key_base64, Algorithm, PrivateKey, PublicKey};
use russh::server::{self, Auth, ChannelOpenHandle, Msg, Session};
use russh::{Channel, ChannelId, ChannelMsg};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio::sync::Mutex;

const USER: &str = "deploy";
const PASSWORD: &str = "correct password";
const VAULT_PASSPHRASE: &str = "vault passphrase";

#[derive(Clone)]
struct TestServer {
    authorized_key: Option<PublicKey>,
}

impl server::Handler for TestServer {
    type Error = russh::Error;

    async fn auth_password(&mut self, user: &str, password: &str) -> Result<Auth, Self::Error> {
        Ok(if user == USER && password == PASSWORD {
            Auth::Accept
        } else {
            Auth::reject()
        })
    }

    async fn auth_publickey(&mut self, user: &str, key: &PublicKey) -> Result<Auth, Self::Error> {
        let authorized = self
            .authorized_key
            .as_ref()
            .is_some_and(|k| k.key_data() == key.key_data());
        Ok(if user == USER && authorized {
            Auth::Accept
        } else {
            Auth::reject()
        })
    }

    async fn channel_open_session(
        &mut self,
        _channel: Channel<Msg>,
        reply: ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        reply.accept().await;
        Ok(())
    }

    async fn data(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        session.data(channel, data.to_vec())?;
        Ok(())
    }
}

fn random_host_key() -> PrivateKey {
    let mut rng = getrandom::rand_core::UnwrapErr(getrandom::SysRng);
    PrivateKey::random(&mut rng, Algorithm::Ed25519).expect("generate host key")
}

/// Serves every connection on a fresh loopback port and returns that port.
async fn start_server(host_key: PrivateKey, handler: TestServer) -> u16 {
    let config = Arc::new(server::Config {
        keys: vec![host_key],
        auth_rejection_time: Duration::from_millis(10),
        auth_rejection_time_initial: Some(Duration::ZERO),
        ..Default::default()
    });
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("addr").port();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let config = config.clone();
            let handler = handler.clone();
            tokio::spawn(async move {
                if let Ok(running) = server::run_stream(config, stream, handler).await {
                    let _ = running.await;
                }
            });
        }
    });
    port
}

fn host(port: u16, auth: serde_json::Value, key_id: Option<String>) -> Host {
    serde_json::from_value(serde_json::json!({
        "id": "00000000-0000-0000-0000-000000000001",
        "label": "Loopback",
        "hostname": "127.0.0.1",
        "port": port,
        "username": USER,
        "auth": auth,
        "key_id": key_id,
        "tags": []
    }))
    .expect("host")
}

fn password_host(port: u16) -> Host {
    host(
        port,
        serde_json::json!({"password": {"credential_key": "unused"}}),
        None,
    )
}

fn known_hosts(dir: &TempDir) -> Arc<Mutex<KnownHosts>> {
    Arc::new(Mutex::new(
        KnownHosts::load(dir.path().join("known_hosts.json")).expect("load known hosts"),
    ))
}

#[tokio::test]
async fn password_auth_connects_and_pins_the_host_key() {
    let dir = tempfile::tempdir().expect("temp dir");
    let host_key = random_host_key();
    let host_public = host_key.public_key().clone();
    let port = start_server(host_key, TestServer { authorized_key: None }).await;
    let pins = known_hosts(&dir);

    let session = connect(&password_host(port), None, pins.clone(), None, None, Some(PASSWORD))
        .await
        .expect("password auth");

    // The session carries data both ways.
    let mut channel = session.channel_open_session().await.expect("open channel");
    session
        .data(channel.id(), b"ping".to_vec())
        .await
        .expect("send data");
    let echoed = loop {
        match tokio::time::timeout(Duration::from_secs(10), channel.wait())
            .await
            .expect("echo timed out")
        {
            Some(ChannelMsg::Data { data }) => break data.to_vec(),
            Some(_) => continue,
            None => panic!("channel closed before the echo arrived"),
        }
    };
    assert_eq!(echoed, b"ping");

    // First use recorded the server's key, and the same key is accepted again.
    let pins = pins.lock().await;
    pins.check_mismatch("127.0.0.1", port, &host_public)
        .expect("pinned key matches");
    assert_eq!(pins.list().len(), 1);
}

#[tokio::test]
async fn a_wrong_password_is_rejected() {
    let dir = tempfile::tempdir().expect("temp dir");
    let port = start_server(random_host_key(), TestServer { authorized_key: None }).await;

    let error = match connect(
        &password_host(port),
        None,
        known_hosts(&dir),
        None,
        None,
        Some("wrong password"),
    )
    .await
    {
        Ok(_) => panic!("a wrong password was accepted"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("authentication rejected"), "{error}");
}

#[tokio::test]
async fn a_changed_host_key_is_refused_before_authentication() {
    let dir = tempfile::tempdir().expect("temp dir");
    let port = start_server(random_host_key(), TestServer { authorized_key: None }).await;
    let pins = known_hosts(&dir);
    let previous = random_host_key().public_key().clone();
    pins.lock()
        .await
        .verify("127.0.0.1", port, &previous)
        .expect("pin previous key");

    let error = match connect(&password_host(port), None, pins.clone(), None, None, Some(PASSWORD))
        .await
    {
        Ok(_) => panic!("a changed host key was accepted"),
        Err(error) => error,
    };
    assert!(
        matches!(error, CoreError::HostKeyMismatch { .. }),
        "expected a host key mismatch, got {error}"
    );
    // The pin is untouched and the presented key is held for review.
    let pins = pins.lock().await;
    pins.check_mismatch("127.0.0.1", port, &previous)
        .expect("original pin kept");
    assert_eq!(pins.pending_changes().len(), 1);
}

async fn assert_key_auth(vault: &Vault, passphrase: &str, key_id: String, public: PublicKey) {
    let dir = tempfile::tempdir().expect("temp dir");
    let port = start_server(
        random_host_key(),
        TestServer {
            authorized_key: Some(public),
        },
    )
    .await;
    let host = host(port, serde_json::json!("publickey"), Some(key_id));
    connect(&host, None, known_hosts(&dir), Some(vault), Some(passphrase), None)
        .await
        .expect("public key auth");
}

#[tokio::test]
async fn ed25519_key_from_the_vault_authenticates() {
    let dir = tempfile::tempdir().expect("temp dir");
    let mut vault = Vault::open(dir.path().join("vault.json")).expect("open vault");
    let key = vault.initialize(VAULT_PASSPHRASE).await.expect("init vault");
    let (private, public_base64) = generate_ed25519().expect("generate");
    let (meta, _) = parse_openssh_private(&private, None).expect("parse");
    let key_id = meta.id.to_string();
    vault.add_key(&key, meta, &private).expect("add key");

    let public = parse_public_key_base64(&public_base64).expect("public key");
    assert_key_auth(&vault, VAULT_PASSPHRASE, key_id, public).await;
}

/// RSA signs with a SHA-2 hash picked from the server's `server-sig-algs`, a
/// path no other key type takes. The key comes from the compatibility fixture,
/// so this also covers an RSA key stored by an earlier build.
#[tokio::test]
async fn rsa_key_from_an_existing_vault_authenticates() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("vault.json");
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/compat/vault-v1.json");
    std::fs::copy(fixture, &path).expect("copy fixture");
    let vault = Vault::open(path).expect("open vault");
    let meta = vault
        .keys_meta()
        .iter()
        .find(|m| m.key_type == KeyType::Rsa)
        .expect("rsa key in fixture")
        .clone();

    let public = parse_public_key_base64(&meta.public_key_base64).expect("public key");
    assert_key_auth(&vault, "compat-fixture-passphrase", meta.id.to_string(), public).await;
}
