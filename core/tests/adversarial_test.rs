//! Adversarial / edge-case tests for protocol and sanitization primitives.
//!
//! These complement the happy-path integration tests by probing the exact
//! inputs a malicious or buggy peer could send.

use connected_core::codec::{decode_message, encode_message};
use connected_core::file_transfer::{is_safe_relative_path, sanitize_filename};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct Probe {
    text: String,
    items: Vec<u32>,
}

// ---------------------------------------------------------------------------
// sanitize_filename — traversal & platform-hostile names
// ---------------------------------------------------------------------------

#[test]
fn sanitize_strips_all_traversal_shapes() {
    // Every one of these must collapse to a bare name inside the download dir.
    let cases = [
        "../../etc/passwd",
        "..\\..\\windows\\system32\\evil.exe",
        "/absolute/path/file.txt",
        "C:\\Users\\victim\\secrets.txt",
        "sub/dir/name.zip",
        "....//....//file",
        "../",
        "..",
        ".",
        "./",
        "\\",
        "//",
    ];
    for case in cases {
        let out = sanitize_filename(case);
        assert!(!out.contains('/'), "{case:?} -> {out:?} contains '/'");
        assert!(!out.contains('\\'), "{case:?} -> {out:?} contains '\\'");
        assert!(!out.contains(".."), "{case:?} -> {out:?} contains '..'");
        assert!(!out.contains(':'), "{case:?} -> {out:?} contains ':'");
        assert!(!out.is_empty(), "{case:?} sanitized to empty");
    }
}

#[test]
fn sanitize_never_returns_dotfiles() {
    // Leading dots are stripped so received files can never silently shadow
    // Unix dot-files. All-dot names collapse to "unnamed".
    let out = sanitize_filename(".bashrc");
    assert_eq!(out, "bashrc");
    let out = sanitize_filename(".hidden.tar.gz");
    assert_eq!(out, "hidden.tar.gz");
    for case in ["...", "..", "."] {
        let out = sanitize_filename(case);
        assert!(!out.starts_with('.'), "{case:?} -> {out:?}");
        assert!(!out.is_empty(), "{case:?} -> {out:?}");
    }
}

#[test]
fn sanitize_handles_unicode_and_long_names() {
    // 300 CJK chars (~900 bytes) must be truncated within the 255-byte budget
    // WITHOUT panicking or splitting a UTF-8 char.
    let long_cjk: String = "文".repeat(300);
    let out = sanitize_filename(&long_cjk);
    assert!(out.len() <= 255, "byte length {}", out.len());
    // Must still be valid UTF-8 (it's a String, but be explicit about intent).

    // Emoji (4-byte) at the boundary must not split.
    let emoji: String = "\u{1F600}".repeat(80); // 320 bytes
    let out = sanitize_filename(&emoji);
    assert!(out.len() <= 255);
    assert!(std::str::from_utf8(out.as_bytes()).is_ok());
}

#[test]
fn sanitize_control_characters() {
    let out = sanitize_filename("bad\u{0000}\u{0007}name.txt");
    // NUL is filtered; the rest stays but cannot affect path structure.
    assert!(!out.contains('\0'));
    assert!(!out.contains('\n'));
}

// ---------------------------------------------------------------------------
// is_safe_relative_path — batch-transfer traversal gate
// ---------------------------------------------------------------------------

#[test]
fn safe_relative_rejects_traversal() {
    for bad in [
        "../escape",
        "a/../../escape",
        "..",
        "/abs",
        "//double",
        "a/..",
        "C:/win",
        "device:payload",
        "a/b/../../../c",
        "..\\windows",
    ] {
        assert!(!is_safe_relative_path(bad), "{bad:?} should be rejected");
    }
}

#[test]
fn safe_relative_accepts_legitimate() {
    for good in [
        "file.txt",
        "folder/sub/file.bin",
        "photos/2024/img-0001.jpg",
        "deep/".trim_end(),
        "a b/c d.txt",
        "ünïcode/ñame.pdf",
    ] {
        assert!(is_safe_relative_path(good), "{good:?} should be accepted");
    }
}

// ---------------------------------------------------------------------------
// codec — decode must never panic on attacker-controlled bytes
// ---------------------------------------------------------------------------

#[test]
fn decode_fuzz_random_bytes_never_panics() {
    // Deterministic xorshift PRNG so failures reproduce.
    let mut state: u64 = 0x9E3779B97F4A7C15;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };

    for _ in 0..20_000 {
        let len = (next() % 512) as usize;
        let mut data: Vec<u8> = Vec::with_capacity(len);
        for _ in 0..len {
            data.push((next() & 0xFF) as u8);
        }
        // Bias some buffers toward the current postcard marker.
        if !data.is_empty() && next() % 3 == 0 {
            data[0] = 0x01;
        }
        let _ = decode_message::<Probe>(&data); // must not panic
    }
}

#[test]
fn decode_fuzz_structurally_valid_but_hostile_lengths() {
    // Postcard uses varints for lengths. Claim a huge string with almost no
    // backing data — decoding must error, not hang or OOM.
    let mut crafted = vec![0x02u8]; // MAGIC_POSTCARD
    crafted.extend_from_slice(&[0xff; 10]);
    assert!(decode_message::<Probe>(&crafted).is_err());
}

#[test]
fn encode_decode_roundtrip_current_protocol() {
    let msg = Probe {
        text: "roundtrip ✓".into(),
        items: (0..1000).collect(),
    };
    let encoded = encode_message(&msg, connected_core::PROTOCOL_VERSION).unwrap();
    assert_eq!(decode_message::<Probe>(&encoded).unwrap().0, msg);

    // The compact postcard frame should stay close to the payload size even
    // for text-heavy messages.
    let texty = Probe {
        text: "payload-\"quoted\"-with-{braces}".repeat(20),
        items: vec![],
    };
    let encoded = encode_message(&texty, connected_core::PROTOCOL_VERSION).unwrap();
    assert!(encoded.len() < texty.text.len() + 100);
}
