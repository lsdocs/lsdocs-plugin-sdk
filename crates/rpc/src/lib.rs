//! MessagePack RPC envelope, framing, codec, peer and transports.
//!
//! - [`message`]: the msgpack-rpc envelope (`[0, id, method, params]` etc.).
//! - [`transport`]: length-prefixed framing and the stdio, pipe and in-memory
//!   transports, including stdout protection for plugins.
//! - [`Peer`] and [`Connection`]: a symmetric endpoint. Either side can send
//!   requests and notifications; incoming ones are dispatched through a
//!   [`Router`].
//!
//! The wire format is specified in `spec/wire.md`.
//!
//! ```no_run
//! # async fn demo() -> Result<(), lsdocs_rpc::Error> {
//! use lsdocs_protocol::{notification, request};
//! use lsdocs_rpc::{Options, Router, transport};
//!
//! let router = Router::new()
//!     .notification::<notification::Log, _>(|params, _peer| {
//!         eprintln!("plugin says: {}", params.message);
//!     });
//! let (peer, connection) = lsdocs_rpc::connect(transport::memory().0, router, Options::default());
//! tokio::spawn(connection.run());
//! peer.request::<request::Shutdown>(&()).await?;
//! # Ok(()) }
//! ```

use std::fmt;

use serde::{Deserialize, Serialize};

pub mod message;
mod peer;
pub mod trace;
pub mod transport;

pub use peer::{Connection, Options, Peer, Router, connect};

/// Error codes used in [`RpcError::code`]. They follow JSON-RPC and LSP.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ErrorCode(pub i32);

impl ErrorCode {
    /// The message could not be decoded.
    pub const PARSE_ERROR: Self = Self(-32700);
    /// The message is not a valid request.
    pub const INVALID_REQUEST: Self = Self(-32600);
    /// The receiver has no handler for the method.
    pub const METHOD_NOT_FOUND: Self = Self(-32601);
    /// The params could not be decoded into the method's params type.
    pub const INVALID_PARAMS: Self = Self(-32602);
    /// The handler failed for a reason of its own.
    pub const INTERNAL_ERROR: Self = Self(-32603);
    /// The request was cancelled with `$/cancel`.
    pub const REQUEST_CANCELLED: Self = Self(-32800);
}

/// An error sent in a response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RpcError {
    /// Kind of error; see [`ErrorCode`].
    pub code: ErrorCode,
    /// Human-readable description.
    pub message: String,
    /// Extra structured detail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

impl RpcError {
    /// Creates an error without extra data.
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }

    /// An [`ErrorCode::INTERNAL_ERROR`] with the given message.
    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::INTERNAL_ERROR, message)
    }
}

impl fmt::Display for RpcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} (code {})", self.message, self.code.0)
    }
}

impl std::error::Error for RpcError {}

/// Why a call on a [`Peer`] failed.
#[derive(Debug)]
pub enum Error {
    /// The other side answered with an error.
    Remote(RpcError),
    /// The connection is closed, or closed before a response arrived.
    Closed,
    /// No response arrived within [`Options::request_timeout`]. The request
    /// was cancelled.
    Timeout,
    /// The params could not be encoded or the result could not be decoded.
    Codec(message::CodecError),
    /// Reading or writing the transport failed.
    Io(std::io::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Remote(e) => write!(f, "remote error: {e}"),
            Error::Closed => f.write_str("connection closed"),
            Error::Timeout => f.write_str("request timed out"),
            Error::Codec(e) => write!(f, "codec error: {e}"),
            Error::Io(e) => write!(f, "transport error: {e}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Remote(e) => Some(e),
            Error::Codec(e) => Some(e),
            Error::Io(e) => Some(e),
            Error::Closed | Error::Timeout => None,
        }
    }
}

impl From<message::CodecError> for Error {
    fn from(e: message::CodecError) -> Self {
        Error::Codec(e)
    }
}
