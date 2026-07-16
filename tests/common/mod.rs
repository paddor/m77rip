#![allow(dead_code)]

use m77rip::{compress, decompress};

fn text_payload(target_bytes: usize) -> Vec<u8> {
    const SENTENCE: &[u8] =
        b"the quick brown fox jumps over the lazy dog; misa77 block test payload\n";
    let mut out = Vec::with_capacity(target_bytes);
    while out.len() < target_bytes {
        out.extend_from_slice(SENTENCE);
    }
    out.truncate(target_bytes);
    out
}

fn json_payload(target_bytes: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(target_bytes);
    let mut i = 0u64;
    while out.len() < target_bytes {
        let line = format!(
            r#"{{"ts":1700000000,"level":"INFO","service":"ingest","event":{i},"message":"repeatable structured payload for misa77 tests"}}"#,
        );
        out.extend_from_slice(line.as_bytes());
        out.push(b'\n');
        i += 1;
    }
    out.truncate(target_bytes);
    out
}

pub static COMPRESSION1K: std::sync::LazyLock<Vec<u8>> =
    std::sync::LazyLock::new(|| text_payload(725));
pub static COMPRESSION34K: std::sync::LazyLock<Vec<u8>> =
    std::sync::LazyLock::new(|| text_payload(34_308));
pub static COMPRESSION65: std::sync::LazyLock<Vec<u8>> =
    std::sync::LazyLock::new(|| text_payload(64_723));
pub static COMPRESSION66JSON: std::sync::LazyLock<Vec<u8>> =
    std::sync::LazyLock::new(|| json_payload(66_675));

pub fn compression1k() -> &'static [u8] {
    &COMPRESSION1K
}

pub fn compression34k() -> &'static [u8] {
    &COMPRESSION34K
}

pub fn compression65() -> &'static [u8] {
    &COMPRESSION65
}

pub fn compression66json() -> &'static [u8] {
    &COMPRESSION66JSON
}

pub fn test_roundtrip(bytes: impl AsRef<[u8]>) {
    let bytes = bytes.as_ref();
    let compressed = compress(bytes);
    let decompressed = decompress(&compressed, bytes.len()).unwrap();
    assert_eq!(decompressed, bytes);
}
