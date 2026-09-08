use crate::error::{ConnectedError, Result};
use serde::{Serialize, de::DeserializeOwned};

/// Marker for the current compact wire format. v1 frames are raw JSON.
const MAGIC_POSTCARD: u8 = 0x02;

/// Upper bound on the number of bytes a single decoded message may consume.
/// The transport layer applies a similar frame limit; keeping the check here
/// makes the codec safe when called directly from tests or other entry points.
const MAX_DECODE_BYTES: usize = 128 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WireFormat {
    V1Json,
    Postcard,
}

impl WireFormat {
    pub const fn for_peer_version(version: u32) -> Self {
        if version <= 2 {
            Self::V1Json
        } else {
            Self::Postcard
        }
    }
}

fn serialization_error(context: &str, error: impl std::fmt::Display) -> ConnectedError {
    ConnectedError::Serialization(serde_json::Error::io(std::io::Error::other(format!(
        "{context}: {error}"
    ))))
}

/// Serialize for a known peer. Versions 0 and 1 are the released JSON protocol.
pub fn encode_message<T: Serialize>(message: &T, peer_version: u32) -> Result<Vec<u8>> {
    match WireFormat::for_peer_version(peer_version) {
        WireFormat::V1Json => serde_json::to_vec(message).map_err(ConnectedError::Serialization),
        WireFormat::Postcard => {
            let payload = postcard::to_stdvec(message)
                .map_err(|error| serialization_error("postcard encode failed", error))?;
            if payload.len() > MAX_DECODE_BYTES {
                return Err(ConnectedError::Protocol(format!(
                    "Encoded message exceeds {MAX_DECODE_BYTES} byte limit"
                )));
            }
            let mut output = Vec::with_capacity(payload.len() + 1);
            output.push(MAGIC_POSTCARD);
            output.extend_from_slice(&payload);
            Ok(output)
        }
    }
}

/// Serialize for the current generation when no peer negotiation is involved.
/// The v1 handshake predates `protocol_version`; retain its exact JSON shape.
pub fn encode_v1_handshake(
    device_id: &str,
    device_name: &str,
    listening_port: u16,
) -> Result<Vec<u8>> {
    #[derive(Serialize)]
    struct Handshake<'a> {
        device_id: &'a str,
        device_name: &'a str,
        listening_port: u16,
    }
    serde_json::to_vec(
        &serde_json::json!({ "Handshake": Handshake { device_id, device_name, listening_port } }),
    )
    .map_err(ConnectedError::Serialization)
}

pub fn encode_v1_handshake_ack(device_id: &str, device_name: &str) -> Result<Vec<u8>> {
    #[derive(Serialize)]
    struct HandshakeAck<'a> {
        device_id: &'a str,
        device_name: &'a str,
    }
    serde_json::to_vec(
        &serde_json::json!({ "HandshakeAck": HandshakeAck { device_id, device_name } }),
    )
    .map_err(ConnectedError::Serialization)
}

/// Decode a frame and retain its format so a response can mirror the request.
pub fn decode_message<T: DeserializeOwned>(data: &[u8]) -> Result<(T, WireFormat)> {
    if data.is_empty() {
        return Err(ConnectedError::Protocol("Empty message".to_string()));
    }
    if data.len() > MAX_DECODE_BYTES {
        return Err(ConnectedError::Protocol(format!(
            "Message exceeds {MAX_DECODE_BYTES} byte limit"
        )));
    }
    if data[0] == MAGIC_POSTCARD {
        if data.len() == 1 {
            return Err(ConnectedError::Protocol(
                "Postcard frame is missing payload".to_string(),
            ));
        }
        let (message, remaining) = postcard::take_from_bytes(&data[1..])
            .map_err(|error| serialization_error("postcard decode failed", error))?;
        if !remaining.is_empty() {
            return Err(ConnectedError::Protocol(
                "Postcard frame contains trailing bytes".to_string(),
            ));
        }
        Ok((message, WireFormat::Postcard))
    } else {
        // v1 uses JSON's externally-tagged representation. Unit variants are
        // JSON strings (for example `"Accept"`), while structured variants
        // are objects. The old bincode marker is deliberately rejected.
        if data[0] == 0x01 {
            return Err(ConnectedError::Protocol(format!(
                "Unsupported message encoding marker: 0x{:02x}",
                data[0]
            )));
        }
        serde_json::from_slice(data)
            .map(|message| (message, WireFormat::V1Json))
            .map_err(ConnectedError::Serialization)
    }
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
    fn v1_json_and_postcard_roundtrip() {
        let msg = Probe {
            text: "hello".into(),
            items: vec![1, 2, 3],
        };
        let v1 = encode_message(&msg, 1).unwrap();
        assert_eq!(v1[0], b'{');
        assert_eq!(decode_message::<Probe>(&v1).unwrap().1, WireFormat::V1Json);
        let v2 = encode_message(&msg, 2).unwrap();
        assert_eq!(v2[0], b'{');
        assert_eq!(decode_message::<Probe>(&v2).unwrap().1, WireFormat::V1Json);
        let current = encode_message(&msg, crate::PROTOCOL_VERSION).unwrap();
        assert_eq!(current[0], MAGIC_POSTCARD);
        assert_eq!(decode_message::<Probe>(&current).unwrap().0, msg);
    }

    #[test]
    fn rejects_bincode_encoding() {
        assert!(decode_message::<Probe>(&[0x01, 0, 1, 2]).is_err());
    }

    #[test]
    fn rejects_trailing_bytes() {
        let mut valid = encode_message(
            &Probe {
                text: "x".into(),
                items: vec![],
            },
            crate::PROTOCOL_VERSION,
        )
        .unwrap();
        valid.push(0xff);
        assert!(decode_message::<Probe>(&valid).is_err());
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

    #[test]
    fn v1_handshake_omits_the_new_version_field() {
        let encoded = encode_v1_handshake("device", "Device", 44444).unwrap();
        let value: serde_json::Value = serde_json::from_slice(&encoded).unwrap();
        assert!(value["Handshake"].get("protocol_version").is_none());
        let (decoded, _) = decode_message::<crate::transport::Message>(&encoded).unwrap();
        match decoded {
            crate::transport::Message::Handshake {
                protocol_version, ..
            } => assert_eq!(protocol_version, 1),
            _ => panic!("expected handshake"),
        }
    }

    #[test]
    fn filesystem_message_roundtrips_in_both_wire_formats() {
        let message = crate::filesystem::FilesystemMessage::ListDirRequest {
            path: "Documents".to_string(),
        };

        let legacy = encode_message(&message, 1).unwrap();
        let (decoded, format) =
            decode_message::<crate::filesystem::FilesystemMessage>(&legacy).unwrap();
        assert_eq!(decoded, message);
        assert_eq!(format, WireFormat::V1Json);

        let current = encode_message(&message, crate::PROTOCOL_VERSION).unwrap();
        let (decoded, format) =
            decode_message::<crate::filesystem::FilesystemMessage>(&current).unwrap();
        assert_eq!(decoded, message);
        assert_eq!(format, WireFormat::Postcard);
    }

    #[test]
    fn file_transfer_response_uses_request_format() {
        let request = crate::file_transfer::FileTransferMessage::SendRequest {
            filename: "example.txt".to_string(),
            size: 12,
            mime_type: Some("text/plain".to_string()),
        };
        let response = crate::file_transfer::FileTransferMessage::Accept;

        for version in [1, crate::PROTOCOL_VERSION] {
            let request_bytes = encode_message(&request, version).unwrap();
            let (_, format) =
                decode_message::<crate::file_transfer::FileTransferMessage>(&request_bytes)
                    .unwrap();
            let response_version = match format {
                WireFormat::V1Json => 1,
                WireFormat::Postcard => crate::PROTOCOL_VERSION,
            };
            let response_bytes = encode_message(&response, response_version).unwrap();
            let (decoded, response_format) =
                decode_message::<crate::file_transfer::FileTransferMessage>(&response_bytes)
                    .unwrap();
            assert_eq!(decoded, response);
            assert_eq!(response_format, format);
        }
    }
}
