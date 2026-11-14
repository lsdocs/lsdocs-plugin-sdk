//! The symmetric RPC endpoint.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use lsdocs_protocol::{CancelParams, Notification, Request, msgpack, notification::Cancel};
use serde::{Serialize, de::DeserializeOwned};
use tokio::sync::{Notify, Semaphore, mpsc, oneshot};
use tokio::task::{AbortHandle, JoinSet};
use tokio_util::codec::{FramedRead, FramedWrite, LengthDelimitedCodec};

use crate::message::{self, CodecError, Message};
use crate::transport::{self, BoxWriter, Transport};
use crate::{Error, ErrorCode, RpcError};

type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;
type RequestHandler =
    Arc<dyn Fn(Bytes, Peer) -> BoxFuture<Result<Vec<u8>, RpcError>> + Send + Sync>;
type NotificationHandler = Arc<dyn Fn(Bytes, &Peer) + Send + Sync>;

/// Settings for a connection.
#[derive(Debug, Clone)]
pub struct Options {
    /// Largest frame accepted or sent, in bytes.
    pub max_frame_len: usize,
    /// How long an outgoing request waits for its response before failing
    /// with [`Error::Timeout`] and being cancelled. `None` waits forever.
    pub request_timeout: Option<Duration>,
    /// Most incoming requests handled at once; further ones wait. `None` is
    /// unlimited. Notifications are not limited.
    pub max_concurrency: Option<usize>,
    /// Outgoing frames queued before senders wait.
    pub send_queue: usize,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            max_frame_len: transport::DEFAULT_MAX_FRAME_LEN,
            request_timeout: None,
            max_concurrency: None,
            send_queue: 64,
        }
    }
}

/// Handlers for incoming requests and notifications, keyed by method.
///
/// A request with no handler is answered with
/// [`ErrorCode::METHOD_NOT_FOUND`]; a notification with no handler is
/// ignored. `$/cancel` is handled by the connection itself.
#[derive(Clone, Default)]
pub struct Router {
    requests: HashMap<String, RequestHandler>,
    notifications: HashMap<String, NotificationHandler>,
}

impl std::fmt::Debug for Router {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Router")
            .field("requests", &self.requests.keys().collect::<Vec<_>>())
            .field(
                "notifications",
                &self.notifications.keys().collect::<Vec<_>>(),
            )
            .finish()
    }
}

impl Router {
    /// An empty router.
    pub fn new() -> Self {
        Self::default()
    }

    /// Handles request `R`. The handler gets the decoded params and a
    /// [`Peer`] for calling back the other side. Params that fail to decode
    /// are answered with [`ErrorCode::INVALID_PARAMS`] without calling it.
    pub fn request<R, F, Fut>(mut self, handler: F) -> Self
    where
        R: Request,
        R::Params: Send + 'static,
        F: Fn(R::Params, Peer) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<R::Result, RpcError>> + Send + 'static,
    {
        let handler: RequestHandler = Arc::new(
            move |raw: Bytes, peer: Peer| match msgpack::decode::<R::Params>(&raw) {
                Ok(params) => {
                    let fut = handler(params, peer);
                    Box::pin(async move {
                        let result = fut.await?;
                        msgpack::encode(&result).map_err(|e| {
                            RpcError::internal(format!("failed to encode result: {e}"))
                        })
                    }) as BoxFuture<_>
                }
                Err(e) => Box::pin(std::future::ready(Err(RpcError::new(
                    ErrorCode::INVALID_PARAMS,
                    format!("invalid params for `{}`: {e}", R::METHOD),
                )))),
            },
        );
        self.requests.insert(R::METHOD.to_owned(), handler);
        self
    }

    /// Handles notification `N`. Notification handlers run one at a time, in
    /// arrival order, on the connection's task, so they should be quick;
    /// spawn a task for slow work. Params that fail to decode are logged and
    /// dropped.
    pub fn notification<N, F>(mut self, handler: F) -> Self
    where
        N: Notification,
        F: Fn(N::Params, &Peer) + Send + Sync + 'static,
    {
        let handler: NotificationHandler =
            Arc::new(
                move |raw: Bytes, peer: &Peer| match msgpack::decode::<N::Params>(&raw) {
                    Ok(params) => handler(params, peer),
                    Err(e) => tracing::warn!(
                        method = N::METHOD,
                        "dropping notification with invalid params: {e}"
                    ),
                },
            );
        self.notifications.insert(N::METHOD.to_owned(), handler);
        self
    }
}

/// One end of a connection. Cheap to clone; all clones share the
/// connection.
#[derive(Clone)]
pub struct Peer {
    inner: Arc<Inner>,
}

struct Inner {
    /// Queue to the writer task; `None` once the connection is shutting down.
    tx: Mutex<Option<mpsc::Sender<Bytes>>>,
    /// Outgoing requests awaiting a response.
    pending: Mutex<HashMap<u32, oneshot::Sender<Result<Bytes, RpcError>>>>,
    next_id: AtomicU32,
    closing: AtomicBool,
    close: Notify,
    options: Options,
}

impl std::fmt::Debug for Peer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Peer")
            .field("closing", &self.inner.closing.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

/// Creates a connection over `transport`. Returns the [`Peer`] for sending
/// and the [`Connection`] that must be run (usually spawned) for anything to
/// happen.
pub fn connect(transport: Transport, router: Router, options: Options) -> (Peer, Connection) {
    let (tx, rx) = mpsc::channel(options.send_queue.max(1));
    let peer = Peer {
        inner: Arc::new(Inner {
            tx: Mutex::new(Some(tx)),
            pending: Mutex::new(HashMap::new()),
            next_id: AtomicU32::new(1),
            closing: AtomicBool::new(false),
            close: Notify::new(),
            options,
        }),
    };
    let connection = Connection {
        peer: peer.clone(),
        transport,
        router,
        rx,
    };
    (peer, connection)
}

impl Peer {
    /// Sends request `R` and waits for its result.
    pub async fn request<R: Request>(&self, params: &R::Params) -> Result<R::Result, Error> {
        self.call(R::METHOD, params).await
    }

    /// Sends a request by method name and waits for its result.
    ///
    /// If the returned future is dropped before the response arrives, or
    /// [`Options::request_timeout`] passes, `$/cancel` is sent for it.
    pub async fn call<P, T>(&self, method: &str, params: &P) -> Result<T, Error>
    where
        P: Serialize + ?Sized,
        T: DeserializeOwned,
    {
        let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        let frame = message::encode_request(id, method, params)?;
        let (tx, rx) = oneshot::channel();
        self.inner.pending.lock().expect("lock").insert(id, tx);
        let mut guard = CancelOnDrop {
            peer: self,
            id,
            armed: true,
        };
        if self.inner.closing.load(Ordering::SeqCst) {
            return Err(Error::Closed);
        }
        if let Err(e) = self.send(frame).await {
            // Never sent, so there is nothing to cancel.
            guard.armed = false;
            return Err(e);
        }
        let response = match self.inner.options.request_timeout {
            Some(limit) => match tokio::time::timeout(limit, rx).await {
                Ok(response) => response,
                Err(_) => return Err(Error::Timeout),
            },
            None => rx.await,
        };
        guard.armed = false;
        let bytes = response
            .map_err(|_| Error::Closed)?
            .map_err(Error::Remote)?;
        msgpack::decode(&bytes)
            .map_err(|e| Error::Codec(CodecError(format!("invalid result for `{method}`: {e}"))))
    }

    /// Sends notification `N`.
    pub async fn notify<N: Notification>(&self, params: &N::Params) -> Result<(), Error> {
        self.notify_raw(N::METHOD, params).await
    }

    /// Sends a notification by method name.
    pub async fn notify_raw<P: Serialize + ?Sized>(
        &self,
        method: &str,
        params: &P,
    ) -> Result<(), Error> {
        self.send(message::encode_notification(method, params)?)
            .await
    }

    /// Asks the connection to stop reading and shut down gracefully. In-flight
    /// incoming requests still finish and are answered; outgoing requests
    /// still waiting fail with [`Error::Closed`].
    pub fn close(&self) {
        self.inner.closing.store(true, Ordering::SeqCst);
        self.inner.close.notify_one();
    }

    /// Whether the connection has started shutting down.
    pub fn is_closed(&self) -> bool {
        self.inner.closing.load(Ordering::SeqCst)
    }

    async fn send(&self, frame: Bytes) -> Result<(), Error> {
        let limit = self.inner.options.max_frame_len;
        if frame.len() > limit {
            return Err(Error::Codec(CodecError(format!(
                "message of {} bytes exceeds the {limit} byte frame limit",
                frame.len()
            ))));
        }
        let tx = self
            .inner
            .tx
            .lock()
            .expect("lock")
            .clone()
            .ok_or(Error::Closed)?;
        tx.send(frame).await.map_err(|_| Error::Closed)
    }

    fn try_send(&self, frame: Bytes) {
        if let Some(tx) = self.inner.tx.lock().expect("lock").as_ref() {
            let _ = tx.try_send(frame);
        }
    }
}

/// Cancels an outgoing request that is abandoned before its response.
struct CancelOnDrop<'a> {
    peer: &'a Peer,
    id: u32,
    armed: bool,
}

impl Drop for CancelOnDrop<'_> {
    fn drop(&mut self) {
        let was_pending = self
            .peer
            .inner
            .pending
            .lock()
            .expect("lock")
            .remove(&self.id)
            .is_some();
        if self.armed
            && was_pending
            && let Ok(frame) =
                message::encode_notification(Cancel::METHOD, &CancelParams { id: self.id })
        {
            self.peer.try_send(frame);
        }
    }
}

/// Drives a connection: reads and dispatches incoming messages and writes
/// outgoing ones. Created by [`connect`].
pub struct Connection {
    peer: Peer,
    transport: Transport,
    router: Router,
    rx: mpsc::Receiver<Bytes>,
}

impl std::fmt::Debug for Connection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Connection").finish_non_exhaustive()
    }
}

/// Bookkeeping for incoming requests being handled.
struct Running {
    tasks: JoinSet<()>,
    by_request: HashMap<u32, AbortHandle>,
    by_task: HashMap<tokio::task::Id, u32>,
}

impl Connection {
    /// Runs until the other side closes the stream, the transport fails, or
    /// [`Peer::close`] is called. Then waits for in-flight incoming requests
    /// to finish and flushes everything queued before returning.
    pub async fn run(self) -> Result<(), Error> {
        let Connection {
            peer,
            transport,
            router,
            rx,
        } = self;
        let options = peer.inner.options.clone();
        let mut reader = FramedRead::new(transport.reader, transport::codec(options.max_frame_len));
        let writer = FramedWrite::new(transport.writer, transport::codec(options.max_frame_len));
        let write_task = tokio::spawn(write_loop(writer, rx, peer.clone()));
        let limit = options
            .max_concurrency
            .map(|n| Arc::new(Semaphore::new(n.max(1))));
        let mut running = Running {
            tasks: JoinSet::new(),
            by_request: HashMap::new(),
            by_task: HashMap::new(),
        };

        let read_result = loop {
            tokio::select! {
                frame = reader.next() => match frame {
                    None => break Ok(()),
                    Some(Err(e)) => break Err(e),
                    Some(Ok(frame)) => {
                        let frame = frame.freeze();
                        crate::trace::frame("recv", &frame);
                        dispatch(frame, &peer, &router, &limit, &mut running).await;
                    }
                },
                Some(joined) = running.tasks.join_next_with_id(), if !running.tasks.is_empty() => {
                    let task = match joined {
                        Ok((task, ())) => task,
                        Err(e) => e.id(),
                    };
                    if let Some(id) = running.by_task.remove(&task) {
                        running.by_request.remove(&id);
                    }
                }
                () = peer.inner.close.notified() => break Ok(()),
            }
        };

        // Stop new outgoing requests, then fail the ones still waiting.
        peer.inner.closing.store(true, Ordering::SeqCst);
        peer.inner.pending.lock().expect("lock").clear();
        // Let in-flight handlers finish and queue their responses.
        while running.tasks.join_next().await.is_some() {}
        // Closing the queue lets the writer drain it and shut down.
        drop(peer.inner.tx.lock().expect("lock").take());
        let write_result = write_task
            .await
            .map_err(|e| Error::Io(std::io::Error::other(e)))?;

        read_result.map_err(Error::Io)?;
        write_result.map_err(Error::Io)
    }
}

async fn dispatch(
    frame: Bytes,
    peer: &Peer,
    router: &Router,
    limit: &Option<Arc<Semaphore>>,
    running: &mut Running,
) {
    let message = match message::decode(frame.clone()) {
        Ok(message) => message,
        Err(e) => {
            tracing::warn!("dropping malformed message: {e}");
            if let Some(id) = message::peek_request_id(&frame) {
                let error = RpcError::new(ErrorCode::INVALID_REQUEST, e.to_string());
                let _ = peer.send(message::encode_error(id, &error)).await;
            }
            return;
        }
    };
    match message {
        Message::Request { id, method, params } => {
            let Some(handler) = router.requests.get(&method) else {
                let error = RpcError::new(
                    ErrorCode::METHOD_NOT_FOUND,
                    format!("no handler for `{method}`"),
                );
                let _ = peer.send(message::encode_error(id, &error)).await;
                return;
            };
            let fut = handler(params, peer.clone());
            let responder = peer.clone();
            let limit = limit.clone();
            let handle = running.tasks.spawn(async move {
                let _permit = match limit {
                    Some(limit) => Some(
                        limit
                            .acquire_owned()
                            .await
                            .expect("semaphore is never closed"),
                    ),
                    None => None,
                };
                let frame = match fut.await {
                    Ok(result) => message::encode_response_raw(id, &result),
                    Err(error) => message::encode_error(id, &error),
                };
                if let Err(Error::Codec(e)) = responder.send(frame).await {
                    // The result was too large to send; say so instead.
                    let error = RpcError::internal(e.to_string());
                    let _ = responder.send(message::encode_error(id, &error)).await;
                }
            });
            running.by_task.insert(handle.id(), id);
            running.by_request.insert(id, handle);
        }
        Message::Notification { method, params } if method == Cancel::METHOD => {
            let Ok(CancelParams { id }) = msgpack::decode(&params) else {
                tracing::warn!("dropping `$/cancel` with invalid params");
                return;
            };
            if let Some(handle) = running.by_request.remove(&id) {
                handle.abort();
                let error = RpcError::new(ErrorCode::REQUEST_CANCELLED, "request cancelled");
                let _ = peer.send(message::encode_error(id, &error)).await;
            }
        }
        Message::Notification { method, params } => match router.notifications.get(&method) {
            Some(handler) => handler(params, peer),
            None => tracing::debug!(%method, "ignoring notification with no handler"),
        },
        Message::Response { id, result } => {
            let waiter = peer.inner.pending.lock().expect("lock").remove(&id);
            match waiter {
                Some(waiter) => {
                    let _ = waiter.send(result);
                }
                None => tracing::debug!(id, "ignoring response to unknown or abandoned request"),
            }
        }
    }
}

/// Writes queued frames until the queue closes. If writing fails, the whole
/// connection is closed so nothing waits on a response that cannot come.
async fn write_loop(
    mut sink: FramedWrite<BoxWriter, LengthDelimitedCodec>,
    mut rx: mpsc::Receiver<Bytes>,
    peer: Peer,
) -> std::io::Result<()> {
    let result = async {
        while let Some(frame) = rx.recv().await {
            crate::trace::frame("send", &frame);
            sink.send(frame).await?;
        }
        SinkExt::<Bytes>::close(&mut sink).await
    }
    .await;
    if result.is_err() {
        peer.close();
    }
    result
}
