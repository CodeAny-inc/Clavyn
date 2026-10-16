//! One misbehaving peer must hold up nothing but its own session.
//!
//! These run the SSH and SFTP managers against an in-process russh server on
//! loopback whose SFTP side never finishes a directory listing and whose shell
//! exits as soon as it starts, and check that other sessions, a close and the
//! drain a vault reset relies on all carry on regardless.

use clavyn_core::host::Host;
use clavyn_core::known_hosts::KnownHosts;
use clavyn_core::session::SessionManager;
use clavyn_core::sftp::SftpManager;
use russh::keys::{Algorithm, PrivateKey};
use russh::server::{self, Auth, ChannelOpenHandle, Msg, Session};
use russh::{Channel, ChannelId};
use russh_sftp::protocol::{File, Handle, Name, Status, StatusCode, Version};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio::sync::Mutex;

const USER: &str = "deploy";
const PASSWORD: &str = "correct password";

/// An SFTP server whose directories never end: every READDIR answers with one
/// more entry and never with end-of-file.
struct EndlessDirectory;

impl russh_sftp::server::Handler for EndlessDirectory {
    type Error = StatusCode;

    fn unimplemented(&self) -> Self::Error {
        StatusCode::OpUnsupported
    }

    async fn init(
        &mut self,
        _version: u32,
        _extensions: HashMap<String, String>,
    ) -> Result<Version, Self::Error> {
        Ok(Version::new())
    }

    async fn opendir(&mut self, id: u32, _path: String) -> Result<Handle, Self::Error> {
        Ok(Handle {
            id,
            handle: "dir".into(),
        })
    }

    async fn readdir(&mut self, id: u32, _handle: String) -> Result<Name, Self::Error> {
        tokio::time::sleep(Duration::from_millis(5)).await;
        Ok(Name {
            id,
            files: vec![File::dummy("entry")],
        })
    }

    async fn realpath(&mut self, id: u32, _path: String) -> Result<Name, Self::Error> {
        Ok(Name {
            id,
            files: vec![File::dummy("/home/deploy")],
        })
    }

    async fn close(&mut self, id: u32, _handle: String) -> Result<Status, Self::Error> {
        Ok(Status {
            id,
            status_code: StatusCode::Ok,
            error_message: "Ok".into(),
            language_tag: "en-US".into(),
        })
    }
}

#[derive(Clone, Default)]
struct TestServer {
    channels: Arc<std::sync::Mutex<HashMap<ChannelId, Channel<Msg>>>>,
    /// Stop reading once the first input arrives, the way a hung peer does.
    stall_on_input: bool,
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
        channel: Channel<Msg>,
        reply: ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.channels
            .lock()
            .expect("channels")
            .insert(channel.id(), channel);
        reply.accept().await;
        Ok(())
    }

    /// The shell says one line and exits, unless this server stalls on input,
    /// in which case it stays open.
    async fn shell_request(
        &mut self,
        channel: ChannelId,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        if self.stall_on_input {
            return Ok(());
        }
        session.data(channel, b"bye\r\n".to_vec())?;
        session.eof(channel)?;
        session.close(channel)?;
        Ok(())
    }

    async fn data(
        &mut self,
        _channel: ChannelId,
        _data: &[u8],
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        if self.stall_on_input {
            std::future::pending::<()>().await;
        }
        Ok(())
    }

    async fn subsystem_request(
        &mut self,
        channel_id: ChannelId,
        name: &str,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        let channel = self.channels.lock().expect("channels").remove(&channel_id);
        match channel {
            Some(channel) if name == "sftp" => {
                session.channel_success(channel_id)?;
                russh_sftp::server::run(channel.into_stream(), EndlessDirectory).await;
            }
            _ => session.channel_failure(channel_id)?,
        }
        Ok(())
    }
}

async fn start_server() -> u16 {
    serve(TestServer::default()).await
}

async fn serve(handler: TestServer) -> u16 {
    let mut rng = getrandom::rand_core::UnwrapErr(getrandom::SysRng);
    let host_key = PrivateKey::random(&mut rng, Algorithm::Ed25519).expect("host key");
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
                if let Ok(running) = server::run_stream(config, stream, handler.clone()).await {
                    let _ = running.await;
                }
            });
        }
    });
    port
}

fn host(port: u16) -> Host {
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

fn known_hosts(dir: &TempDir) -> Arc<Mutex<KnownHosts>> {
    Arc::new(Mutex::new(
        KnownHosts::load(dir.path().join("known_hosts.json")).expect("load known hosts"),
    ))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_listing_that_never_ends_holds_up_no_other_sftp_session() {
    let dir = tempfile::tempdir().expect("temp dir");
    let port = start_server().await;
    let host = host(port);
    let pins = known_hosts(&dir);
    let manager = Arc::new(SftpManager::new());
    for id in ["stuck", "other"] {
        manager
            .connect(id.into(), &host, None, pins.clone(), None, None, Some(PASSWORD))
            .await
            .expect("sftp connect");
    }

    let listing = tokio::spawn({
        let manager = manager.clone();
        async move { manager.list_dir("stuck", "/").await }
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!listing.is_finished(), "the endless listing ended on its own");

    let home = tokio::time::timeout(Duration::from_secs(5), manager.canonicalize("other", "."))
        .await
        .expect("another session waited on the listing")
        .expect("canonicalize");
    assert_eq!(home, "/home/deploy");

    // The drain a vault reset runs must take both sessions out of reach
    // without waiting for the listing.
    let closed = tokio::time::timeout(Duration::from_secs(5), manager.close_all())
        .await
        .expect("close_all waited on the listing");
    assert_eq!(closed, 2);
    assert!(manager.list().await.is_empty());

    // Closing the session ends the listing rather than leaving it running.
    let result = tokio::time::timeout(Duration::from_secs(2), listing)
        .await
        .expect("the listing outlived its session")
        .expect("listing task");
    assert!(result.is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn closing_one_sftp_session_does_not_wait_on_its_listing() {
    let dir = tempfile::tempdir().expect("temp dir");
    let port = start_server().await;
    let manager = Arc::new(SftpManager::new());
    manager
        .connect("stuck".into(), &host(port), None, known_hosts(&dir), None, None, Some(PASSWORD))
        .await
        .expect("sftp connect");

    let listing = tokio::spawn({
        let manager = manager.clone();
        async move { manager.list_dir("stuck", "/").await }
    });
    tokio::time::sleep(Duration::from_millis(300)).await;

    tokio::time::timeout(Duration::from_secs(5), manager.close("stuck"))
        .await
        .expect("close waited on the listing")
        .expect("close");
    assert!(manager.list().await.is_empty());
    let result = tokio::time::timeout(Duration::from_secs(2), listing)
        .await
        .expect("the listing outlived its session")
        .expect("listing task");
    assert!(result.is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_shell_that_exits_leaves_no_session_behind() {
    let dir = tempfile::tempdir().expect("temp dir");
    let port = start_server().await;
    let (closed_tx, mut closed_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let output = Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
    let sessions = SessionManager::new(
        {
            let output = output.clone();
            Arc::new(move |_, data: &[u8]| output.lock().expect("output").extend_from_slice(data))
        },
        Arc::new(move |id: &str, _| {
            let _ = closed_tx.send(id.to_string());
        }),
    );

    let claim = sessions.claim("shell").expect("claim");
    sessions
        .create_ssh_session(&claim, &host(port), None, known_hosts(&dir), None, None, Some(PASSWORD), 80, 24)
        .await
        .expect("ssh session");
    drop(claim);

    let closed = tokio::time::timeout(Duration::from_secs(10), closed_rx.recv())
        .await
        .expect("the close was never announced");
    assert_eq!(closed.as_deref(), Some("shell"));
    assert_eq!(output.lock().expect("output").as_slice(), b"bye\r\n");

    // The reader removes its entry after announcing the close; give it the
    // moment that takes, then the id must be gone and free again.
    let gone = async {
        while sessions.contains("shell").await || sessions.is_claimed("shell") {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    };
    tokio::time::timeout(Duration::from_secs(10), gone)
        .await
        .expect("a session whose shell exited stayed registered");
    assert!(sessions.write("shell", b"ls\n").await.is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_write_stuck_behind_a_peer_that_stopped_reading_ends_with_the_close() {
    let dir = tempfile::tempdir().expect("temp dir");
    let port = serve(TestServer {
        stall_on_input: true,
        ..TestServer::default()
    })
    .await;
    let sessions = Arc::new(SessionManager::new(Arc::new(|_, _| {}), Arc::new(|_, _| {})));
    let claim = sessions.claim("stuck").expect("claim");
    sessions
        .create_ssh_session(&claim, &host(port), None, known_hosts(&dir), None, None, Some(PASSWORD), 80, 24)
        .await
        .expect("ssh session");
    drop(claim);

    // Keep writing until the peer's window and the socket buffers are full
    // and a write stops returning.
    let written = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let writer = tokio::spawn({
        let sessions = sessions.clone();
        let written = written.clone();
        async move {
            loop {
                sessions.write("stuck", &[b'x'; 64 * 1024]).await?;
                written.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
            #[allow(unreachable_code)]
            Ok::<(), clavyn_core::CoreError>(())
        }
    });
    let stuck = async {
        loop {
            let before = written.load(std::sync::atomic::Ordering::Relaxed);
            tokio::time::sleep(Duration::from_millis(500)).await;
            if before > 0 && written.load(std::sync::atomic::Ordering::Relaxed) == before {
                return;
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(20), stuck)
        .await
        .expect("writes never stalled");
    assert!(!writer.is_finished());

    tokio::time::timeout(Duration::from_secs(10), sessions.close("stuck"))
        .await
        .expect("close waited on the stuck write")
        .expect("close");
    let result = tokio::time::timeout(Duration::from_secs(2), writer)
        .await
        .expect("the stuck write outlived the close")
        .expect("writer task");
    assert!(result.is_err());
    assert!(!sessions.contains("stuck").await);
}
