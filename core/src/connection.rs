use crate::host::{AuthMethod, Host};
use crate::identity::Identity;
use crate::known_hosts::KnownHosts;
use crate::vault::Vault;
use crate::{CoreError, Result};
use russh::client::{self, Config, Handle};
use russh::keys::{
    decode_secret_key, Algorithm, EcdsaCurve, HashAlg, PrivateKeyWithHashAlg,
    PublicKeyOrCertificate,
};
use russh::{mac, Preferred};
use std::borrow::Cow;
use std::collections::HashSet;
use std::sync::{Arc, OnceLock, PoisonError};
use std::time::Duration;
use tokio::sync::Mutex;
use zeroize::Zeroizing;

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
///
/// The host-key and MAC lists are Clavyn's own rather than russh's defaults, so
/// which algorithms a connection accepts is decided here and does not move
/// with a dependency upgrade.
fn client_config() -> Arc<Config> {
    static CONFIG: OnceLock<Arc<Config>> = OnceLock::new();
    CONFIG
        .get_or_init(|| {
            let mut config = Config::default();
            config.keepalive_interval = Some(Duration::from_secs(30));
            config.inactivity_timeout = Some(Duration::from_secs(3600));
            config.preferred = Preferred {
                key: Cow::Borrowed(HOST_KEY_ALGORITHMS),
                mac: Cow::Borrowed(MAC_ALGORITHMS),
                ..Preferred::default()
            };
            Arc::new(config)
        })
        .clone()
}

/// Host-key algorithms offered to a server, most preferred first.
///
/// `ssh-rsa` is left out: as an algorithm name it means an RSA signature over
/// SHA-1, so a server that can only prove its identity that way fails key
/// exchange instead of being pinned on a SHA-1 signature. RSA host keys are
/// still accepted through `rsa-sha2-512` and `rsa-sha2-256`.
const HOST_KEY_ALGORITHMS: &[Algorithm] = &[
    Algorithm::Ed25519,
    Algorithm::Ecdsa {
        curve: EcdsaCurve::NistP256,
    },
    Algorithm::Ecdsa {
        curve: EcdsaCurve::NistP384,
    },
    Algorithm::Ecdsa {
        curve: EcdsaCurve::NistP521,
    },
    Algorithm::Rsa {
        hash: Some(HashAlg::Sha512),
    },
    Algorithm::Rsa {
        hash: Some(HashAlg::Sha256),
    },
];

/// MACs offered to a server, most preferred first.
///
/// The HMAC-SHA1 pair comes last, so it is only negotiated with a server that
/// offers no SHA-2 MAC; without it such a server cannot be reached at all. HMAC
/// does not depend on SHA-1's collision resistance, which is what is broken.
/// AEAD ciphers carry their own tag and never use this list.
const MAC_ALGORITHMS: &[mac::Name] = &[
    mac::HMAC_SHA512_ETM,
    mac::HMAC_SHA256_ETM,
    mac::HMAC_SHA512,
    mac::HMAC_SHA256,
    mac::HMAC_SHA1_ETM,
    mac::HMAC_SHA1,
];

fn timed_out(stage: &str, addr: &str) -> CoreError {
    CoreError::Ssh(format!(
        "{stage} {addr}: timed out after {}s",
        CONNECT_TIMEOUT.as_secs()
    ))
}

/// Handler that verifies the server key against the known_hosts store (TOFU).
/// On first sight: record + accept. On match: accept. On mismatch: reject with
/// `CoreError::HostKeyMismatch`, which russh propagates out of `connect` as the
/// handler's own error type instead of the generic "unknown key" failure.
pub struct SshHandler {
    host: String,
    port: u16,
    known_hosts: Arc<Mutex<KnownHosts>>,
    /// The RSA hash of the host-key algorithm negotiated in the current key
    /// exchange, `None` for any other key type. russh hands `check_server_key`
    /// the bare key without it, so it is taken from `kex_done`, which russh
    /// calls first with every negotiated algorithm.
    host_key_hash: Option<HashAlg>,
}

impl client::Handler for SshHandler {
    type Error = CoreError;

    async fn kex_done(
        &mut self,
        _shared_secret: Option<&[u8]>,
        names: &russh::Names,
        _session: &mut client::Session,
    ) -> std::result::Result<(), Self::Error> {
        self.host_key_hash = match names.key {
            Algorithm::Rsa { hash } => hash,
            _ => None,
        };
        Ok(())
    }

    async fn check_server_key(
        &mut self,
        server_key: &PublicKeyOrCertificate,
    ) -> std::result::Result<bool, Self::Error> {
        // The client does not advertise certificate host-key algorithms, so a
        // conforming server only ever presents a plain key. A certificate would
        // need a trusted authority to check it against, and there is none, so
        // it is refused rather than pinned as if it were a bare key. The refusal
        // is an error rather than `Ok(false)`, which russh would report as a
        // bare "unknown server key" with the reason left out.
        let server_public_key = match server_key {
            PublicKeyOrCertificate::PublicKey { key, .. } => key,
            PublicKeyOrCertificate::Certificate(_) => {
                tracing::warn!(
                    "{}:{} presented a host certificate; rejecting",
                    self.host,
                    self.port
                );
                return Err(CoreError::Ssh(format!(
                    "connect {}:{}: the server presented a host certificate, which Clavyn \
                     does not accept because it has no certificate authority to check it \
                     against; the server must offer a plain host key",
                    self.host, self.port
                )));
            }
        };
        let mut kh = self.known_hosts.lock().await;
        if let Err(mismatch) = kh.check_mismatch(&self.host, self.port, server_public_key) {
            // Hold the key so the user can accept this exact one after comparing
            // fingerprints; unpinning the host would trust whatever answers next.
            kh.hold_presented_key(&self.host, self.port, server_public_key, self.host_key_hash);
            tracing::warn!(
                "host key mismatch for {}:{} — rejecting",
                self.host,
                self.port
            );
            return Err(mismatch);
        }
        kh.verify(&self.host, self.port, server_public_key, self.host_key_hash)
    }
}

/// Extra context for a key-exchange failure, which russh surfaces as the bare
/// string "No common algorithm" with no hint at which algorithm list failed.
///
/// Clavyn builds russh without its `flate2` feature, so the client offers only
/// `none` for compression. RFC 4253 section 6.2 makes `none` REQUIRED of every
/// implementation, so a server that will not accept it is non-conforming — say
/// so, otherwise the failure reads as a Clavyn bug.
///
/// A host-key failure is the deliberate refusal of [`HOST_KEY_ALGORITHMS`]: the
/// usual cause is a server that can only sign its host key with `ssh-rsa`.
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
        CoreError::SshProtocol(russh::Error::NoCommonAlgo {
            kind: russh::AlgorithmKind::Key,
            ..
        }) => {
            " (host key: this server offers no host-key algorithm Clavyn accepts; a \
             server limited to `ssh-rsa` signs with SHA-1, which Clavyn refuses, and needs \
             rsa-sha2-256 or rsa-sha2-512 enabled or a newer host key)"
        }
        _ => "",
    }
}

/// Servers, by `host:port`, whose `server-sig-algs` named no RSA hash on an
/// earlier connection in this process, usually because they never send that
/// extension at all.
///
/// russh waits up to a second for the extension before it can say which RSA
/// hash a server takes, and a server that omits it once omits it every time, so
/// later RSA logins to the same server skip the wait and use the same fallback.
fn servers_without_sig_algs() -> &'static std::sync::Mutex<HashSet<String>> {
    static SERVERS: OnceLock<std::sync::Mutex<HashSet<String>>> = OnceLock::new();
    SERVERS.get_or_init(Default::default)
}

/// The hash an RSA key signs with when authenticating to `addr`.
async fn rsa_signing_hash(session: &Handle<SshHandler>, addr: &str) -> Result<HashAlg> {
    let known_silent = servers_without_sig_algs()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .contains(addr);
    if known_silent {
        return Ok(rsa_hash_for(None));
    }
    let advertised = session
        .best_supported_rsa_hash()
        .await
        .map_err(|e| CoreError::Ssh(format!("auth: {e}")))?;
    if advertised.is_none() {
        servers_without_sig_algs()
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(addr.to_string());
    }
    Ok(rsa_hash_for(advertised))
}

/// Picks the RSA signature hash from russh's reading of `server-sig-algs`:
/// `None` when the server sent no usable list, `Some(None)` when the list
/// names only `ssh-rsa`, and `Some(Some(hash))` for the best SHA-2 hash on it.
///
/// Clavyn never signs with SHA-1. Without a SHA-2 entry to go by it offers
/// rsa-sha2-512, which RFC 8332 servers accept even when they omit the
/// extension; a server that verifies only `ssh-rsa` rejects it, and the login
/// fails rather than falling back to SHA-1.
fn rsa_hash_for(advertised: Option<Option<HashAlg>>) -> HashAlg {
    match advertised {
        Some(Some(hash)) => hash,
        Some(None) | None => HashAlg::Sha512,
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
        host_key_hash: None,
    };

    let addr = format!("{}:{}", host.hostname, host.port);
    // A changed host key keeps its own error variant: wrapping it in a generic
    // connect failure would hide the two fingerprints that make the change
    // legible. `CoreError::Ssh` only reaches here from `check_server_key`, which
    // already names the address, so it is passed through unwrapped too.
    let mut session = tokio::time::timeout(
        CONNECT_TIMEOUT,
        client::connect(client_config(), &addr, handler),
    )
    .await
    .map_err(|_| timed_out("connect", &addr))?
    .map_err(|e| match e {
        CoreError::HostKeyMismatch { .. } | CoreError::Ssh(_) => e,
        other => CoreError::Ssh(format!(
            "connect {addr}: {other}{}",
            negotiation_hint(&other)
        )),
    })?;

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
                // `get_key` leaves wiping the decrypted key to the caller, and
                // it is read in place so no second copy is made.
                let private = Zeroizing::new(vault.get_key(passphrase, &key_id.to_string()).await?);
                let openssh = std::str::from_utf8(&private)
                    .map_err(|e| CoreError::Key(format!("utf8: {e}")))?;
                let pair = decode_secret_key(openssh, None).map_err(|e| match e {
                    // Import stores a key decrypted, so only a key filed with
                    // its own passphrase still in place lands here.
                    russh::keys::Error::KeyIsEncrypted => CoreError::Key(
                        "this key is still protected by its own passphrase, which Clavyn \
                         does not keep; delete it and import it again with that passphrase"
                            .into(),
                    ),
                    other => CoreError::Key(format!("decode: {other}")),
                })?;
                // Only RSA takes a hash; every other key type ignores it.
                let hash_alg = if pair.algorithm().is_rsa() {
                    Some(rsa_signing_hash(&session, &addr).await?)
                } else {
                    None
                };
                session
                    .authenticate_publickey(
                        username,
                        PrivateKeyWithHashAlg::new(Arc::new(pair), hash_alg),
                    )
                    .await
            }
        };
        outcome.map_err(|e| CoreError::Ssh(format!("auth: {e}")))
    })
    .await
    .map_err(|_| timed_out("auth", &addr))??;

    if !auth_ok.success() {
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
    fn host_key_mismatch_is_explained_as_the_sha1_refusal() {
        let hint = negotiation_hint(&no_common_algo(russh::AlgorithmKind::Key));
        assert!(hint.contains("host key"), "{hint}");
        assert!(hint.contains("ssh-rsa"), "{hint}");
    }

    #[test]
    fn other_algorithm_mismatches_are_left_alone() {
        let cipher = no_common_algo(russh::AlgorithmKind::Cipher);
        assert_eq!(negotiation_hint(&cipher), "");
        assert_eq!(negotiation_hint(&CoreError::Ssh("plain".into())), "");
    }

    #[test]
    fn host_keys_are_never_negotiated_with_a_sha1_signature() {
        let offered = &client_config().preferred.key;
        assert!(
            !offered.contains(&Algorithm::Rsa { hash: None }),
            "{offered:?}"
        );
        assert!(offered.contains(&Algorithm::Rsa {
            hash: Some(HashAlg::Sha512)
        }));
        assert!(offered.contains(&Algorithm::Rsa {
            hash: Some(HashAlg::Sha256)
        }));
    }

    #[test]
    fn hmac_sha1_is_offered_only_after_every_sha2_mac() {
        let offered = &client_config().preferred.mac;
        let position = |name: mac::Name| offered.iter().position(|m| *m == name).unwrap();
        let last_sha2 = [
            mac::HMAC_SHA512_ETM,
            mac::HMAC_SHA256_ETM,
            mac::HMAC_SHA512,
            mac::HMAC_SHA256,
        ]
        .into_iter()
        .map(position)
        .max()
        .unwrap();
        assert!(position(mac::HMAC_SHA1_ETM) > last_sha2);
        assert!(position(mac::HMAC_SHA1) > last_sha2);
    }

    #[test]
    fn rsa_signs_with_the_sha2_hash_the_server_lists() {
        assert_eq!(rsa_hash_for(Some(Some(HashAlg::Sha512))), HashAlg::Sha512);
        assert_eq!(rsa_hash_for(Some(Some(HashAlg::Sha256))), HashAlg::Sha256);
    }

    #[test]
    fn rsa_never_falls_back_to_sha1() {
        // The server lists only `ssh-rsa`.
        assert_eq!(rsa_hash_for(Some(None)), HashAlg::Sha512);
        // The server sent no `server-sig-algs` at all.
        assert_eq!(rsa_hash_for(None), HashAlg::Sha512);
    }
}
