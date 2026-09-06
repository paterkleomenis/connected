use crate::error::{ConnectedError, Result};
use bincode::Options as _;
use serde::{Serialize, de::DeserializeOwned};

/// Magic bytes for versioned framing.
/// Old peers (v1) send raw JSON with no prefix (first byte is `{` 0x7B).
/// New peers (v2+) send 0x01 + bincode or 0x00 + JSON.
const MAGIC_BINCODE: u8 = 0x01;
const MAGIC_JSON: u8 = 0x00;

/// Upper bound on the number of bytes a single decode may consume.
///
/// Security: bincode trusts embedded length prefixes, so deserializing
/// attacker-controlled bytes without a limit lets tiny frames drive runaway
/// reads. The transport layer caps frames at ~105 MB; this mirrors that bound.
const MAX_DECODE_BYTES: u64 = 128 * 1024 * 1024;

fn bincode_options() -> impl bincode::Options {
    // Match the legacy `bincode::serialize` wire format exactly
    // (fixint encoding, little-endian) and only add the byte budget.
    // NOTE: DefaultOptions alone would switch to varint encoding and break
    // compatibility with peers using the plain serialize/deserialize API.
    bincode::options()
        .with_fixint_encoding()
        .with_limit(MAX_DECODE_BYTES)
}

/// Serialize a message for a given peer version.
/// - v1: JSON without prefix (backward compat)
/// - v2+: bincode with 0x01 prefix (faster, ~30% smaller, no base64 overhead for control messages)
pub fn encode_message<T: Serialize>(msg: &T, peer_version: u32) -> Result<Vec<u8>> {
    if peer_version >= 2 {
        let mut out = Vec::with_capacity(64);
        out.push(MAGIC_BINCODE);
        let mut bin = bincode::serialize(msg).map_err(|e| {
            ConnectedError::Serialization(serde_json::Error::io(std::io::Error::other(
                e.to_string(),
            )))
        })?;
        out.append(&mut bin);
        Ok(out)
    } else {
        // JSON without prefix for old peers
        serde_json::to_vec(msg).map_err(ConnectedError::Serialization)
    }
}

/// Deserialize a message that may be JSON (old) or bincode with magic (new).
/// Tries magic byte first, then falls back to JSON for backward compat.
pub fn decode_message<T: DeserializeOwned>(data: &[u8]) -> Result<T> {
    if data.is_empty() {
        return Err(ConnectedError::Protocol("Empty message".to_string()));
    }
    match data[0] {
        MAGIC_BINCODE => bincode_options().deserialize::<T>(&data[1..]).map_err(|e| {
            ConnectedError::Serialization(serde_json::Error::io(std::io::Error::other(format!(
                "bincode deserialize failed: {}",
                e
            ))))
        }),
        MAGIC_JSON => {
            serde_json::from_slice::<T>(&data[1..]).map_err(ConnectedError::Serialization)
        }
        _ => {
            // No magic — old JSON (starts with `{` 0x7B)
            // Try JSON first, then bincode as extra fallback (in case sender forgot magic)
            serde_json::from_slice::<T>(data).or_else(|_| {
                bincode_options().deserialize::<T>(data).map_err(|e| {
                    ConnectedError::Serialization(serde_json::Error::io(std::io::Error::other(
                        format!("bincode deserialize failed: {}", e),
                    )))
                })
            })
        }
    }
}

/// Helper to get peer version, defaulting to 1 for unknown peers.
pub fn peer_version_or_default(version: Option<u32>) -> u32 {
    version.unwrap_or(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[derive(Serialize, Deserialize, Debug, PartialEq)]
    struct Probe {
        text: String,
        items: Vec<u32>,
    }

    #[test]
    fn bincode_roundtrip() {
        let msg = Probe {
            text: "hello".into(),
            items: vec![1, 2, 3],
        };
        let encoded = encode_message(&msg, 2).unwrap();
        assert_eq!(encoded[0], MAGIC_BINCODE);
        let decoded: Probe = decode_message(&encoded).unwrap();
        assert_eq!(decoded, msg);
    }

    #[test]
    fn json_roundtrip_v1() {
        let msg = Probe {
            text: "legacy".into(),
            items: vec![],
        };
        let encoded = encode_message(&msg, 1).unwrap();
        let decoded: Probe = decode_message(&encoded).unwrap();
        assert_eq!(decoded, msg);
    }

    #[test]
    fn rejects_oversized_bincode_declared_lengths() {
        // Crafted payload claiming a huge embedded Vec must fail fast on the
        // byte budget instead of driving a runaway read.
        let mut crafted = vec![MAGIC_BINCODE];
        crafted.extend_from_slice(&u64::MAX.to_le_bytes()); // "length" of the string field
        let result: Result<Probe> = decode_message(&crafted);
        assert!(result.is_err());
    }

    #[test]
    fn empty_input_is_protocol_error() {
        let result: Result<Probe> = decode_message(&[]);
        assert!(result.is_err());
    }
}
