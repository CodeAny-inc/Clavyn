use crate::host::{AuthMethod, Host};
use crate::identity::Identity;
use crate::known_hosts::KnownHosts;
use crate::vault::Vault;
use crate::{CoreError, Result};
use russh::client::{self, Config, Handle};
use russh::keys::key;
use russh_keys::decode_secret_key;
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tokio::sync::Mutex;

/// Upper bound on opening the transport and on completing authentication.
/// A peer that accepts the TCP connection and then stops responding otherwise
/// leaves both futures pending forever.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);

/// Client defaults shared by every session.
///
/// russh's own defaults leave `keepalive_interval` and `inactivity_timeout`
/// unset, so a peer that disappears without closing the connection — a dropped
/// VPN, a suspended laptop — is never detected and the session hangs instead of
/// reporting a close. With an interval set, russh gives up after
/// `keepalive_max` (3) unanswered probes, so a dead peer surfaces in about two
/// minutes. The inactivity bound is only a backstop: any byte from the server,
/// including its reply to a keepalive, restarts that timer, so an idle but live
/// terminal is never torn down under a user who stepped away.
fn client_config() -> Arc<Config> {
    static CONFIG: OnceLock<Arc<Config>> = OnceLock::new();
    CONFIG
        .get_or_init(|| {
            let mut config = Config::default();
            config.keepalive_interval = Some(Duration::from_secs(30));
            config.inactivity_timeout = Some(Duration::from_secs(3600));
            Arc::new(config)
        })
        .clone()
}

fn timed_out(stage: &str, addr: &str) -> CoreError {
    CoreError::Ssh(format!(
        "{stage} {addr}: timed out after {}s",
        CONNECT_TIMEOUT.as_secs()
    ))
}

/// Handler that verifies the server key against the known_hosts store (TOFU).
/// On first sight: record + accept. On match: accept. On mismatch: reject.
pub struct SshHandler {
    host: String,
    port: u16,
    known_hosts: Arc<Mutex<KnownHosts>>,
}

#[async_trait::async_trait]
impl client::Handler for SshHandler {
    type Error = CoreError;

    async fn check_server_key(
        &mut self,
        server_public_key: &key::PublicKey,
    ) -> std::result::Result<bool, Self::Error> {
        let mut kh = self.known_hosts.lock().await;
        let trusted = kh.verify(&self.host, self.port, server_public_key)?;
        if !trusted {
            tracing::warn!(
                "host key mismatch for {}:{} — rejecting",
                self.host,
                self.port
            );
        }
        Ok(trusted)
    }
}

/// Open an authenticated SSH session and return the handle.
/// The caller is responsible for opening a channel and starting the shell.
///
/// If the host has an `identity_id`, the identity must resolve and is used to
/// resolve the username, auth method, and key — overriding the host's own
/// fields. A broken linked-identity reference fails closed instead of silently
/// falling back to stale host credentials.
pub async fn connect(
    host: &Host,
    identity: Option<&Identity>,
    known_hosts: Arc<Mutex<KnownHosts>>,
    vault: Option<&Vault>,
    passphrase: Option<&str>,
    password: Option<&str>,
) -> Result<Handle<SshHandler>> {
    if host.identity_id.is_some() && identity.is_none() {
        return Err(CoreError::InvalidInput(
            "linked SSH identity not found; repair the host configuration before reconnecting".into(),
        ));
    }

    // Resolve effective username, auth, and key from identity if present.
    let (username, auth, key_id) = match identity {
        Some(id) => (&id.username, &id.auth, id.key_id),
        None => (&host.username, &host.auth, host.key_id),
    };

    // Agent used to masquerade as empty-password auth here, which could make
    // mocked UI tests look successful while native SSH always behaved differently.
    // Until a real agent transport/signer is implemented for every supported OS,
    // reject it explicitly before opening a network connection.
    if matches!(auth, AuthMethod::Agent) {
        return Err(CoreError::InvalidInput(
            "SSH agent authentication is not supported yet; choose Password or SSH Key".into(),
        ));
    }

    let handler = SshHandler {
        host: host.hostname.clone(),
        port: host.port,
        known_hosts,
    };

    let addr = format!("{}:{}", host.hostname, host.port);
    let mut session = tokio::time::timeout(
        CONNECT_TIMEOUT,
        client::connect(client_config(), &addr, handler),
    )
    .await
    .map_err(|_| timed_out("connect", &addr))?
    .map_err(|e| CoreError::Ssh(format!("connect {addr}: {e}")))?;

    // Authentication is bounded separately, and the two bounds cover disjoint
    // stages. `client::connect` does not return until russh has read the
    // server's banner and the key exchange has completed, so a peer that
    // answers the TCP handshake and then goes silent trips the connect timeout
    // above and never reaches this point. What is left for this bound is a
    // server that finishes the key exchange and then stalls while answering an
    // authentication request. Neither timeout subsumes the other.
    let auth_ok = tokio::time::timeout(CONNECT_TIMEOUT, async {
        let outcome = match auth {
            AuthMethod::Agent => {
                unreachable!("agent auth is rejected before network connection")
            }
            AuthMethod::Password { .. } => {
                let pw = password.ok_or_else(|| {
                    CoreError::InvalidInput("password required but not provided".into())
                })?;
                session.authenticate_password(username, pw).await
            }
            AuthMethod::PublicKey => {
                let key_id = key_id.ok_or_else(|| {
                    CoreError::InvalidInput("publickey auth but no key_id set".into())
                })?;
                let passphrase = passphrase.ok_or_else(|| {
                    CoreError::InvalidInput("vault passphrase required for key auth".into())
                })?;
                let vault = vault.ok_or_else(|| {
                    CoreError::InvalidInput("vault required for key auth".into())
                })?;
                let private_pem = vault.get_key(passphrase, &key_id.to_string()).await?;
                let pem_str = String::from_utf8(private_pem)
                    .map_err(|e| CoreError::Key(format!("utf8: {e}")))?;
                let pair = decode_secret_key(&pem_str, None)
                    .map_err(|e| CoreError::Key(format!("decode: {e}")))?;
                session
                    .authenticate_publickey(username, Arc::new(pair))
                    .await
            }
        };
        outcome.map_err(|e| CoreError::Ssh(format!("auth: {e}")))
    })
    .await
    .map_err(|_| timed_out("auth", &addr))??;

    if !auth_ok {
        return Err(CoreError::Ssh("authentication rejected by server".into()));
    }
    Ok(session)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn known_hosts() -> Arc<Mutex<KnownHosts>> {
        let path = std::env::temp_dir().join(format!(
            "clavyn-connection-test-{}.json",
            uuid::Uuid::new_v4()
        ));
        Arc::new(Mutex::new(KnownHosts::load(path).unwrap()))
    }

    fn host(auth: serde_json::Value, identity_id: Option<&str>) -> Host {
        serde_json::from_value(serde_json::json!({
            "id": "00000000-0000-0000-0000-000000000001",
            "label": "Fixture",
            "hostname": "must-not-connect.example.test",
            "port": 22,
            "username": "deploy",
            "auth": auth,
            "identity_id": identity_id,
            "tags": []
        }))
        .unwrap()
    }

    #[tokio::test]
    async fn missing_linked_identity_fails_before_network_fallback() {
        let host = host(
            serde_json::json!({"password": {"credential_key": "metadata-only"}}),
            Some("00000000-0000-0000-0000-000000000002"),
        );
        let error = match connect(&host, None, known_hosts(), None, None, Some("SECRET")).await {
            Ok(_) => panic!("missing linked identity unexpectedly reached a successful SSH connection"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("linked SSH identity not found"));
    }

    #[tokio::test]
    async fn agent_auth_fails_explicitly_before_network_connection() {
        let host = host(serde_json::json!("agent"), None);
        let error = match connect(&host, None, known_hosts(), None, None, None).await {
            Ok(_) => panic!("unsupported SSH Agent auth unexpectedly reached a successful connection"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("SSH agent authentication is not supported yet"));
    }
}
