// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Attaching gRPC to the sockets Buck2 hands us.
//!
//! On Unix, Buck2 creates two socket pairs, keeps one end of each, and passes
//! the other ends as raw file descriptors on the command line. Elsewhere it
//! binds two ports, passes their addresses, and accepts the connections the
//! executor makes back to them.
//!
//! Either way nothing here binds or accepts, and the direction a socket was
//! established in says nothing about which side serves: Buck2 is the gRPC
//! client on the executor socket and the server on the orchestrator one, no
//! matter who connected to whom. So this module is only concerned with wrapping
//! an already-connected stream in the shapes tonic's server and client want.

use crate::{
    errors::{ExpectedError, Result},
    proto::test_executor_server::{TestExecutor, TestExecutorServer},
};
use futures::{Stream, StreamExt};
use hyper_util::rt::TokioIo;
#[cfg(unix)]
use std::os::fd::{FromRawFd, RawFd};
use std::{
    future::Future,
    io,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll},
};
use tokio::net::TcpStream;
use tonic::transport::{Channel, Endpoint, Server, Uri};
use tower::Service;

/// How to reach one of the two sockets, as named on the command line.
///
/// Plain data: turning it into a socket has to happen inside a tokio runtime,
/// since that is where the reactor a socket registers with lives.
#[derive(Clone, Debug)]
pub enum SocketSpec {
    /// A file descriptor Buck2 passed down, on Unix.
    #[cfg(unix)]
    Fd(RawFd),

    /// An address Buck2 is listening on, elsewhere.
    Addr(String),
}

/// One of the two streams Buck2 set up, in whichever form the platform uses.
#[derive(Debug)]
pub enum Socket {
    /// A Unix domain socket Buck2 passed as an inherited file descriptor.
    #[cfg(unix)]
    Unix(tokio::net::UnixStream),

    /// A TCP connection to an address Buck2 is listening on.
    Tcp(TcpStream),
}

impl Socket {
    /// Turns a spec into a usable socket.
    ///
    /// Must be called from within a tokio runtime.
    pub async fn adopt(spec: SocketSpec, which: &'static str) -> Result<Self> {
        match spec {
            #[cfg(unix)]
            SocketSpec::Fd(fd) => Self::from_raw_fd(fd, which),
            SocketSpec::Addr(addr) => Self::connect_tcp(&addr, which).await,
        }
    }

    /// Adopts a file descriptor Buck2 passed on the command line.
    ///
    /// Buck2 clears `FD_CLOEXEC` on exactly two descriptors before spawning the
    /// executor and names both on the command line, so a descriptor that came
    /// from Buck2 is a connected stream socket that nothing else owns. That is
    /// not true of a number a person typed, which is why the flags carrying
    /// these are hidden.
    #[cfg(unix)]
    fn from_raw_fd(fd: RawFd, which: &'static str) -> Result<Self> {
        // SAFETY: see above -- Buck2 handed us this descriptor and nothing else
        // in the process has taken ownership of it.
        let std_stream = unsafe { std::os::unix::net::UnixStream::from_raw_fd(fd) };
        std_stream
            .set_nonblocking(true)
            .and_then(|()| tokio::net::UnixStream::from_std(std_stream))
            .map(Socket::Unix)
            .map_err(|error| ExpectedError::SocketAdoptError { which, error })
    }

    /// Connects to an address Buck2 is listening on.
    async fn connect_tcp(addr: &str, which: &'static str) -> Result<Self> {
        TcpStream::connect(addr)
            .await
            .map(Socket::Tcp)
            .map_err(|error| ExpectedError::SocketConnectError {
                which,
                addr: addr.to_owned(),
                error,
            })
    }

    /// Builds a client channel that speaks over this socket.
    ///
    /// The URI is required by the HTTP/2 machinery but never dialed, since the
    /// connector below ignores it and returns the socket we already hold.
    pub async fn into_channel(self, which: &'static str) -> Result<Channel> {
        Endpoint::from_static("http://buck2.invalid")
            .connect_with_connector(SocketConnector::new(self))
            .await
            .map_err(|error| ExpectedError::ChannelBuildError {
                which,
                error: Box::new(error),
            })
    }
}

/// Serves the executor service over a socket until Buck2 hangs up.
///
/// `serve_with_incoming_shutdown` normally consumes a listener; here the single
/// already-connected socket is handed over as a one-item stream, so the server
/// never accepts anything and ends when that connection closes.
pub async fn serve_test_executor<T>(
    socket: Socket,
    service: TestExecutorServer<T>,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> Result<()>
where
    T: TestExecutor,
{
    let server = Server::builder().add_service(service);
    let result = match socket {
        #[cfg(unix)]
        Socket::Unix(stream) => {
            server
                .serve_with_incoming_shutdown(once_connected(stream), shutdown)
                .await
        }
        Socket::Tcp(stream) => {
            server
                .serve_with_incoming_shutdown(once_connected(stream), shutdown)
                .await
        }
    };

    result.map_err(|error| ExpectedError::ExecutorServeError {
        error: Box::new(error),
    })
}

/// Presents one connected stream as the stream of incoming connections.
///
/// The stream must never end. tonic's accept loop treats an exhausted incoming
/// stream as "stop serving" and, in the graceful case, immediately closes the
/// connections it has already accepted -- so a plain one-item stream would tear
/// down the connection the moment it handed it over. Staying pending afterwards
/// leaves the loop parked on `next()` until the shutdown signal fires, which is
/// the behaviour a real listener would have.
fn once_connected<S>(stream: S) -> impl Stream<Item = io::Result<S>> {
    tokio_stream::once(Ok(stream)).chain(futures::stream::pending())
}

/// A connector that yields one pre-connected socket and then refuses.
///
/// tonic expects to be able to reconnect, so it holds a connector rather than a
/// connection. There is nothing to reconnect to here: if the socket is lost,
/// Buck2 has gone away and the run is over. Returning an error on the second
/// call surfaces that as a channel error rather than as a silent hang.
struct SocketConnector {
    socket: Arc<Mutex<Option<Socket>>>,
}

impl SocketConnector {
    fn new(socket: Socket) -> Self {
        Self {
            socket: Arc::new(Mutex::new(Some(socket))),
        }
    }
}

/// The IO handed back to tonic, wrapped for hyper's traits.
enum SocketIo {
    #[cfg(unix)]
    Unix(TokioIo<tokio::net::UnixStream>),
    Tcp(TokioIo<TcpStream>),
}

impl Service<Uri> for SocketConnector {
    type Response = SocketIo;
    type Error = io::Error;
    type Future = Pin<Box<dyn Future<Output = io::Result<SocketIo>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, _uri: Uri) -> Self::Future {
        let taken = self
            .socket
            .lock()
            .expect("socket mutex is never held across a panic")
            .take();
        Box::pin(async move {
            match taken {
                #[cfg(unix)]
                Some(Socket::Unix(stream)) => Ok(SocketIo::Unix(TokioIo::new(stream))),
                Some(Socket::Tcp(stream)) => Ok(SocketIo::Tcp(TokioIo::new(stream))),
                None => Err(io::Error::new(
                    io::ErrorKind::NotConnected,
                    "the socket Buck2 provided has already been used; \
                     it cannot be reconnected",
                )),
            }
        })
    }
}

/// Forwards hyper's IO traits to whichever socket kind is underneath.
///
/// A macro rather than a `Box<dyn ...>` so the enum stays `Unpin` and the
/// forwarding costs nothing.
macro_rules! delegate_io {
    ($self:ident, $method:ident $(, $arg:expr)*) => {
        match $self.get_mut() {
            #[cfg(unix)]
            SocketIo::Unix(io) => Pin::new(io).$method($($arg),*),
            SocketIo::Tcp(io) => Pin::new(io).$method($($arg),*),
        }
    };
}

impl hyper::rt::Read for SocketIo {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: hyper::rt::ReadBufCursor<'_>,
    ) -> Poll<io::Result<()>> {
        delegate_io!(self, poll_read, cx, buf)
    }
}

impl hyper::rt::Write for SocketIo {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        delegate_io!(self, poll_write, cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        delegate_io!(self, poll_flush, cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        delegate_io!(self, poll_shutdown, cx)
    }
}
