//! Ending a connection from outside russh.
//!
//! russh has no call that stops a session's I/O task. `Handle::disconnect` only
//! queues a message, and the task stops reading that queue while a channel is
//! waiting on the peer's window, so the goodbye can sit there for as long as
//! the peer stays stalled. Dropping every `Handle` does not end the task
//! either while a channel half is still alive. [`Transport`] closes the TCP
//! stream underneath russh instead: every read and write on it fails, and
//! russh ends the session, and every channel on it, on that error.

use crate::{CoreError, Result};
use russh::client::{Handle, Handler};
use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::task::{Context, Poll, Waker};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::watch;

/// How long a polite goodbye may take before the transport is cut. A peer that
/// has stopped reading never lets the goodbye out.
pub(crate) const GOODBYE_GRACE: Duration = Duration::from_secs(5);

const READ: usize = 0;
const WRITE: usize = 1;

struct Shared {
    cut: AtomicBool,
    /// The wakers of a read and a write parked on the inner stream, so that a
    /// cut ends them at once rather than when the peer next sends something.
    wakers: Mutex<[Option<Waker>; 2]>,
    /// Set once russh has dropped the stream, which it does when its session
    /// task ends, however that happens.
    ended: watch::Sender<bool>,
}

fn lock_wakers(shared: &Shared) -> MutexGuard<'_, [Option<Waker>; 2]> {
    // A waker slot is valid whatever a panicking holder was doing with it.
    shared.wakers.lock().unwrap_or_else(|e| e.into_inner())
}

/// The means to end one connection's TCP stream, held by whoever owns the
/// connection.
#[derive(Clone)]
pub struct Transport {
    shared: Arc<Shared>,
}

impl Transport {
    /// Wrap `stream` so that the returned transport can sever it.
    pub(crate) fn wrap<S>(stream: S) -> (Severable<S>, Transport) {
        let shared = Arc::new(Shared {
            cut: AtomicBool::new(false),
            wakers: Mutex::new([None, None]),
            ended: watch::Sender::new(false),
        });
        let transport = Transport {
            shared: shared.clone(),
        };
        (
            Severable {
                inner: stream,
                shared,
            },
            transport,
        )
    }

    /// Fail every pending and later read and write on the stream. russh ends
    /// the session on the error, which closes its channels and fails every
    /// call still waiting on it.
    pub fn cut(&self) {
        self.shared.cut.store(true, Ordering::SeqCst);
        let parked = std::mem::take(&mut *lock_wakers(&self.shared));
        for waker in parked.into_iter().flatten() {
            waker.wake();
        }
    }

    /// Resolves once russh has let go of the stream, which means the session
    /// task has ended, whether by a goodbye, an error or a cut.
    pub async fn ended(&self) {
        let mut ended = self.shared.ended.subscribe();
        let _ = ended.wait_for(|ended| *ended).await;
    }
}

/// A stream that fails every operation once its [`Transport`] is cut.
pub(crate) struct Severable<S> {
    inner: S,
    shared: Arc<Shared>,
}

impl<S> Severable<S> {
    /// Park `cx`'s waker in `slot`, then fail if the stream has been cut. The
    /// waker is parked first so that a cut landing between the two still
    /// finds it and wakes the caller.
    fn check(&self, slot: usize, cx: &Context<'_>) -> io::Result<()> {
        {
            let mut wakers = lock_wakers(&self.shared);
            match &mut wakers[slot] {
                Some(parked) if parked.will_wake(cx.waker()) => {}
                parked => *parked = Some(cx.waker().clone()),
            }
        }
        if self.shared.cut.load(Ordering::SeqCst) {
            Err(io::Error::new(
                io::ErrorKind::ConnectionAborted,
                "the connection was closed",
            ))
        } else {
            Ok(())
        }
    }
}

impl<S> Drop for Severable<S> {
    fn drop(&mut self) {
        self.shared.ended.send_replace(true);
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for Severable<S> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if let Err(e) = this.check(READ, cx) {
            return Poll::Ready(Err(e));
        }
        Pin::new(&mut this.inner).poll_read(cx, buf)
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for Severable<S> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        if let Err(e) = this.check(WRITE, cx) {
            return Poll::Ready(Err(e));
        }
        Pin::new(&mut this.inner).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if let Err(e) = this.check(WRITE, cx) {
            return Poll::Ready(Err(e));
        }
        Pin::new(&mut this.inner).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if let Err(e) = this.check(WRITE, cx) {
            return Poll::Ready(Err(e));
        }
        Pin::new(&mut this.inner).poll_shutdown(cx)
    }
}

/// Say goodbye to the peer and wait for the session to end, for at most
/// [`GOODBYE_GRACE`], then cut the transport whether or not that happened.
/// Once this returns the connection is over, or ends on its next poll.
pub(crate) async fn goodbye<H: Handler>(handle: &Handle<H>, transport: &Transport) {
    let polite = async {
        let _ = handle
            .disconnect(russh::Disconnect::ByApplication, "", "en")
            .await;
        transport.ended().await;
    };
    let _ = tokio::time::timeout(GOODBYE_GRACE, polite).await;
    transport.cut();
}

/// Raised once, when a connection is closed. Work already running on the
/// connection watches it and stops, so it cannot keep the connection busy, or
/// report success, after the close.
pub(crate) struct CloseSignal(watch::Sender<bool>);

impl CloseSignal {
    pub(crate) fn new() -> Self {
        Self(watch::Sender::new(false))
    }

    pub(crate) fn raise(&self) {
        self.0.send_replace(true);
    }

    /// Resolves once the signal has been raised, at once if it already was.
    pub(crate) async fn raised(&self) {
        let mut raised = self.0.subscribe();
        let _ = raised.wait_for(|raised| *raised).await;
    }

    /// Run `operation` until it finishes or the signal is raised, and answer
    /// `closed()` in the second case.
    pub(crate) async fn guard<T>(
        &self,
        operation: impl Future<Output = Result<T>>,
        closed: impl FnOnce() -> CoreError,
    ) -> Result<T> {
        tokio::select! {
            result = operation => result,
            () = self.raised() => Err(closed()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Transport;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn a_cut_ends_a_read_that_is_already_waiting() {
        let (near, _far) = tokio::io::duplex(64);
        let (mut stream, transport) = Transport::wrap(near);
        let read = tokio::spawn(async move {
            let mut buf = [0u8; 8];
            stream.read(&mut buf).await
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!read.is_finished());
        transport.cut();
        let result = tokio::time::timeout(Duration::from_secs(1), read)
            .await
            .expect("the read outlived the cut")
            .expect("read task");
        assert_eq!(
            result.expect_err("read after cut").kind(),
            std::io::ErrorKind::ConnectionAborted
        );
    }

    #[tokio::test]
    async fn a_cut_ends_a_write_blocked_on_a_full_buffer() {
        let (near, _far) = tokio::io::duplex(16);
        let (mut stream, transport) = Transport::wrap(near);
        let write = tokio::spawn(async move { stream.write_all(&[0u8; 1024]).await });
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!write.is_finished(), "nothing reads the far end");
        transport.cut();
        let result = tokio::time::timeout(Duration::from_secs(1), write)
            .await
            .expect("the write outlived the cut")
            .expect("write task");
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn ended_resolves_once_the_stream_is_dropped() {
        let (near, _far) = tokio::io::duplex(16);
        let (stream, transport) = Transport::wrap(near);
        let ended = tokio::spawn({
            let transport = transport.clone();
            async move { transport.ended().await }
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!ended.is_finished());
        drop(stream);
        tokio::time::timeout(Duration::from_secs(1), ended)
            .await
            .expect("ended never resolved")
            .expect("ended task");
    }
}
