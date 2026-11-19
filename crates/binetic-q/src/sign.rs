//! Signing primitives for the Q wire standard.
//!
//! A `NodeKey` is a 32-byte key that attributes a receipt chain to a specific
//! node identity. The attribution model:
//!
//! 1. The receipt chain (L0–L3) runs on **unsigned** packets — the existing
//!    `ReceiptTree::verify*` path is preserved exactly.
//! 2. After the chain is finalized, the node computes an **attribution tag**:
//!    `SHA-256("binetic-q-attest/v1" || key_bytes || chain_root)`
//!    This is a domain-separated keyed commitment over the whole receipt chain.
//! 3. A verifier who holds the node's key calls `verify_attribution(key,
//!    chain_root, tag)` to confirm the chain came from that node.
//!
//! The all-zero key is the "unsigned" sentinel — `run_signed` with the zero
//! key behaves identically to `run`, yielding `attribution_tag = None`.
//!
//! Keys are derived deterministically from a 64-bit seed so tests and tooling
//! never need external randomness. Production code should derive keys from a
//! hardware secret or a cryptographic KDF.

use binetic_core::{hash_bytes, HASH_LEN, ZERO_KEY};

/// A 32-byte node key used to attribute receipt chains.
///
/// `NodeKey::zero()` is the unsigned sentinel (backward-compatible).
/// `NodeKey::from_seed(seed)` derives a deterministic test/tool key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeKey(pub [u8; HASH_LEN]);

impl NodeKey {
    /// The unsigned sentinel. `run_signed` with this key yields
    /// `attribution_tag = None` (same as `run`).
    pub fn zero() -> Self {
        Self(ZERO_KEY)
    }

    /// True when this key is the all-zero unsigned sentinel.
    pub fn is_zero(&self) -> bool {
        self.0 == ZERO_KEY
    }

    /// Derive a deterministic key from a 64-bit seed. The derivation is
    /// SHA-256(seed_le || "binetic-q-node-key") so the domain is explicit and
    /// the result is distinct from any hash of raw data.
    pub fn from_seed(seed: u64) -> Self {
        let mut preimage = [0u8; 8 + 18];
        preimage[..8].copy_from_slice(&seed.to_le_bytes());
        preimage[8..].copy_from_slice(b"binetic-q-node-key");
        Self(hash_bytes(&preimage))
    }

    /// Raw bytes.
    pub fn as_bytes(&self) -> &[u8; HASH_LEN] {
        &self.0
    }

    /// Compute the attribution tag for a given chain root:
    /// `SHA-256("binetic-q-attest/v1" || key_bytes || chain_root)`
    pub fn attribution_tag(&self, chain_root: &[u8; HASH_LEN]) -> [u8; HASH_LEN] {
        const PREFIX: &[u8] = b"binetic-q-attest/v1";
        let mut preimage = Vec::with_capacity(PREFIX.len() + HASH_LEN * 2);
        preimage.extend_from_slice(PREFIX);
        preimage.extend_from_slice(&self.0);
        preimage.extend_from_slice(chain_root);
        hash_bytes(&preimage)
    }
}

/// Verify an attribution tag: returns `true` when
/// `tag == key.attribution_tag(chain_root)`.
///
/// This is the verifier-side check: the holder of the node's key confirms
/// that the chain root was produced by that node.
pub fn verify_attribution(
    key: &NodeKey,
    chain_root: &[u8; HASH_LEN],
    tag: &[u8; HASH_LEN],
) -> bool {
    key.attribution_tag(chain_root) == *tag
}

impl From<[u8; HASH_LEN]> for NodeKey {
    fn from(bytes: [u8; HASH_LEN]) -> Self {
        Self(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_key_is_zero() {
        assert!(NodeKey::zero().is_zero());
        assert_eq!(NodeKey::zero().0, [0u8; 32]);
    }

    #[test]
    fn from_seed_is_deterministic() {
        let a = NodeKey::from_seed(42);
        let b = NodeKey::from_seed(42);
        assert_eq!(a, b);
    }

    #[test]
    fn different_seeds_produce_different_keys() {
        let a = NodeKey::from_seed(1);
        let b = NodeKey::from_seed(2);
        assert_ne!(a, b);
    }

    #[test]
    fn from_seed_is_not_zero() {
        // A derived key must not accidentally be the unsigned sentinel.
        assert!(!NodeKey::from_seed(0).is_zero());
        assert!(!NodeKey::from_seed(42).is_zero());
    }

    #[test]
    fn attribution_tag_is_deterministic() {
        let key = NodeKey::from_seed(7);
        let root = [1u8; 32];
        assert_eq!(key.attribution_tag(&root), key.attribution_tag(&root));
    }

    #[test]
    fn verify_attribution_accepts_correct_key() {
        let key = NodeKey::from_seed(7);
        let root = [2u8; 32];
        let tag = key.attribution_tag(&root);
        assert!(verify_attribution(&key, &root, &tag));
    }

    #[test]
    fn verify_attribution_rejects_wrong_key() {
        let key_a = NodeKey::from_seed(1);
        let key_b = NodeKey::from_seed(2);
        let root = [3u8; 32];
        let tag = key_a.attribution_tag(&root);
        assert!(!verify_attribution(&key_b, &root, &tag));
    }

    #[test]
    fn verify_attribution_rejects_tampered_root() {
        let key = NodeKey::from_seed(5);
        let root = [4u8; 32];
        let tag = key.attribution_tag(&root);
        let tampered = [5u8; 32];
        assert!(!verify_attribution(&key, &tampered, &tag));
    }
}
