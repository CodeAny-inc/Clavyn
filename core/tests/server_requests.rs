//! What a connected server can ask of the client, checked against an
//! in-process russh server on loopback: channels it tries to open back are
//! refused, and a host's startup command reaches the shell it was meant for.

mod common;

use clavyn_core::host::Host;
use clavyn_core::session::SessionManager;
use common::loopback::{known_hosts, password_host, random_host_key, serve, server_config};
use common::loopback::{PASSWORD, USER};
use russh::server::{self, Auth, ChannelOpenHandle, Msg, Session};
use russh::{Channel, ChannelId};
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::mpsc;

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

    /// Opens one channel of every type russh can open back, in the order
    /// [`OPENED_BACK`] lists them.
    async fn shell_request(
        &mut self,
        _channel: ChannelId,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        if self.open_back {
            let handle = session.handle();
            let seen = self.seen.clone();
            tokio::spawn(async move {
                let report = |kind, accepted| {
                    let _ = seen.send(Seen::Opened { kind, accepted });
                };
                report("agent", handle.channel_open_agent().await.is_ok());
                report("x11", handle.channel_open_x11("127.0.0.1", 6010).await.is_ok());
                report(
                    "forwarded-tcpip",
                    handle
                        .channel_open_forwarded_tcpip("127.0.0.1", 8080, "127.0.0.1", 50000)
                        .await
                        .is_ok(),
                );
                report(
                    "forwarded-streamlocal",
                    handle
                        .channel_open_forwarded_streamlocal("/tmp/forwarded.sock")
                        .await
                        .is_ok(),
                );
                report(
                    "direct-tcpip",
                    handle
                        .channel_open_direct_tcpip("127.0.0.1", 22, "127.0.0.1", 50001)
                        .await
                        .is_ok(),
                );
                report(
                    "direct-streamlocal",
                    handle
                        .channel_open_direct_streamlocal("/tmp/direct.sock")
                        .await
                        .is_ok(),
                );
                report("session", handle.channel_open_session().await.is_ok());
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

/// Every channel type the test server opens back, in the order it opens them.
const OPENED_BACK: [&str; 7] = [
    "agent",
    "x11",
    "forwarded-tcpip",
    "forwarded-streamlocal",
    "direct-tcpip",
    "direct-streamlocal",
    "session",
];

async fn start_server(open_back: bool) -> (u16, mpsc::UnboundedReceiver<Seen>) {
    let (seen, events) = mpsc::unbounded_channel();
    let port = serve(server_config(random_host_key()), TestServer { seen, open_back }).await;
    (port, events)
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
    open_shell(&sessions, &password_host(port), &dir).await;

    for expected in OPENED_BACK {
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
    let mut host = password_host(port);
    host.startup_command = Some("tmux attach".into());
    open_shell(&sessions, &host, &dir).await;

    match next(&mut events).await {
        Seen::Input(bytes) => assert_eq!(bytes, b"tmux attach\r"),
        other => panic!("unexpected {other:?}"),
    }
}
