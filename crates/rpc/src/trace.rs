//! Logs every frame as JSON when the `lsdocs_rpc::wire` tracing target is
//! enabled at TRACE level (what `--trace-rpc` turns on).

use std::fmt;

use serde::de::{Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Value};

/// Tracing target for wire logs.
pub const TARGET: &str = "lsdocs_rpc::wire";

/// Logs `frame` if wire tracing is enabled.
pub(crate) fn frame(direction: &'static str, frame: &[u8]) {
    if tracing::enabled!(target: TARGET, tracing::Level::TRACE) {
        tracing::trace!(target: TARGET, direction, "{}", to_json(frame));
    }
}

/// Renders a MessagePack value as JSON text. Binary data, which JSON cannot
/// hold, is shown as `"<N bytes>"`.
pub fn to_json(frame: &[u8]) -> String {
    match rmp_serde::from_slice::<Traced>(frame) {
        Ok(Traced(value)) => value.to_string(),
        Err(e) => format!("<undecodable frame: {e}>"),
    }
}

struct Traced(Value);

impl<'de> Deserialize<'de> for Traced {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_any(TracedVisitor).map(Traced)
    }
}

struct TracedVisitor;

impl<'de> Visitor<'de> for TracedVisitor {
    type Value = Value;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("any MessagePack value")
    }

    fn visit_bool<E>(self, v: bool) -> Result<Value, E> {
        Ok(v.into())
    }
    fn visit_i64<E>(self, v: i64) -> Result<Value, E> {
        Ok(v.into())
    }
    fn visit_u64<E>(self, v: u64) -> Result<Value, E> {
        Ok(v.into())
    }
    fn visit_f64<E>(self, v: f64) -> Result<Value, E> {
        Ok(v.into())
    }
    fn visit_str<E>(self, v: &str) -> Result<Value, E> {
        Ok(v.into())
    }
    fn visit_bytes<E>(self, v: &[u8]) -> Result<Value, E> {
        Ok(format!("<{} bytes>", v.len()).into())
    }
    fn visit_unit<E>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }
    fn visit_none<E>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }
    fn visit_some<D: Deserializer<'de>>(self, d: D) -> Result<Value, D::Error> {
        Traced::deserialize(d).map(|t| t.0)
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Value, A::Error> {
        let mut items = Vec::new();
        while let Some(Traced(item)) = seq.next_element()? {
            items.push(item);
        }
        Ok(Value::Array(items))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Value, A::Error> {
        let mut out = Map::new();
        while let Some((Traced(key), Traced(value))) = map.next_entry()? {
            let key = match key {
                Value::String(s) => s,
                other => other.to_string(),
            };
            out.insert(key, value);
        }
        Ok(Value::Object(out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_render_as_json_with_binary_summarized() {
        let frame = crate::message::encode_response(
            4,
            &lsdocs_protocol::RenderResult {
                output: vec![0; 10],
                extension: "html".into(),
                diagnostics: vec![],
            },
        )
        .unwrap();
        assert_eq!(
            to_json(&frame),
            r#"[1,4,null,{"extension":"html","output":"<10 bytes>"}]"#
        );
    }
}
