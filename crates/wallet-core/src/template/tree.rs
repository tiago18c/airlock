//! The merkle trees compressed NFTs live in: the shapes a tree can take
//! and the size of its account.
//!
//! A concurrent merkle tree has a depth (it holds 2^depth leaves) and a
//! buffer (how many changes to it can land in one slot). The account
//! compression programs take only the pairs below, the `ALL_DEPTH_SIZE_PAIRS`
//! of MPL Account Compression's SDK (`sdk/src/constants/index.ts`), which
//! its program's `merkle_tree_get_size` matches. Any other pair fails on
//! chain, so a template offers these as a choice.
//!
//! The account holds a 56-byte header, the tree, and the canopy: the top
//! levels of the tree kept on chain so a proof can leave them out. Its size
//! is `getConcurrentMerkleTreeAccountSize` of that SDK. Bubblegum refuses a
//! tree whose canopy leaves proofs longer than 17 nodes
//! (`MAX_ACC_PROOFS_SIZE`, `check_canopy_size` in its `create_tree`).

use crate::idl::build::IdlArgKind;

/// Every depth and buffer pair a tree can be made with.
pub const TREE_SHAPES: [(u32, u32); 34] = [
    (3, 8),
    (5, 8),
    (6, 16),
    (7, 16),
    (8, 16),
    (9, 16),
    (10, 32),
    (11, 32),
    (12, 32),
    (13, 32),
    (14, 64),
    (14, 256),
    (14, 1024),
    (14, 2048),
    (15, 64),
    (16, 64),
    (17, 64),
    (18, 64),
    (19, 64),
    (20, 64),
    (20, 256),
    (20, 1024),
    (20, 2048),
    (24, 64),
    (24, 256),
    (24, 512),
    (24, 1024),
    (24, 2048),
    (26, 512),
    (26, 1024),
    (26, 2048),
    (30, 512),
    (30, 1024),
    (30, 2048),
];

/// The longest proof Bubblegum takes: a tree of more levels than this
/// keeps the rest in its canopy.
pub const MAX_PROOF_NODES: u32 = 17;

/// A header: the account type, the header's version, and its data
/// (buffer size, depth, authority, creation slot, a flag and padding).
const HEADER_LEN: usize = 2 + 54;
const NODE_LEN: usize = 32;

/// How a shape reads in a form: "16,384 NFTs (depth 14, buffer 64)".
pub fn shape_name(depth: u32, buffer: u32) -> String {
    let capacity = 1u64 << depth;
    let digits = capacity.to_string();
    let mut grouped = String::new();
    for (at, digit) in digits.chars().enumerate() {
        if at > 0 && (digits.len() - at).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    format!("{grouped} NFTs (depth {depth}, buffer {buffer})")
}

/// The depth and buffer of the shape named `text`.
pub fn tree_shape(text: &str) -> Option<(u32, u32)> {
    TREE_SHAPES
        .into_iter()
        .find(|(depth, buffer)| shape_name(*depth, *buffer) == text.trim())
}

/// The choice an input given a shape takes.
pub fn tree_shape_kind() -> IdlArgKind {
    IdlArgKind::Enum {
        variants: TREE_SHAPES
            .iter()
            .map(|(depth, buffer)| shape_name(*depth, *buffer))
            .collect(),
    }
}

/// The bytes of a tree account of this shape with a canopy of `canopy`
/// levels. `None` for a canopy Bubblegum refuses: one that leaves proofs
/// longer than [`MAX_PROOF_NODES`], or one as deep as the tree.
pub fn merkle_tree_len(depth: u32, buffer: u32, canopy: u32) -> Option<usize> {
    if canopy >= depth || canopy < depth.saturating_sub(MAX_PROOF_NODES) {
        return None;
    }
    let depth = depth as usize;
    // A change log: a root, a path, an index and padding.
    let change_log = NODE_LEN + depth * NODE_LEN + 4 + 4;
    // The rightmost proof: a path, a leaf, an index and padding.
    let rightmost = depth * NODE_LEN + NODE_LEN + 4 + 4;
    // Sequence number, active index and buffer size, then the buffer.
    let tree = 8 + 8 + 8 + buffer as usize * change_log + rightmost;
    let canopy = ((1usize << (canopy + 1)) - 2) * NODE_LEN;
    Some(HEADER_LEN + tree + canopy)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sizes `getConcurrentMerkleTreeAccountSize` gives.
    #[test]
    fn a_tree_is_its_header_its_buffer_and_its_canopy() {
        assert_eq!(merkle_tree_len(14, 64, 0), Some(31_800));
        assert_eq!(merkle_tree_len(3, 8, 0), Some(1_304));
        assert_eq!(merkle_tree_len(14, 64, 4), Some(31_800 + 30 * 32));
        assert_eq!(merkle_tree_len(20, 1024, 10), Some(697_080 + 2_046 * 32));
    }

    #[test]
    fn a_canopy_bubblegum_refuses_has_no_size() {
        assert_eq!(merkle_tree_len(24, 64, 6), None);
        assert!(merkle_tree_len(24, 64, 7).is_some());
        assert_eq!(merkle_tree_len(14, 64, 14), None);
        assert!(merkle_tree_len(14, 64, 13).is_some());
    }

    #[test]
    fn a_shape_is_named_with_what_it_holds() {
        assert_eq!(shape_name(14, 64), "16,384 NFTs (depth 14, buffer 64)");
        assert_eq!(shape_name(3, 8), "8 NFTs (depth 3, buffer 8)");
        assert_eq!(
            shape_name(30, 2048),
            "1,073,741,824 NFTs (depth 30, buffer 2048)"
        );
        assert_eq!(
            tree_shape("16,384 NFTs (depth 14, buffer 64)"),
            Some((14, 64))
        );
        assert_eq!(tree_shape("depth 14, buffer 64"), None);
        assert_eq!(tree_shape("16,384 NFTs (depth 14, buffer 128)"), None);
    }
}
