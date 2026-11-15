//! A peer that produces output faster than the UI acknowledges it is held to
//! one window of unacknowledged output.
//!
//! Runs the SSH session manager against an in-process russh server on loopback
//! whose shell writes without pause for as long as the connection will take
//! it, and checks that what reaches the data callback stops growing until the
//! UI acknowledges it, that the stall reaches the server, that a stalled
//! session still closes, and that input and resizes sent during a stall do not
//! keep it stalled once the UI catches up.

use clavyn_core::host::Host;
use clavyn_core::known_hosts::KnownHosts;
use clavyn_core::output::{MAX_BATCH, OUTPUT_WINDOW};
use clavyn_core::session::SessionManager;
use russh::keys::{Algorithm, PrivateKey};
use russh::server::{self, Auth, ChannelOpenHandle, Msg, Session};
use russh::{Channel, ChannelId};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::Mutex;

const USER: &str = "deploy";
const PASSWORD: &str = "correct password";
const CHUNK: usize = 64 * 1024;

/// A server whose shell floods its channel, counting what it managed to hand
/// to its connection and recording what the client sent it.
#[derive(Clone, Default)]
struct FloodServer {
    produced: Arc<AtomicU64>,
    /// Bytes of client input that reached the shell.
    typed: Arc<AtomicU64>,
    /// The most recent terminal size the client set.
    size: Arc<std::sync::Mutex<Option<(u32, u32)>>>,
}

impl FloodServer {
    fn size(&self) -> Option<(u32, u32)> {
        *self.size.lock().expect("size")
    }
}

impl server::Handler for FloodServer {
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
        let produced = self.produced.clone();
        tokio::spawn(async move {
            while handle.data(channel, vec![b'x'; CHUNK]).await.is_ok() {
                produced.fetch_add(CHUNK as u64, Ordering::Relaxed);
            }
        });
        Ok(())
    }

    async fn data(
        &mut self,
        _channel: ChannelId,
        data: &[u8],
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.typed.fetch_add(data.len() as u64, Ordering::SeqCst);
        Ok(())
    }

    async fn window_change_request(
        &mut self,
        _channel: ChannelId,
        cols: u32,
        rows: u32,
        _pix_width: u32,
        _pix_height: u32,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        *self.size.lock().expect("size") = Some((cols, rows));
        Ok(())
    }
}

async fn start_server(handler: FloodServer) -> u16 {
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
                if let Ok(running) = server::run_stream(config, stream, handler).await {
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

/// An SSH session on a flooding server whose output nothing acknowledges yet.
struct Flooded {
    sessions: Arc<SessionManager>,
    server: FloodServer,
    /// Output that reached the data callback.
    delivered: Arc<AtomicU64>,
    /// Ids the session manager announced as closed.
    closed: tokio::sync::mpsc::UnboundedReceiver<String>,
    _dir: tempfile::TempDir,
}

/// Connects a session on `session_id` to a fresh flooding server.
async fn flooded_session(session_id: &str) -> Flooded {
    let dir = tempfile::tempdir().expect("temp dir");
    let server = FloodServer::default();
    let port = start_server(server.clone()).await;

    let delivered = Arc::new(AtomicU64::new(0));
    let (closed_tx, closed) = tokio::sync::mpsc::unbounded_channel::<String>();
    let sessions = Arc::new(SessionManager::new(
        {
            let delivered = delivered.clone();
            Arc::new(move |_, data: &[u8]| {
                delivered.fetch_add(data.len() as u64, Ordering::SeqCst);
            })
        },
        Arc::new(move |id: &str, _| {
            let _ = closed_tx.send(id.to_string());
        }),
    ));
    let known_hosts = Arc::new(Mutex::new(
        KnownHosts::load(dir.path().join("known_hosts.json")).expect("known hosts"),
    ));

    let claim = sessions.claim(session_id).expect("claim");
    sessions
        .create_ssh_session(&claim, &host(port), None, known_hosts, None, None, Some(PASSWORD), 80, 24)
        .await
        .expect("ssh session");
    drop(claim);

    Flooded {
        sessions,
        server,
        delivered,
        closed,
        _dir: dir,
    }
}

/// Waits until `counter` holds still across a sampling interval and returns
/// the value it settled on.
async fn settled(counter: &AtomicU64) -> u64 {
    let settle = async {
        let mut last = counter.load(Ordering::SeqCst);
        loop {
            tokio::time::sleep(Duration::from_millis(400)).await;
            let now = counter.load(Ordering::SeqCst);
            if now == last && now > 0 {
                return now;
            }
            last = now;
        }
    };
    tokio::time::timeout(Duration::from_secs(20), settle)
        .await
        .expect("output kept growing with nothing acknowledged")
}

/// Closes `session_id` with nothing more acknowledged and expects the close to
/// be announced.
async fn close_and_expect_announcement(flood: &mut Flooded, session_id: &str) {
    tokio::time::timeout(Duration::from_secs(10), flood.sessions.close(session_id))
        .await
        .expect("close waited on the stalled reader")
        .expect("close");
    let closed = tokio::time::timeout(Duration::from_secs(15), flood.closed.recv())
        .await
        .expect("a stalled session never announced its close");
    assert_eq!(closed.as_deref(), Some(session_id));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unacknowledged_output_stalls_the_session_until_it_is_acknowledged() {
    let mut flood = flooded_session("flood").await;

    // A due batch is still delivered after the window is spent, and the
    // batcher can hold one protocol packet past a full batch.
    let ceiling = OUTPUT_WINDOW + MAX_BATCH as u64 + CHUNK as u64;

    let first = settled(&flood.delivered).await;
    assert!(first >= OUTPUT_WINDOW, "stalled at {first} bytes, short of the window");
    assert!(first <= ceiling, "{first} bytes delivered past a window of {OUTPUT_WINDOW}");

    // The stall reaches the peer: it stops getting anything onto the
    // connection rather than having it buffered on our side without limit.
    let produced = &flood.server.produced;
    let stalled_at = settled(produced).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(produced.load(Ordering::SeqCst), stalled_at, "the server never stalled");
    assert_eq!(flood.delivered.load(Ordering::SeqCst), first);

    // Acknowledging what arrived lets about one more window through, and no
    // more than that.
    flood.sessions.ack("flood", first).await;
    let second = settled(&flood.delivered).await;
    assert!(second > first, "acknowledged output did not resume the session");
    assert!(
        second - first <= ceiling,
        "{} bytes delivered past an acknowledgement of {first}",
        second - first
    );

    // A stalled session still closes, and announces it, with nothing more
    // acknowledged.
    close_and_expect_announcement(&mut flood, "flood").await;

    // Acknowledgements trailing a closed session are ignored.
    flood.sessions.ack("flood", 1).await;
    flood.sessions.ack("never-existed", 1).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn input_and_a_resize_sent_during_a_stall_do_not_keep_the_session_stalled() {
    let mut flood = flooded_session("resize").await;
    let first = settled(&flood.delivered).await;
    assert!(first >= OUTPUT_WINDOW, "stalled at {first} bytes, short of the window");

    // While the reader is stalled, russh's connection loop is blocked handing
    // it output, so nothing queued for the peer goes out. Enough keystrokes
    // fill that outbound queue, and a resize sent after them has to wait for
    // the reader to resume.
    const KEYS: u64 = 16;
    for _ in 0..KEYS {
        let sessions = flood.sessions.clone();
        tokio::spawn(async move {
            let _ = sessions.write("resize", b"q").await;
        });
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    tokio::time::timeout(Duration::from_secs(5), flood.sessions.resize("resize", 100, 30))
        .await
        .expect("the resize request waited on the stalled connection")
        .expect("resize");
    tokio::time::sleep(Duration::from_millis(300)).await;

    // A UI that catches up acknowledges what it has received, as often as it
    // receives it. The session must then resume, and the input and the resize
    // must reach the peer.
    let caught_up = async {
        let mut acknowledged = 0;
        loop {
            let delivered = flood.delivered.load(Ordering::SeqCst);
            if delivered > first
                && flood.server.typed.load(Ordering::SeqCst) == KEYS
                && flood.server.size() == Some((100, 30))
            {
                return;
            }
            flood.sessions.ack("resize", delivered - acknowledged).await;
            acknowledged = delivered;
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    };
    if tokio::time::timeout(Duration::from_secs(15), caught_up).await.is_err() {
        panic!(
            "the session stayed stalled after the UI caught up: {} bytes delivered (stalled at \
             {first}), {} of {KEYS} keystrokes and size {:?} reached the server",
            flood.delivered.load(Ordering::SeqCst),
            flood.server.typed.load(Ordering::SeqCst),
            flood.server.size(),
        );
    }

    close_and_expect_announcement(&mut flood, "resize").await;
}
