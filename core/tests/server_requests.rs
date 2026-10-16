//! What a connected server can ask of the client, checked against an
//! in-process russh server on loopback: channels it tries to open back are
//! refused, and a host's startup command reaches the shell it was meant for.

use clavyn_core::host::Host;
use clavyn_core::known_hosts::KnownHosts;
use clavyn_core::session::SessionManager;
use russh::keys::{Algorithm, PrivateKey};
use russh::server::{self, Auth, ChannelOpenHandle, Msg, Session};
use russh::{Channel, ChannelId};
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio::sync::{mpsc, Mutex};

const USER: &str = "deploy";
const PASSWORD: &str = "correct password";

/// What the server saw, reported back to the test.
#[derive(Debug)]
enum Seen {
    /// Bytes the client sent on the shell channel.
    Input(Vec<u8>),
    /// Whether the client accepted each channel the server tried to open.
    Opened { kind: &'static str, accepted: bool },
}

#[derive(Clone)]
struct TestServer {
    seen: mpsc::UnboundedSender<Seen>,
    /// Try to open channels back to the client once its shell starts.
    open_back: bool,
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

    async fn channel_open_session(
        &mut self,
        _channel: Channel<Msg>,
        reply: ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        reply.accept().await;
        Ok(())
    }

    async fn shell_request(
        &mut self,
        _channel: ChannelId,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        if self.open_back {
            let handle = session.handle();
            let seen = self.seen.clone();
            tokio::spawn(async move {
                let agent = handle.channel_open_agent().await.is_ok();
                let _ = seen.send(Seen::Opened { kind: "agent", accepted: agent });
                let x11 = handle.channel_open_x11("127.0.0.1", 6010).await.is_ok();
                let _ = seen.send(Seen::Opened { kind: "x11", accepted: x11 });
                let forwarded = handle
                    .channel_open_forwarded_tcpip("127.0.0.1", 8080, "127.0.0.1", 50000)
                    .await
                    .is_ok();
                let _ = seen.send(Seen::Opened {
                    kind: "forwarded-tcpip",
                    accepted: forwarded,
                });
                let session = handle.channel_open_session().await.is_ok();
                let _ = seen.send(Seen::Opened { kind: "session", accepted: session });
            });
        }
        Ok(())
    }

    async fn data(
        &mut self,
        _channel: ChannelId,
        data: &[u8],
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        let _ = self.seen.send(Seen::Input(data.to_vec()));
        Ok(())
    }
}

async fn start_server(open_back: bool) -> (u16, mpsc::UnboundedReceiver<Seen>) {
    let (seen, events) = mpsc::unbounded_channel();
    let mut rng = getrandom::rand_core::UnwrapErr(getrandom::SysRng);
    let host_key = PrivateKey::random(&mut rng, Algorithm::Ed25519).expect("host key");
    let config = Arc::new(server::Config {
        keys: vec![host_key],
        auth_rejection_time: Duration::from_millis(10),
        auth_rejection_time_initial: Some(Duration::ZERO),
        ..Default::default()
    });
    let handler = TestServer { seen, open_back };
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
    (port, events)
}

fn host(port: u16, startup_command: Option<&str>) -> Host {
    serde_json::from_value(serde_json::json!({
        "id": "00000000-0000-0000-0000-000000000001",
        "label": "Loopback",
        "hostname": "127.0.0.1",
        "port": port,
        "username": USER,
        "auth": {"password": {"credential_key": "unused"}},
        "startup_command": startup_command,
        "tags": []
    }))
    .expect("host")
}

fn known_hosts(dir: &TempDir) -> Arc<Mutex<KnownHosts>> {
    Arc::new(Mutex::new(
        KnownHosts::load(dir.path().join("known_hosts.json")).expect("load known hosts"),
    ))
}

fn sessions() -> SessionManager {
    SessionManager::new(Arc::new(|_, _| {}), Arc::new(|_, _| {}))
}

async fn open_shell(sessions: &SessionManager, host: &Host, dir: &TempDir) {
    let claim = sessions.claim("shell").expect("claim");
    sessions
        .create_ssh_session(&claim, host, None, known_hosts(dir), None, None, Some(PASSWORD), 80, 24)
        .await
        .expect("ssh session");
}

async fn next(events: &mut mpsc::UnboundedReceiver<Seen>) -> Seen {
    tokio::time::timeout(Duration::from_secs(10), events.recv())
        .await
        .expect("the server saw nothing")
        .expect("server gone")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn channels_the_server_opens_back_are_refused() {
    let dir = tempfile::tempdir().expect("temp dir");
    let (port, mut events) = start_server(true).await;
    let sessions = sessions();
    open_shell(&sessions, &host(port, None), &dir).await;

    for expected in ["agent", "x11", "forwarded-tcpip", "session"] {
        match next(&mut events).await {
            Seen::Opened { kind, accepted } => {
                assert_eq!(kind, expected);
                assert!(!accepted, "the client accepted a {kind} channel");
            }
            other => panic!("unexpected {other:?}"),
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_startup_command_is_typed_into_the_shell() {
    let dir = tempfile::tempdir().expect("temp dir");
    let (port, mut events) = start_server(false).await;
    let sessions = sessions();
    open_shell(&sessions, &host(port, Some("tmux attach")), &dir).await;

    match next(&mut events).await {
        Seen::Input(bytes) => assert_eq!(bytes, b"tmux attach\r"),
        other => panic!("unexpected {other:?}"),
    }
}
