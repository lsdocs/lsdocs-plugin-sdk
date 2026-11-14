//! Byte streams a [`Peer`](crate::Peer) runs over, and how frames are cut
//! from them.
//!
//! Each frame is a 4-byte big-endian length followed by that many bytes of
//! MessagePack. Frames over [`Options::max_frame_len`](crate::Options) are
//! rejected before they are read into memory.
//!
//! - [`stdio`]: for a plugin talking to the host over its stdin and stdout.
//!   It calls [`protect_stdout`] first, so stray prints cannot corrupt the
//!   stream.
//! - [`pipe`]: a named pipe (Windows) or unix socket. The host listens and
//!   passes the name to the plugin in [`pipe::ENV_VAR`].
//! - [`memory`]: an in-process pair, for tests and first-party plugins.

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::io::{AsyncRead, AsyncWrite};
use tokio_util::codec::LengthDelimitedCodec;

/// Default for [`Options::max_frame_len`](crate::Options): 64 MiB.
pub const DEFAULT_MAX_FRAME_LEN: usize = 64 * 1024 * 1024;

/// A boxed reading half.
pub type BoxReader = Box<dyn AsyncRead + Send + Unpin>;
/// A boxed writing half.
pub type BoxWriter = Box<dyn AsyncWrite + Send + Unpin>;

/// The two halves of a byte stream.
pub struct Transport {
    /// Where incoming bytes come from.
    pub reader: BoxReader,
    /// Where outgoing bytes go.
    pub writer: BoxWriter,
}

impl Transport {
    /// Wraps separate reading and writing halves.
    pub fn new(
        reader: impl AsyncRead + Send + Unpin + 'static,
        writer: impl AsyncWrite + Send + Unpin + 'static,
    ) -> Self {
        Self {
            reader: Box::new(reader),
            writer: Box::new(writer),
        }
    }

    /// Splits a bidirectional stream into a transport.
    pub fn from_stream(stream: impl AsyncRead + AsyncWrite + Send + 'static) -> Self {
        let (reader, writer) = tokio::io::split(stream);
        Self::new(reader, writer)
    }
}

impl std::fmt::Debug for Transport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Transport { .. }")
    }
}

/// The frame codec: u32 big-endian length prefix.
pub(crate) fn codec(max_frame_len: usize) -> LengthDelimitedCodec {
    LengthDelimitedCodec::builder()
        .length_field_type::<u32>()
        .max_frame_length(max_frame_len)
        .new_codec()
}

/// Two connected in-memory transports.
pub fn memory() -> (Transport, Transport) {
    let (a, b) = tokio::io::duplex(64 * 1024);
    (Transport::from_stream(a), Transport::from_stream(b))
}

/// A transport over this process's stdin and stdout, for a plugin launched
/// by the host. Calls [`protect_stdout`], so it can be used once per
/// process. Must be called inside a tokio runtime.
pub fn stdio() -> io::Result<Transport> {
    let stdout = tokio::fs::File::from_std(protect_stdout()?);
    Ok(Transport::new(ThreadStdin::spawn()?, stdout))
}

/// Stdin read on a dedicated, detached thread.
///
/// `tokio::io::stdin` reads on tokio's blocking pool, and that read cannot be
/// cancelled, so a plugin that finishes while the host still holds its stdin
/// open would hang at runtime shutdown. A detached thread does not hold up
/// shutdown and simply ends with the process.
struct ThreadStdin {
    rx: tokio::sync::mpsc::Receiver<io::Result<bytes::Bytes>>,
    buf: bytes::Bytes,
}

impl ThreadStdin {
    fn spawn() -> io::Result<Self> {
        use std::io::Read;
        let (tx, rx) = tokio::sync::mpsc::channel(4);
        std::thread::Builder::new()
            .name("lsdocs-stdin".into())
            .spawn(move || {
                let mut stdin = std::io::stdin().lock();
                let mut buf = vec![0u8; 64 * 1024];
                loop {
                    let item = match stdin.read(&mut buf) {
                        // EOF: dropping `tx` ends the stream.
                        Ok(0) => break,
                        Ok(n) => Ok(bytes::Bytes::copy_from_slice(&buf[..n])),
                        Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                        Err(e) => Err(e),
                    };
                    let failed = item.is_err();
                    if tx.blocking_send(item).is_err() || failed {
                        break;
                    }
                }
            })?;
        Ok(Self {
            rx,
            buf: bytes::Bytes::new(),
        })
    }
}

impl AsyncRead for ThreadStdin {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        out: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        use bytes::Buf;
        use std::task::Poll;
        if self.buf.is_empty() {
            match self.rx.poll_recv(cx) {
                Poll::Pending => return Poll::Pending,
                // Reading nothing signals EOF.
                Poll::Ready(None) => return Poll::Ready(Ok(())),
                Poll::Ready(Some(Err(e))) => return Poll::Ready(Err(e)),
                Poll::Ready(Some(Ok(chunk))) => self.buf = chunk,
            }
        }
        let n = out.remaining().min(self.buf.len());
        out.put_slice(&self.buf[..n]);
        self.buf.advance(n);
        Poll::Ready(Ok(()))
    }
}

static STDOUT_TAKEN: AtomicBool = AtomicBool::new(false);

/// Takes this process's real stdout for exclusive use and points stdout at
/// stderr, so later prints (from Rust, C libraries or an embedded Python or
/// Node runtime that writes to the standard handle) go to stderr instead of
/// corrupting the protocol stream.
///
/// Returns the real stdout as a file. Fails if called more than once.
///
/// Language runtimes that cache their own stdout object (e.g. Python's
/// `sys.stdout`) must also be redirected by the binding.
pub fn protect_stdout() -> io::Result<std::fs::File> {
    if STDOUT_TAKEN.swap(true, Ordering::SeqCst) {
        return Err(io::Error::other("stdout has already been taken"));
    }
    use std::io::Write;
    // Anything already buffered belongs on the original stream.
    let _ = std::io::stdout().flush();
    sys::protect_stdout()
}

#[cfg(unix)]
#[allow(unsafe_code)]
mod sys {
    use std::fs::File;
    use std::io;
    use std::os::fd::FromRawFd;

    pub fn protect_stdout() -> io::Result<File> {
        // SAFETY: plain fd syscalls; the duplicated fd is owned by the
        // returned File and nothing else.
        unsafe {
            let real = libc::fcntl(libc::STDOUT_FILENO, libc::F_DUPFD_CLOEXEC, 3);
            if real < 0 {
                return Err(io::Error::last_os_error());
            }
            if libc::dup2(libc::STDERR_FILENO, libc::STDOUT_FILENO) < 0 {
                let e = io::Error::last_os_error();
                libc::close(real);
                return Err(e);
            }
            Ok(File::from_raw_fd(real))
        }
    }
}

#[cfg(windows)]
#[allow(unsafe_code)]
mod sys {
    use std::fs::File;
    use std::io;
    use std::os::windows::io::FromRawHandle;

    use windows_sys::Win32::Foundation::{
        CloseHandle, DUPLICATE_SAME_ACCESS, DuplicateHandle, HANDLE, INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::System::Console::{
        GetStdHandle, STD_ERROR_HANDLE, STD_OUTPUT_HANDLE, SetStdHandle,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    pub fn protect_stdout() -> io::Result<File> {
        // SAFETY: Win32 handle calls on this process's own standard handles.
        // The duplicated handle is owned by the returned File and nothing
        // else.
        unsafe {
            let out = GetStdHandle(STD_OUTPUT_HANDLE);
            if out.is_null() || out == INVALID_HANDLE_VALUE {
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    "process has no stdout",
                ));
            }
            let mut real: HANDLE = std::ptr::null_mut();
            let process = GetCurrentProcess();
            if DuplicateHandle(
                process,
                out,
                process,
                &mut real,
                0,
                0,
                DUPLICATE_SAME_ACCESS,
            ) == 0
            {
                return Err(io::Error::last_os_error());
            }
            // Win32 callers, including Rust's std, look the handle up on
            // every write.
            if SetStdHandle(STD_OUTPUT_HANDLE, GetStdHandle(STD_ERROR_HANDLE)) == 0 {
                let e = io::Error::last_os_error();
                CloseHandle(real);
                return Err(e);
            }
            // The C runtime's fd 1 (used by printf and embedded runtimes)
            // keeps its own copy. This fails harmlessly when the CRT has no
            // stdio fds, e.g. with no console.
            libc::dup2(2, 1);
            Ok(File::from_raw_handle(real as _))
        }
    }
}

/// Named pipe (Windows) or unix socket transports.
///
/// The host binds a [`Listener`] to a name from [`unique_name`], launches the
/// plugin with that name in the [`ENV_VAR`] environment variable, and
/// accepts one connection. The plugin calls [`connect_from_env`].
pub mod pipe {
    use std::io;
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::Transport;

    /// Environment variable holding the pipe name for the plugin.
    pub const ENV_VAR: &str = "LSDOCS_PIPE";

    /// A name no other listener in this process uses.
    pub fn unique_name() -> String {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let id = format!(
            "lsdocs-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        #[cfg(windows)]
        {
            format!(r"\\.\pipe\{id}")
        }
        #[cfg(unix)]
        {
            std::env::temp_dir()
                .join(format!("{id}.sock"))
                .to_string_lossy()
                .into_owned()
        }
    }

    /// Waits for one plugin to connect.
    #[derive(Debug)]
    pub struct Listener {
        #[cfg(windows)]
        server: tokio::net::windows::named_pipe::NamedPipeServer,
        #[cfg(unix)]
        listener: tokio::net::UnixListener,
        #[cfg(unix)]
        path: std::path::PathBuf,
    }

    impl Listener {
        /// Creates the pipe or socket. Must be called inside a tokio runtime.
        pub fn bind(name: &str) -> io::Result<Self> {
            #[cfg(windows)]
            {
                let server = tokio::net::windows::named_pipe::ServerOptions::new()
                    .first_pipe_instance(true)
                    .create(name)?;
                Ok(Self { server })
            }
            #[cfg(unix)]
            {
                let listener = tokio::net::UnixListener::bind(name)?;
                Ok(Self {
                    listener,
                    path: name.into(),
                })
            }
        }

        /// Waits for the plugin to connect.
        pub async fn accept(self) -> io::Result<Transport> {
            #[cfg(windows)]
            {
                // No Drop impl on Windows, so the server can be moved out.
                self.server.connect().await?;
                Ok(Transport::from_stream(self.server))
            }
            #[cfg(unix)]
            {
                let (stream, _) = self.listener.accept().await?;
                Ok(Transport::from_stream(stream))
            }
        }
    }

    #[cfg(unix)]
    impl Drop for Listener {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.path);
        }
    }

    /// Connects to a listener.
    pub async fn connect(name: &str) -> io::Result<Transport> {
        #[cfg(windows)]
        {
            use std::time::Duration;
            use tokio::net::windows::named_pipe::ClientOptions;
            // ERROR_PIPE_BUSY: another client is mid-connect; retry briefly.
            const ERROR_PIPE_BUSY: i32 = 231;
            let mut attempts = 0;
            loop {
                match ClientOptions::new().open(name) {
                    Ok(client) => return Ok(Transport::from_stream(client)),
                    Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY) && attempts < 100 => {
                        attempts += 1;
                        tokio::time::sleep(Duration::from_millis(20)).await;
                    }
                    Err(e) => return Err(e),
                }
            }
        }
        #[cfg(unix)]
        {
            Ok(Transport::from_stream(
                tokio::net::UnixStream::connect(name).await?,
            ))
        }
    }

    /// Connects to the listener named in [`ENV_VAR`].
    pub async fn connect_from_env() -> io::Result<Transport> {
        let name = std::env::var(ENV_VAR).map_err(|_| {
            io::Error::new(io::ErrorKind::NotFound, format!("{ENV_VAR} is not set"))
        })?;
        connect(&name).await
    }
}
