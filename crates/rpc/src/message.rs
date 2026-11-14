//! The msgpack-rpc envelope.
//!
//! Every frame holds one message, a MessagePack array:
//!
//! | Message      | Shape                                 |
//! |--------------|---------------------------------------|
//! | request      | `[0, id, method, params]`             |
//! | response     | `[1, id, error, result]`              |
//! | notification | `[2, method, params]`                 |
//!
//! `params` is an array of arguments, as msgpack-rpc specifies. lsdocs
//! methods take one argument (the params struct), or none when the params
//! type is `()`. A response has a nil `error` and a `result` on success, or an
//! [`RpcError`] map and a nil `result` on failure.
//!
//! Decoding does not decode the params or result: they are handed on as raw
//! MessagePack so the receiver can decode straight into its typed value.

use bytes::{BufMut, Bytes, BytesMut};
use rmp::Marker;
use serde::{Deserialize, Serialize};

use crate::RpcError;

const REQUEST: u8 = 0;
const RESPONSE: u8 = 1;
const NOTIFICATION: u8 = 2;
/// MessagePack `nil`.
const NIL: &[u8] = &[0xc0];

/// A decoded message. Params and results stay encoded.
#[derive(Debug, Clone, PartialEq)]
pub enum Message {
    /// A request expecting a response.
    Request {
        /// Request id, unique among the sender's in-flight requests.
        id: u32,
        /// Method name.
        method: String,
        /// The single argument, still encoded; `nil` if there was none.
        params: Bytes,
    },
    /// A response to a request.
    Response {
        /// Id of the request this answers.
        id: u32,
        /// The encoded result, or the error.
        result: Result<Bytes, RpcError>,
    },
    /// A notification.
    Notification {
        /// Method name.
        method: String,
        /// The single argument, still encoded; `nil` if there was none.
        params: Bytes,
    },
}

/// Why a frame could not be decoded or a value encoded.
#[derive(Debug)]
pub struct CodecError(pub String);

impl std::fmt::Display for CodecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for CodecError {}

fn err(msg: impl std::fmt::Display) -> CodecError {
    CodecError(msg.to_string())
}

/// Encodes a value with the protocol's rules (structs as maps).
fn encode_value<T: Serialize + ?Sized>(value: &T) -> Result<Vec<u8>, CodecError> {
    lsdocs_protocol::msgpack::encode(value).map_err(err)
}

/// Writes `params` as the msgpack-rpc argument array: `[]` for a unit/nil
/// value, otherwise `[params]`.
fn put_params<T: Serialize + ?Sized>(buf: &mut BytesMut, params: &T) -> Result<(), CodecError> {
    let value = encode_value(params)?;
    let mut w = buf.writer();
    if value == NIL {
        rmp::encode::write_array_len(&mut w, 0).map_err(err)?;
    } else {
        rmp::encode::write_array_len(&mut w, 1).map_err(err)?;
        w.get_mut().put_slice(&value);
    }
    Ok(())
}

/// Encodes a request.
pub fn encode_request<T: Serialize + ?Sized>(
    id: u32,
    method: &str,
    params: &T,
) -> Result<Bytes, CodecError> {
    let mut buf = BytesMut::new();
    let mut w = (&mut buf).writer();
    rmp::encode::write_array_len(&mut w, 4).map_err(err)?;
    rmp::encode::write_pfix(&mut w, REQUEST).map_err(err)?;
    rmp::encode::write_uint(&mut w, id.into()).map_err(err)?;
    rmp::encode::write_str(&mut w, method).map_err(err)?;
    put_params(&mut buf, params)?;
    Ok(buf.freeze())
}

/// Encodes a notification.
pub fn encode_notification<T: Serialize + ?Sized>(
    method: &str,
    params: &T,
) -> Result<Bytes, CodecError> {
    let mut buf = BytesMut::new();
    let mut w = (&mut buf).writer();
    rmp::encode::write_array_len(&mut w, 3).map_err(err)?;
    rmp::encode::write_pfix(&mut w, NOTIFICATION).map_err(err)?;
    rmp::encode::write_str(&mut w, method).map_err(err)?;
    put_params(&mut buf, params)?;
    Ok(buf.freeze())
}

/// Encodes a successful response whose result is already encoded.
pub fn encode_response_raw(id: u32, result: &[u8]) -> Bytes {
    let mut buf = BytesMut::with_capacity(8 + result.len());
    let mut w = (&mut buf).writer();
    // Writing to a BytesMut cannot fail.
    rmp::encode::write_array_len(&mut w, 4).expect("in-memory write");
    rmp::encode::write_pfix(&mut w, RESPONSE).expect("in-memory write");
    rmp::encode::write_uint(&mut w, id.into()).expect("in-memory write");
    rmp::encode::write_nil(&mut w).expect("in-memory write");
    buf.put_slice(result);
    buf.freeze()
}

/// Encodes a successful response.
pub fn encode_response<T: Serialize + ?Sized>(id: u32, result: &T) -> Result<Bytes, CodecError> {
    Ok(encode_response_raw(id, &encode_value(result)?))
}

/// Encodes an error response.
pub fn encode_error(id: u32, error: &RpcError) -> Bytes {
    let mut buf = BytesMut::new();
    let mut w = (&mut buf).writer();
    rmp::encode::write_array_len(&mut w, 4).expect("in-memory write");
    rmp::encode::write_pfix(&mut w, RESPONSE).expect("in-memory write");
    rmp::encode::write_uint(&mut w, id.into()).expect("in-memory write");
    let error = encode_value(error).expect("RpcError always encodes");
    buf.put_slice(&error);
    buf.put_slice(NIL);
    buf.freeze()
}

/// Decodes one frame.
pub fn decode(frame: Bytes) -> Result<Message, CodecError> {
    let mut rd: &[u8] = &frame;
    let len =
        rmp::decode::read_array_len(&mut rd).map_err(|e| err(format!("not an array: {e}")))?;
    let kind: u8 =
        rmp::decode::read_int(&mut rd).map_err(|e| err(format!("bad message type: {e}")))?;
    let message = match (kind, len) {
        (REQUEST, 4) => {
            let id = read_id(&mut rd)?;
            let method = read_method(&mut rd)?;
            let params = read_params(&frame, &mut rd)?;
            Message::Request { id, method, params }
        }
        (RESPONSE, 4) => {
            let id = read_id(&mut rd)?;
            let error = if rd.first() == Some(&0xc0) {
                rd = &rd[1..];
                None
            } else {
                let mut de = rmp_serde::Deserializer::new(&mut rd);
                Some(
                    RpcError::deserialize(&mut de)
                        .map_err(|e| err(format!("bad error object: {e}")))?,
                )
            };
            let result = match error {
                Some(error) => Err(error),
                None => Ok(slice_rest(&frame, rd)),
            };
            Message::Response { id, result }
        }
        (NOTIFICATION, 3) => {
            let method = read_method(&mut rd)?;
            let params = read_params(&frame, &mut rd)?;
            Message::Notification { method, params }
        }
        (REQUEST | RESPONSE | NOTIFICATION, len) => {
            return Err(err(format!("message type {kind} with {len} elements")));
        }
        (kind, _) => return Err(err(format!("unknown message type {kind}"))),
    };
    Ok(message)
}

/// The id of a request frame, if `frame` looks like a request. Used to answer
/// a request whose params could not be decoded.
pub fn peek_request_id(frame: &[u8]) -> Option<u32> {
    let mut rd = frame;
    let len = rmp::decode::read_array_len(&mut rd).ok()?;
    let kind: u8 = rmp::decode::read_int(&mut rd).ok()?;
    if kind != REQUEST || len != 4 {
        return None;
    }
    read_id(&mut rd).ok()
}

fn read_id(rd: &mut &[u8]) -> Result<u32, CodecError> {
    rmp::decode::read_int(rd).map_err(|e| err(format!("bad message id: {e}")))
}

fn read_method(rd: &mut &[u8]) -> Result<String, CodecError> {
    let len =
        rmp::decode::read_str_len(rd).map_err(|e| err(format!("bad method name: {e}")))? as usize;
    if rd.len() < len {
        return Err(err("truncated method name"));
    }
    let (name, rest) = rd.split_at(len);
    *rd = rest;
    String::from_utf8(name.to_vec()).map_err(|_| err("method name is not UTF-8"))
}

/// Reads the argument array header and returns the single argument.
fn read_params(frame: &Bytes, rd: &mut &[u8]) -> Result<Bytes, CodecError> {
    match rmp::decode::read_marker(rd).map_err(|_| err("missing params"))? {
        Marker::FixArray(0) => Ok(Bytes::from_static(NIL)),
        Marker::FixArray(1) => Ok(slice_rest(frame, rd)),
        Marker::FixArray(n) => Err(err(format!("expected at most 1 argument, got {n}"))),
        Marker::Array16 | Marker::Array32 => Err(err("expected at most 1 argument")),
        _ => Err(err("params must be an array")),
    }
}

/// The part of `frame` that `rest` (a suffix of it) covers, without copying.
fn slice_rest(frame: &Bytes, rest: &[u8]) -> Bytes {
    frame.slice(frame.len() - rest.len()..)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ErrorCode;
    use lsdocs_protocol::{ProgramInfo, msgpack};

    fn info() -> ProgramInfo {
        ProgramInfo {
            name: "x".into(),
            version: "1".into(),
        }
    }

    #[test]
    fn request_round_trip() {
        let frame = encode_request(7, "parse", &info()).unwrap();
        let Message::Request { id, method, params } = decode(frame).unwrap() else {
            panic!()
        };
        assert_eq!((id, method.as_str()), (7, "parse"));
        assert_eq!(msgpack::decode::<ProgramInfo>(&params).unwrap(), info());
    }

    #[test]
    fn request_is_a_plain_msgpack_rpc_array() {
        // [0, 300, "m", [{"name": "x", "version": "1"}]] as decoded by a
        // generic reader.
        let frame = encode_request(300, "m", &info()).unwrap();
        let value: serde_json::Value = rmp_serde::from_slice(&frame).unwrap();
        assert_eq!(
            value,
            serde_json::json!([0, 300, "m", [{"name": "x", "version": "1"}]])
        );
    }

    #[test]
    fn unit_params_are_an_empty_array() {
        let frame = encode_notification("initialized", &()).unwrap();
        let value: serde_json::Value = rmp_serde::from_slice(&frame).unwrap();
        assert_eq!(value, serde_json::json!([2, "initialized", []]));
        let Message::Notification { params, .. } = decode(frame).unwrap() else {
            panic!()
        };
        msgpack::decode::<()>(&params).unwrap();
    }

    /// The worked example in spec/wire.md.
    #[test]
    fn spec_example_bytes() {
        let frame = encode_notification("initialized", &()).unwrap();
        assert_eq!(
            frame.as_ref(),
            b"\x93\x02\xabinitialized\x90",
            "update spec/wire.md if this changes"
        );
    }

    #[test]
    fn success_response_round_trip() {
        let frame = encode_response(9, &info()).unwrap();
        let value: serde_json::Value = rmp_serde::from_slice(&frame).unwrap();
        assert_eq!(
            value,
            serde_json::json!([1, 9, null, {"name": "x", "version": "1"}])
        );
        let Message::Response { id, result } = decode(frame).unwrap() else {
            panic!()
        };
        assert_eq!(id, 9);
        assert_eq!(
            msgpack::decode::<ProgramInfo>(&result.unwrap()).unwrap(),
            info()
        );
    }

    #[test]
    fn error_response_round_trip() {
        let error = RpcError::new(ErrorCode::METHOD_NOT_FOUND, "no such method: x");
        let frame = encode_error(3, &error);
        let value: serde_json::Value = rmp_serde::from_slice(&frame).unwrap();
        assert_eq!(
            value,
            serde_json::json!([1, 3, {"code": -32601, "message": "no such method: x"}, null])
        );
        assert_eq!(
            decode(frame).unwrap(),
            Message::Response {
                id: 3,
                result: Err(error)
            }
        );
    }

    #[test]
    fn malformed_frames_are_rejected() {
        let bad = |v: serde_json::Value| decode(Bytes::from(rmp_serde::to_vec(&v).unwrap()));
        assert!(bad(serde_json::json!({"not": "an array"})).is_err());
        assert!(bad(serde_json::json!([5, 1, "m", []])).is_err());
        assert!(bad(serde_json::json!([0, 1, "m"])).is_err());
        assert!(bad(serde_json::json!([0, 1, "m", [1, 2]])).is_err());
        assert!(bad(serde_json::json!([0, -1, "m", []])).is_err());
        assert!(bad(serde_json::json!([2, "m", {"k": 1}])).is_err());
    }

    #[test]
    fn peek_finds_request_ids_only() {
        assert_eq!(
            peek_request_id(&encode_request(5, "m", &()).unwrap()),
            Some(5)
        );
        assert_eq!(
            peek_request_id(&encode_notification("m", &()).unwrap()),
            None
        );
    }
}
