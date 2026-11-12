//! An in-process russh server on loopback, and the client-side pieces a test
//! needs to connect to it the way the app does.

use clavyn_core::host::Host;
use clavyn_core::known_hosts::KnownHosts;
use russh::keys::{Algorithm, PrivateKey};
use russh::server;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio::sync::Mutex;

/// The account a loopback server's password check accepts.
pub const USER: &str = "deploy";
pub const PASSWORD: &str = "correct password";

/// A fresh Ed25519 host key.
pub fn random_host_key() -> PrivateKey {
    let mut rng = getrandom::rand_core::UnwrapErr(getrandom::SysRng);
    PrivateKey::random(&mut rng, Algorithm::Ed25519).expect("generate host key")
}

/// Server settings for `host_key`. A rejected authentication is answered at
/// once rather than after russh's usual delay, so tests that expect one do not
/// wait it out.
pub fn server_config(host_key: PrivateKey) -> server::Config {
    server::Config {
        keys: vec![host_key],
        auth_rejection_time: Duration::from_millis(10),
        auth_rejection_time_initial: Some(Duration::ZERO),
        ..Default::default()
    }
}

/// Serves every connection on a fresh loopback port with a copy of `handler`
/// and returns that port.
pub async fn serve<H>(config: server::Config, handler: H) -> u16
where
    H: server::Handler + Clone + Send + 'static,
{
    let config = Arc::new(config);
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

/// A saved host pointing at the loopback server on `port`, signing in as
/// [`USER`] with a password.
pub fn password_host(port: u16) -> Host {
    serde_json::from_value(serde_json::json!({
        "id": "00000000-0000-0000-0000-000000000001",
        "label": "Loopback",
        "hostname": "127.0.0.1",
        "port": port,
        "username": USER,
        "auth": {"password": {"credential_key": "unused"}},
        "tags": []
    }))
    .expect("host")
}

/// An empty known-hosts store in `dir`, so the first connection pins the
/// loopback server's key.
pub fn known_hosts(dir: &TempDir) -> Arc<Mutex<KnownHosts>> {
    Arc::new(Mutex::new(
        KnownHosts::load(dir.path().join("known_hosts.json")).expect("load known hosts"),
    ))
}
