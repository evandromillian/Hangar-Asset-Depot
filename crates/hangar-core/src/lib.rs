//! Shared domain types for Hangar.
//!
//! Everything in here is plain data + serde, so it compiles both for the
//! server (native) and for the Leptos/Bevy client (wasm32).

pub mod animation;
pub mod api;
pub mod bridge;
pub mod enums;
pub mod materials;
pub mod paths;
pub mod preview;

pub use animation::*;
pub use api::*;
pub use bridge::*;
pub use enums::*;
pub use materials::*;
pub use preview::*;

/// Hex-encoded BLAKE3 digest (64 lowercase hex chars) — the identity of a blob.
pub type BlobHash = String;

/// Returns true when `s` looks like a BLAKE3 hex digest.
pub fn is_valid_hash(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// URL (relative to the site root) where a blob can be downloaded.
pub fn blob_url(hash: &str) -> String {
    format!("/api/blobs/{hash}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_validation() {
        assert!(is_valid_hash(&"a".repeat(64)));
        assert!(!is_valid_hash(&"A".repeat(64)));
        assert!(!is_valid_hash("abc"));
        assert!(!is_valid_hash(&"g".repeat(64)));
    }
}
