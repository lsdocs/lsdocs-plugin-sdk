//! One test suite, run over every transport: in-memory, a child process on
//! stdio, and a child process on a named pipe / unix socket.
//!
//! The child is the `echo` example. `cargo test` builds examples, but a
//! filtered run (`cargo test --test suite`) does not; build them first with
//! `cargo build -p lsdocs-rpc --examples`.

#[path = "../examples/echo/router.rs"]
mod router;

use std::future::Future;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::{Duration, Instant};

use lsdocs_rpc::transport::{self, Transport, pipe};
use lsdocs_rpc::{Error, ErrorCode, Options, Peer, connect};
use router::*;
use serde_json::{Value, json};
use tokio::process::{Child, Command};
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};
use tokio::task::JoinHandle;

/// The test side of a connection to an echo peer.
struct Harness {
    peer: Peer,
    notified: UnboundedReceiver<String>,
    connection: JoinHandle<Result<(), Error>>,
    /// The echo peer's own connection task (in-memory only).
    server: Option<JoinHandle<Result<(), Error>>>,
    child: Option<Child>,
}

impl Harness {
    fn new(transport: Transport, options: Options) -> Self {
        let (tx, notified) = unbounded_channel();
        let (peer, connection) = connect(transport, router::router(Some(tx)), options);
        Self {
            peer,
            notified,
            connection: tokio::spawn(connection.run()),
            server: None,
            child: None,
        }
    }

    async fn stats(&self) -> Stats {
        self.peer.request::<GetStats>(&()).await.unwrap()
    }

    /// Polls stats until `pred` holds, failing after two seconds.
    async fn wait_for_stats(&self, what: &str, pred: impl Fn(Stats) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let stats = self.stats().await;
            if pred(stats) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {what}: {stats:?}"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}

/// Finds the built `echo` example in `target/<profile>/examples`, searching
/// upward from this test binary (whose location depends on cargo's
/// build-dir layout).
fn echo_bin() -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    let name = format!("echo{}", std::env::consts::EXE_SUFFIX);
    exe.ancestors()
        .map(|dir| dir.join("examples").join(&name))
        .find(|path| path.is_file())
        .unwrap_or_else(|| {
            panic!(
                "no examples/{name} above {}; run `cargo build -p lsdocs-rpc --examples`",
                exe.display()
            )
        })
}

async fn memory(options: Options) -> Harness {
    let (client, server) = transport::memory();
    let (_server_peer, server_connection) =
        connect(server, router::router(None), Options::default());
    let mut harness = Harness::new(client, options);
    harness.server = Some(tokio::spawn(server_connection.run()));
    harness
}

async fn stdio(options: Options) -> Harness {
    let mut child = Command::new(echo_bin())
        .arg("stdio")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let transport = Transport::new(child.stdout.take().unwrap(), child.stdin.take().unwrap());
    let mut harness = Harness::new(transport, options);
    harness.child = Some(child);
    harness
}

async fn pipe(options: Options) -> Harness {
    let name = pipe::unique_name();
    let listener = pipe::Listener::bind(&name).unwrap();
    let child = Command::new(echo_bin())
        .arg("pipe")
        .env(pipe::ENV_VAR, &name)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let transport = tokio::time::timeout(Duration::from_secs(10), listener.accept())
        .await
        .expect("echo child did not connect")
        .unwrap();
    let mut harness = Harness::new(transport, options);
    harness.child = Some(child);
    harness
}

fn remote_code(result: Result<impl std::fmt::Debug, Error>) -> ErrorCode {
    match result {
        Err(Error::Remote(e)) => e.code,
        other => panic!("expected a remote error, got {other:?}"),
    }
}

/// Runs every case against harnesses from `make`.
async fn run_suite<F, Fut>(make: F)
where
    F: Fn(Options) -> Fut,
    Fut: Future<Output = Harness>,
{
    let mut h = make(Options::default()).await;
    let p = &h.peer;

    // Echo round trip with nested values and non-ASCII text.
    let value = json!({"a": [1, 2, {"b": null}], "s": "✓ ünïcode", "n": -7});
    assert_eq!(p.request::<Echo>(&value).await.unwrap(), value);

    // Unit params and result.
    p.request::<Ping>(&()).await.unwrap();

    // Concurrent requests complete independently, out of order.
    let start = Instant::now();
    let slow = async {
        let tag = p
            .request::<Sleep>(&SleepParams {
                ms: 400,
                tag: "slow".into(),
            })
            .await
            .unwrap();
        (tag, start.elapsed())
    };
    let fast = async {
        let tag = p
            .request::<Sleep>(&SleepParams {
                ms: 20,
                tag: "fast".into(),
            })
            .await
            .unwrap();
        (tag, start.elapsed())
    };
    let ((slow_tag, slow_at), (fast_tag, fast_at)) = tokio::join!(slow, fast);
    assert_eq!((slow_tag.as_str(), fast_tag.as_str()), ("slow", "fast"));
    assert!(
        fast_at < slow_at,
        "fast {fast_at:?} should finish before slow {slow_at:?}"
    );

    // Errors: unknown method, bad params, handler failure.
    assert_eq!(
        remote_code(p.call::<_, Value>("test/nope", &()).await),
        ErrorCode::METHOD_NOT_FOUND
    );
    assert_eq!(
        remote_code(p.call::<_, u32>("test/double", "not a number").await),
        ErrorCode::INVALID_PARAMS
    );
    match p.request::<Fail>(&"boom".to_string()).await {
        Err(Error::Remote(e)) => {
            assert_eq!(e.code, ErrorCode::INTERNAL_ERROR);
            assert_eq!(e.message, "boom");
        }
        other => panic!("expected handler error, got {other:?}"),
    }
    assert_eq!(p.request::<Double>(&21).await.unwrap(), 42);

    // The other side can call back while handling our request.
    let value = json!({"via": "call_back"});
    assert_eq!(p.request::<CallBack>(&value).await.unwrap(), value);

    // Notifications both ways.
    p.notify::<NotifyMe>(&"hello".to_string()).await.unwrap();
    let got = tokio::time::timeout(Duration::from_secs(2), h.notified.recv()).await;
    assert_eq!(got.unwrap().as_deref(), Some("hello"));

    // A large message (4 MiB).
    let big = json!("x".repeat(4 * 1024 * 1024));
    assert_eq!(p.request::<Echo>(&big).await.unwrap(), big);

    // Dropping a request future cancels it on the other side.
    let before = h.stats().await;
    let abandoned = tokio::time::timeout(
        Duration::from_millis(100),
        p.request::<Sleep>(&SleepParams {
            ms: 10_000,
            tag: "abandoned".into(),
        }),
    )
    .await;
    assert!(abandoned.is_err(), "the sleep should not have finished");
    h.wait_for_stats("cancellation", |s| s.cancelled == before.cancelled + 1)
        .await;

    // Closing: the connection drains and ends cleanly, then calls fail.
    h.peer.close();
    finish(&mut h).await;
    assert!(matches!(
        h.peer.request::<Ping>(&()).await,
        Err(Error::Closed)
    ));

    // Request timeout and frame limit options.
    let mut h = make(Options {
        request_timeout: Some(Duration::from_millis(100)),
        max_frame_len: 64 * 1024,
        ..Options::default()
    })
    .await;
    let before = h.stats().await;
    let timed_out = h
        .peer
        .request::<Sleep>(&SleepParams {
            ms: 10_000,
            tag: "late".into(),
        })
        .await;
    assert!(matches!(timed_out, Err(Error::Timeout)), "{timed_out:?}");
    h.wait_for_stats("timeout cancellation", |s| {
        s.cancelled == before.cancelled + 1
    })
    .await;
    let too_big = h.peer.request::<Echo>(&json!("x".repeat(128 * 1024))).await;
    assert!(matches!(too_big, Err(Error::Codec(_))), "{too_big:?}");
    // The connection is still usable afterwards.
    h.peer.request::<Ping>(&()).await.unwrap();
    h.peer.close();
    finish(&mut h).await;
}

/// Waits for both ends of a closed harness to finish cleanly.
async fn finish(h: &mut Harness) {
    let limit = Duration::from_secs(5);
    tokio::time::timeout(limit, &mut h.connection)
        .await
        .expect("connection did not finish")
        .unwrap()
        .unwrap();
    if let Some(server) = h.server.take() {
        tokio::time::timeout(limit, server)
            .await
            .expect("server connection did not finish")
            .unwrap()
            .unwrap();
    }
    if let Some(mut child) = h.child.take() {
        let status = tokio::time::timeout(limit, child.wait())
            .await
            .expect("echo child did not exit")
            .unwrap();
        let mut stderr = String::new();
        if let Some(mut err) = child.stderr.take() {
            use tokio::io::AsyncReadExt;
            err.read_to_string(&mut stderr).await.unwrap();
        }
        assert!(status.success(), "echo child failed: {status}\n{stderr}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn memory_transport() {
    run_suite(memory).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn stdio_transport() {
    run_suite(stdio).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn pipe_transport() {
    run_suite(pipe).await;
}

/// A stdio peer that ends its own connection exits even though its stdin is
/// still open (a blocking stdin read must not hold up shutdown).
#[tokio::test(flavor = "multi_thread")]
async fn stdio_peer_exits_while_stdin_is_open() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    // Raw frames rather than a Peer: a Peer would close the child's stdin as
    // soon as the child closed its side, which is exactly what this test
    // must not do.
    let mut child = Command::new(echo_bin())
        .arg("stdio")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = child.stdout.take().unwrap();

    let request = lsdocs_rpc::message::encode_request(1, "test/close_me", &()).unwrap();
    stdin.write_u32(request.len() as u32).await.unwrap();
    stdin.write_all(&request).await.unwrap();
    stdin.flush().await.unwrap();

    let len = stdout.read_u32().await.unwrap();
    let mut response = vec![0; len as usize];
    stdout.read_exact(&mut response).await.unwrap();
    assert!(matches!(
        lsdocs_rpc::message::decode(response.into()).unwrap(),
        lsdocs_rpc::message::Message::Response {
            id: 1,
            result: Ok(_)
        }
    ));

    // `stdin` is still open here.
    let status = tokio::time::timeout(Duration::from_secs(5), child.wait())
        .await
        .expect("echo child hung after closing its connection while stdin was open")
        .unwrap();
    assert!(status.success());
    drop(stdin);
}

/// With a concurrency limit, incoming requests beyond it wait their turn.
#[tokio::test(flavor = "multi_thread")]
async fn concurrency_limit_queues_requests() {
    let (client, server) = transport::memory();
    let (_server_peer, server_connection) = connect(
        server,
        router::router(None),
        Options {
            max_concurrency: Some(1),
            ..Options::default()
        },
    );
    tokio::spawn(server_connection.run());
    let h = Harness::new(client, Options::default());
    let (pa, pb) = (
        SleepParams {
            ms: 150,
            tag: "a".into(),
        },
        SleepParams {
            ms: 150,
            tag: "b".into(),
        },
    );
    let start = Instant::now();
    let (a, b) = tokio::join!(h.peer.request::<Sleep>(&pa), h.peer.request::<Sleep>(&pb));
    a.unwrap();
    b.unwrap();
    let elapsed = start.elapsed();
    assert!(
        elapsed >= Duration::from_millis(300),
        "two 150 ms requests overlapped despite the limit: {elapsed:?}"
    );
}

/// With stdio, a stray print after taking the transport goes to stderr and
/// does not corrupt the stream.
#[tokio::test(flavor = "multi_thread")]
async fn stdio_stray_prints_go_to_stderr() {
    let mut h = stdio(Options::default()).await;
    h.peer.request::<Ping>(&()).await.unwrap();
    h.peer.close();
    let child = h.child.as_mut().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    tokio::time::timeout(Duration::from_secs(5), &mut h.connection)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let status = child.wait().await.unwrap();
    let mut text = String::new();
    use tokio::io::AsyncReadExt;
    stderr.read_to_string(&mut text).await.unwrap();
    assert!(status.success(), "{text}");
    assert!(
        text.contains("echo: stray stdout line"),
        "stderr was: {text:?}"
    );
}
