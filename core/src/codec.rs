use crate::error::{ConnectedError, Result};
use serde::{Serialize, de::DeserializeOwned};

/// Magic byte for the current postcard wire format.
///
/// The protocol is intentionally strict: frames from the removed JSON and
/// bincode formats are rejected instead of being guessed at or decoded with a
/// compatibility fallback.
const MAGIC_POSTCARD: u8 = 0x02;

/// Upper bound on the number of bytes a single decoded message may consume.
/// The transport layer applies a similar frame limit; keeping the check here
/// makes the codec safe when called directly from tests or other entry points.
const MAX_DECODE_BYTES: usize = 128 * 1024 * 1024;

fn serialization_error(context: &str, error: impl std::fmt::Display) -> ConnectedError {
    ConnectedError::Serialization(serde_json::Error::io(std::io::Error::other(format!(
        "{context}: {error}"
    ))))
}

/// Serialize a message using the current postcard protocol.
pub fn encode_message<T: Serialize>(msg: &T) -> Result<Vec<u8>> {
    let payload = postcard::to_stdvec(msg)
        .map_err(|error| serialization_error("postcard encode failed", error))?;
    if payload.len() > MAX_DECODE_BYTES {
        return Err(ConnectedError::Protocol(format!(
            "Encoded message exceeds {} byte limit",
            MAX_DECODE_BYTES
        )));
    }

    let mut output = Vec::with_capacity(payload.len() + 1);
    output.push(MAGIC_POSTCARD);
    output.extend_from_slice(&payload);
    Ok(output)
}

/// Deserialize one current-protocol postcard message.
pub fn decode_message<T: DeserializeOwned>(data: &[u8]) -> Result<T> {
    if data.len() < 2 {
        return Err(ConnectedError::Protocol(
            "Postcard frame is empty or missing payload".to_string(),
        ));
    }
    if data.len() > MAX_DECODE_BYTES {
        return Err(ConnectedError::Protocol(format!(
            "Message exceeds {} byte limit",
            MAX_DECODE_BYTES
        )));
    }
    if data[0] != MAGIC_POSTCARD {
        return Err(ConnectedError::Protocol(format!(
            "Unsupported message encoding marker: 0x{:02x}",
            data[0]
        )));
    }

    let (message, remaining) = postcard::take_from_bytes::<T>(&data[1..])
        .map_err(|error| serialization_error("postcard decode failed", error))?;
    if !remaining.is_empty() {
        return Err(ConnectedError::Protocol(
            "Postcard frame contains trailing bytes".to_string(),
        ));
    }
    Ok(message)
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
    fn postcard_roundtrip() {
        let msg = Probe {
            text: "hello".into(),
            items: vec![1, 2, 3],
        };
        let encoded = encode_message(&msg).unwrap();
        assert_eq!(encoded[0], MAGIC_POSTCARD);
        let decoded: Probe = decode_message(&encoded).unwrap();
        assert_eq!(decoded, msg);
    }

    #[test]
    fn rejects_legacy_encodings() {
        assert!(decode_message::<Probe>(b"{\"text\":\"legacy\"}").is_err());
        assert!(decode_message::<Probe>(&[0x01, 0, 1, 2, 3]).is_err());
    }

    #[test]
    fn rejects_trailing_bytes() {
        let msg = Probe {
            text: "strict".into(),
            items: vec![],
        };
        let mut encoded = encode_message(&msg).unwrap();
        encoded.push(0xff);
        assert!(decode_message::<Probe>(&encoded).is_err());
    }

    #[test]
    fn rejects_hostile_lengths() {
        // Postcard uses varints for collection lengths. This claims an
        // impossibly large string while providing no backing bytes.
        let mut crafted = vec![MAGIC_POSTCARD];
        crafted.extend_from_slice(&[0xff; 10]);
        assert!(decode_message::<Probe>(&crafted).is_err());
    }

    #[test]
    fn rejects_empty_input() {
        assert!(decode_message::<Probe>(&[]).is_err());
        assert!(decode_message::<Probe>(&[MAGIC_POSTCARD]).is_err());
    }
}
