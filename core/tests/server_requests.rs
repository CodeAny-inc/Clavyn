//! What a connected server can ask of the client, and what the client sends it
//! unasked, checked against an in-process russh server on loopback: channels
//! the server tries to open back are refused, and a host's startup command
//! reaches the shell it was meant for once that shell is ready for it.

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
    /// The shell finished printing its prompt.
    Prompted,
}

/// How the test server's shell behaves once it starts.
#[derive(Clone, Copy)]
enum Shell {
    /// Prints nothing at all.
    Silent,
    /// Prints a login banner, runs its startup files without a word for
    /// [`STARTUP_FILES`], then prints a prompt and waits.
    Prompt,
    /// Prints nothing and opens channels back to the client.
    OpenBack,
}

/// How long the [`Shell::Prompt`] shell is quiet between its banner and its
/// prompt, the way a shell busy with its startup files is.
const STARTUP_FILES: Duration = Duration::from_millis(400);

#[derive(Clone)]
struct TestServer {
    seen: mpsc::UnboundedSender<Seen>,
    shell: Shell,
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
        channel: ChannelId,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        let handle = session.handle();
        let seen = self.seen.clone();
        match self.shell {
            Shell::Silent => {}
            Shell::Prompt => {
                tokio::spawn(async move {
                    let _ = handle.data(channel, &b"Last login: today\r\n"[..]).await;
                    tokio::time::sleep(STARTUP_FILES).await;
                    let _ = handle.data(channel, &b"$ "[..]).await;
                    let _ = seen.send(Seen::Prompted);
                });
            }
            Shell::OpenBack => {
                tokio::spawn(open_back(handle, seen));
            }
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

/// Opens one channel of every type russh can open back, in the order
/// [`OPENED_BACK`] lists them, and reports whether the client accepted each.
async fn open_back(handle: server::Handle, seen: mpsc::UnboundedSender<Seen>) {
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
}

async fn start_server(shell: Shell) -> (u16, mpsc::UnboundedReceiver<Seen>) {
    let (seen, events) = mpsc::unbounded_channel();
    let port = serve(server_config(random_host_key()), TestServer { seen, shell }).await;
    (port, events)
}

fn sessions() -> SessionManager {
    SessionManager::new(Arc::new(|_, _| {}), Arc::new(|_, _| {}))
}

fn host_with_startup_command(port: u16) -> Host {
    let mut host = password_host(port);
    host.startup_command = Some("tmux attach".into());
    host
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
    let (port, mut events) = start_server(Shell::OpenBack).await;
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
async fn the_startup_command_waits_for_the_shell_prompt() {
    let dir = tempfile::tempdir().expect("temp dir");
    let (port, mut events) = start_server(Shell::Prompt).await;
    let sessions = sessions();
    open_shell(&sessions, &host_with_startup_command(port), &dir).await;

    // Typed any earlier, the command would land while the startup files are
    // still running and could be read by them instead of the shell.
    match next(&mut events).await {
        Seen::Prompted => {}
        other => panic!("the client sent something before the prompt: {other:?}"),
    }
    match next(&mut events).await {
        Seen::Input(bytes) => assert_eq!(bytes, b"tmux attach\r"),
        other => panic!("unexpected {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn input_sent_while_the_startup_command_waits_follows_it() {
    let dir = tempfile::tempdir().expect("temp dir");
    let (port, mut events) = start_server(Shell::Prompt).await;
    let sessions = Arc::new(sessions());
    open_shell(&sessions, &host_with_startup_command(port), &dir).await;

    let typed = tokio::spawn({
        let sessions = sessions.clone();
        async move { sessions.write("shell", b"ls\r").await }
    });

    let mut inputs = Vec::new();
    while inputs.len() < 2 {
        if let Seen::Input(bytes) = next(&mut events).await {
            inputs.push(bytes);
        }
    }
    assert_eq!(inputs, [b"tmux attach\r".to_vec(), b"ls\r".to_vec()]);
    typed.await.expect("write task").expect("write");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_shell_that_prints_nothing_still_gets_the_startup_command() {
    let dir = tempfile::tempdir().expect("temp dir");
    let (port, mut events) = start_server(Shell::Silent).await;
    let sessions = sessions();
    open_shell(&sessions, &host_with_startup_command(port), &dir).await;

    match next(&mut events).await {
        Seen::Input(bytes) => assert_eq!(bytes, b"tmux attach\r"),
        other => panic!("unexpected {other:?}"),
    }
}
