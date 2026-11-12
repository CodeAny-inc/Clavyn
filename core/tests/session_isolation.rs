//! One misbehaving peer must hold up nothing but its own session.
//!
//! These run the SSH and SFTP managers against an in-process russh server on
//! loopback whose SFTP side never finishes a directory listing and whose shell
//! exits as soon as it starts, or stops reading its input, and check that other
//! sessions, a close and the drain a vault reset relies on all carry on
//! regardless, and that a close always ends the connection and frees its id.

use clavyn_core::host::Host;
use clavyn_core::known_hosts::KnownHosts;
use clavyn_core::session::SessionManager;
use clavyn_core::sftp::SftpManager;
use russh::keys::{Algorithm, PrivateKey};
use russh::server::{self, Auth, ChannelOpenHandle, Msg, Session};
use russh::{Channel, ChannelId};
use russh_sftp::protocol::{
    Attrs, File, FileAttributes, Handle, Name, OpenFlags, Status, StatusCode, Version,
};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio::sync::Mutex;

const USER: &str = "deploy";
const PASSWORD: &str = "correct password";

/// An SFTP server whose directories never end: every READDIR answers with more
/// entries and never with end-of-file. Its files exist only as names, enough
/// to tell an exclusive create from one that replaces a file.
#[derive(Clone)]
struct EndlessDirectory {
    /// Entries in each READDIR answer.
    page: usize,
    /// Pause before each READDIR answer.
    page_delay: Duration,
    /// Pause before each STAT answer.
    stat_delay: Duration,
    /// Handles closed so far.
    closes: Arc<AtomicUsize>,
    /// Paths created so far.
    files: Arc<std::sync::Mutex<HashSet<String>>>,
}

impl Default for EndlessDirectory {
    fn default() -> Self {
        Self {
            page: 1,
            page_delay: Duration::from_millis(5),
            stat_delay: Duration::ZERO,
            closes: Arc::default(),
            files: Arc::default(),
        }
    }
}

fn ok_status(id: u32) -> Status {
    Status {
        id,
        status_code: StatusCode::Ok,
        error_message: "Ok".into(),
        language_tag: "en-US".into(),
    }
}

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
        tokio::time::sleep(self.page_delay).await;
        Ok(Name {
            id,
            files: vec![File::dummy("entry"); self.page],
        })
    }

    async fn realpath(&mut self, id: u32, _path: String) -> Result<Name, Self::Error> {
        Ok(Name {
            id,
            files: vec![File::dummy("/home/deploy")],
        })
    }

    async fn close(&mut self, id: u32, _handle: String) -> Result<Status, Self::Error> {
        self.closes.fetch_add(1, Ordering::SeqCst);
        Ok(ok_status(id))
    }

    async fn open(
        &mut self,
        id: u32,
        filename: String,
        pflags: OpenFlags,
        _attrs: FileAttributes,
    ) -> Result<Handle, Self::Error> {
        let mut files = self.files.lock().expect("files");
        if pflags.contains(OpenFlags::EXCLUDE) && files.contains(&filename) {
            return Err(StatusCode::Failure);
        }
        files.insert(filename.clone());
        Ok(Handle {
            id,
            handle: filename,
        })
    }

    async fn write(
        &mut self,
        id: u32,
        _handle: String,
        _offset: u64,
        _data: Vec<u8>,
    ) -> Result<Status, Self::Error> {
        Ok(ok_status(id))
    }

    async fn stat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
        tokio::time::sleep(self.stat_delay).await;
        if self.files.lock().expect("files").contains(&path) {
            Ok(Attrs {
                id,
                attrs: FileAttributes::default(),
            })
        } else {
            Err(StatusCode::NoSuchFile)
        }
    }
}

#[derive(Clone, Default)]
struct TestServer {
    channels: Arc<std::sync::Mutex<HashMap<ChannelId, Channel<Msg>>>>,
    /// Stop reading once the first input arrives, the way a hung peer does.
    stall_on_input: bool,
    /// Stop reading for this long when the first input arrives, then read
    /// normally again, the way a peer that recovers does.
    stall_once: Option<(Duration, Arc<AtomicBool>)>,
    sftp: EndlessDirectory,
    /// Connections the server has seen end.
    ended: Arc<AtomicUsize>,
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
        if self.stall_on_input || self.stall_once.is_some() {
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
        if let Some((stall, stalled)) = &self.stall_once {
            if !stalled.swap(true, Ordering::SeqCst) {
                tokio::time::sleep(*stall).await;
            }
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
                russh_sftp::server::run(channel.into_stream(), self.sftp.clone()).await;
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
                handler.ended.fetch_add(1, Ordering::SeqCst);
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
    connect_shell(&sessions, port, &dir).await;
    let writer = write_until_stuck(&sessions).await;

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

/// Open a shell on `port` under the id "stuck".
async fn connect_shell(sessions: &SessionManager, port: u16, dir: &TempDir) {
    let claim = sessions.claim("stuck").expect("claim");
    sessions
        .create_ssh_session(&claim, &host(port), None, known_hosts(dir), None, None, Some(PASSWORD), 80, 24)
        .await
        .expect("ssh session");
}

/// Keep writing to "stuck" until the peer's window and the socket buffers are
/// full and a write stops returning, and hand back the task still writing.
async fn write_until_stuck(
    sessions: &Arc<SessionManager>,
) -> tokio::task::JoinHandle<Result<(), clavyn_core::CoreError>> {
    let written = Arc::new(AtomicUsize::new(0));
    let writer = tokio::spawn({
        let sessions = sessions.clone();
        let written = written.clone();
        async move {
            loop {
                sessions.write("stuck", &[b'x'; 64 * 1024]).await?;
                written.fetch_add(1, Ordering::Relaxed);
            }
            #[allow(unreachable_code)]
            Ok::<(), clavyn_core::CoreError>(())
        }
    });
    let stuck = async {
        loop {
            let before = written.load(Ordering::Relaxed);
            tokio::time::sleep(Duration::from_millis(500)).await;
            if before > 0 && written.load(Ordering::Relaxed) == before {
                return;
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(20), stuck)
        .await
        .expect("writes never stalled");
    assert!(!writer.is_finished());
    writer
}

/// A manager whose close announcements arrive on the returned receiver.
fn announcing_manager() -> (Arc<SessionManager>, tokio::sync::mpsc::UnboundedReceiver<String>) {
    let (closed_tx, closed_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let sessions = SessionManager::new(
        Arc::new(|_, _| {}),
        Arc::new(move |id: &str, _| {
            let _ = closed_tx.send(id.to_string());
        }),
    );
    (Arc::new(sessions), closed_rx)
}

/// Wait for the close of "stuck" to be announced and its id to come free.
async fn expect_released(
    sessions: &SessionManager,
    closed_rx: &mut tokio::sync::mpsc::UnboundedReceiver<String>,
) {
    let announced = tokio::time::timeout(Duration::from_secs(10), closed_rx.recv())
        .await
        .expect("the close was never announced");
    assert_eq!(announced.as_deref(), Some("stuck"));
    let free = async {
        while sessions.is_claimed("stuck") {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    };
    tokio::time::timeout(Duration::from_secs(5), free)
        .await
        .expect("the id stayed claimed after the close");
    assert!(sessions.claim("stuck").is_ok(), "the id could not be reused");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn closing_a_session_whose_peer_stopped_reading_frees_its_id() {
    let dir = tempfile::tempdir().expect("temp dir");
    let port = serve(TestServer {
        stall_on_input: true,
        ..TestServer::default()
    })
    .await;
    let (sessions, mut closed_rx) = announcing_manager();
    connect_shell(&sessions, port, &dir).await;
    let writer = write_until_stuck(&sessions).await;

    // The goodbye cannot get past the unread input, so it is the cut after
    // the grace that has to end the connection, and with it the reader.
    tokio::time::timeout(Duration::from_secs(10), sessions.close("stuck"))
        .await
        .expect("close waited on the stuck write")
        .expect("close");
    let _ = writer.await;
    expect_released(&sessions, &mut closed_rx).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_peer_that_recovers_after_the_goodbye_timed_out_is_still_disconnected() {
    let dir = tempfile::tempdir().expect("temp dir");
    let server = TestServer {
        stall_once: Some((Duration::from_secs(8), Arc::new(AtomicBool::new(false)))),
        ..TestServer::default()
    };
    let ended = server.ended.clone();
    let port = serve(server).await;
    let (sessions, mut closed_rx) = announcing_manager();
    connect_shell(&sessions, port, &dir).await;
    let writer = write_until_stuck(&sessions).await;

    tokio::time::timeout(Duration::from_secs(10), sessions.close("stuck"))
        .await
        .expect("close waited on the stuck write")
        .expect("close");
    let _ = writer.await;
    expect_released(&sessions, &mut closed_rx).await;

    // Once the peer reads again it finds the connection gone, rather than a
    // session nobody can address any more.
    let gone = async {
        while ended.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    };
    tokio::time::timeout(Duration::from_secs(15), gone)
        .await
        .expect("the connection outlived the close on the server's side");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_resize_never_waits_on_a_peer_that_stopped_reading() {
    let dir = tempfile::tempdir().expect("temp dir");
    let port = serve(TestServer {
        stall_on_input: true,
        ..TestServer::default()
    })
    .await;
    let (sessions, mut closed_rx) = announcing_manager();
    connect_shell(&sessions, port, &dir).await;
    let writer = write_until_stuck(&sessions).await;

    for step in 0..100u32 {
        tokio::time::timeout(Duration::from_secs(1), sessions.resize("stuck", 80 + step, 24))
            .await
            .expect("a resize waited on the stalled peer")
            .expect("resize");
    }

    tokio::time::timeout(Duration::from_secs(10), sessions.close("stuck"))
        .await
        .expect("close waited on the stuck resize")
        .expect("close");
    let _ = writer.await;
    expect_released(&sessions, &mut closed_rx).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_listing_past_the_entry_cap_is_refused_and_its_handle_closed() {
    let dir = tempfile::tempdir().expect("temp dir");
    let sftp = EndlessDirectory {
        page: 2_000,
        page_delay: Duration::ZERO,
        ..EndlessDirectory::default()
    };
    let closes = sftp.closes.clone();
    let port = serve(TestServer {
        sftp,
        ..TestServer::default()
    })
    .await;
    let manager = SftpManager::new();
    manager
        .connect("big".into(), &host(port), None, known_hosts(&dir), None, None, Some(PASSWORD))
        .await
        .expect("sftp connect");

    let error = tokio::time::timeout(Duration::from_secs(60), manager.list_dir("big", "/"))
        .await
        .expect("the listing ran past the entry cap")
        .expect_err("a directory past the entry cap was listed");
    assert!(error.to_string().contains("more than"), "{error}");
    assert_eq!(closes.load(Ordering::SeqCst), 1, "the directory handle was left open");
    manager.close_all().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn of_two_uploads_to_one_path_without_overwrite_only_one_creates_it() {
    let dir = tempfile::tempdir().expect("temp dir");
    let local = dir.path().join("report.txt");
    std::fs::write(&local, b"report").expect("local file");
    // Slow existence checks let both uploads pass theirs before either one
    // creates the file.
    let port = serve(TestServer {
        sftp: EndlessDirectory {
            stat_delay: Duration::from_millis(200),
            ..EndlessDirectory::default()
        },
        ..TestServer::default()
    })
    .await;
    let manager = Arc::new(SftpManager::new());
    manager
        .connect("up".into(), &host(port), None, known_hosts(&dir), None, None, Some(PASSWORD))
        .await
        .expect("sftp connect");

    let upload = || {
        let manager = manager.clone();
        let local = local.clone();
        tokio::spawn(async move {
            manager
                .upload_from_local("up", &local, "/report.txt", false)
                .await
        })
    };
    let (first, second) = tokio::join!(upload(), upload());
    let results = [first.expect("upload task"), second.expect("upload task")];
    assert_eq!(
        results.iter().filter(|result| result.is_ok()).count(),
        1,
        "{results:?}"
    );
    let refused = results
        .iter()
        .find_map(|result| result.as_ref().err())
        .expect("one upload refused");
    assert!(refused.to_string().contains("already exists"), "{refused}");
    manager.close_all().await;
}
