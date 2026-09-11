use crate::host::{AuthMethod, Host};
use crate::identity::Identity;
use crate::known_hosts::KnownHosts;
use crate::vault::Vault;
use crate::{CoreError, Result};
use russh::client::{self, Config, Handle};
use russh::keys::key;
use russh_keys::decode_secret_key;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Handler that verifies the server key against the known_hosts store (TOFU).
/// On first sight: record + accept. On match: accept. On mismatch: reject with
/// `CoreError::HostKeyMismatch`, which russh propagates out of `connect` as the
/// handler's own error type instead of the generic "unknown key" failure.
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
        if let Err(mismatch) = kh.check_mismatch(&self.host, self.port, server_public_key) {
            // Hold the key so the user can accept this exact one after comparing
            // fingerprints; unpinning the host would trust whatever answers next.
            kh.hold_presented_key(&self.host, self.port, server_public_key);
            tracing::warn!(
                "host key mismatch for {}:{} — rejecting",
                self.host,
                self.port
            );
            return Err(mismatch);
        }
        kh.verify(&self.host, self.port, server_public_key)
    }
}

/// Extra context for a key-exchange failure, which russh surfaces as the bare
/// string "No common algorithm" with no hint at which algorithm list failed.
///
/// Clavyn builds russh without its `flate2` feature, so the client offers only
/// `none` for compression. RFC 4253 section 6.2 makes `none` REQUIRED of every
/// implementation, so a server that will not accept it is non-conforming — say
/// so, otherwise the failure reads as a Clavyn bug.
fn negotiation_hint(err: &CoreError) -> &'static str {
    match err {
        CoreError::SshProtocol(russh::Error::NoCommonAlgo {
            kind: russh::AlgorithmKind::Compression,
            ..
        }) => {
            " (compression: this server refuses the `none` compression that RFC 4253 \
             section 6.2 requires every SSH implementation to support, and Clavyn offers \
             nothing else)"
        }
        _ => "",
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

    let config = Arc::new(Config::default());
    let handler = SshHandler {
        host: host.hostname.clone(),
        port: host.port,
        known_hosts,
    };

    let addr = format!("{}:{}", host.hostname, host.port);
    // A changed host key keeps its own error variant: wrapping it in a generic
    // connect failure would hide the two fingerprints that make the change
    // legible.
    let mut session = client::connect(config, &addr, handler)
        .await
        .map_err(|e| match e {
            CoreError::HostKeyMismatch { .. } => e,
            other => CoreError::Ssh(format!(
                "connect {addr}: {other}{}",
                negotiation_hint(&other)
            )),
        })?;

    let auth_ok = match auth {
        AuthMethod::Agent => unreachable!("agent auth is rejected before network connection"),
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
            let private_pem = vault.get_key(passphrase, &key_id.to_string())?;
            let pem_str = String::from_utf8(private_pem)
                .map_err(|e| CoreError::Key(format!("utf8: {e}")))?;
            let pair = decode_secret_key(&pem_str, None)
                .map_err(|e| CoreError::Key(format!("decode: {e}")))?;
            session
                .authenticate_publickey(username, Arc::new(pair))
                .await
        }
    }
    .map_err(|e| CoreError::Ssh(format!("auth: {e}")))?;

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

    fn no_common_algo(kind: russh::AlgorithmKind) -> CoreError {
        CoreError::SshProtocol(russh::Error::NoCommonAlgo {
            kind,
            ours: vec!["none".into()],
            theirs: vec!["zlib".into()],
        })
    }

    #[test]
    fn compression_mismatch_is_explained_as_compression() {
        let hint = negotiation_hint(&no_common_algo(russh::AlgorithmKind::Compression));
        assert!(hint.contains("compression"));
        assert!(hint.contains("RFC 4253"));
    }

    #[test]
    fn other_algorithm_mismatches_are_left_alone() {
        let cipher = no_common_algo(russh::AlgorithmKind::Cipher);
        assert_eq!(negotiation_hint(&cipher), "");
        assert_eq!(negotiation_hint(&CoreError::Ssh("plain".into())), "");
    }
}
