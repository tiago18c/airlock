//! The shipped Bubblegum templates, against Bubblegum, MPL Account
//! Compression and MPL Noop (devnet runs mainnet's code of each; the
//! Bubblegum IDL ships for both): a tree sized from the shape and canopy
//! chosen, and a compressed NFT minted into it.

use crate::support::templates::{Reader, TemplateDesk};
use solana_pubkey::Pubkey;
use wallet_core::{IdlNetwork, KeypairSigner, WalletSigner};

const BUBBLEGUM: Pubkey = Pubkey::from_str_const("BGUMAp9Gq7iTEuizy4pqaxsTyUCBK68MDfK752saRPUY");
const COMPRESSION: Pubkey = Pubkey::from_str_const("mcmt6YrQEMKw8Mw43FmpRLmf7BqRnFMKmAcbxE3xkAW");
const NOOP: Pubkey = Pubkey::from_str_const("mnoopTCrg4p8ry25e4bcWA9XZjbNjMTfgYVGGEdRsf3");
const CORE: Pubkey = Pubkey::from_str_const("CoREENxT6tW1HoK8ypY1SxRMZTcVPm7R94rH4PZNhX7d");

fn desk() -> TemplateDesk {
    TemplateDesk::new(
        IdlNetwork::Devnet,
        &[
            (BUBBLEGUM, "mpl_bubblegum.so"),
            (COMPRESSION, "mpl_account_compression.so"),
            (NOOP, "mpl_noop.so"),
            (CORE, "mpl_core.so"),
        ],
        &[],
    )
}

fn config_of(tree: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[tree.as_ref()], &BUBBLEGUM).0
}

/// A tree config's creator, delegate, capacity, NFTs minted and whether
/// anyone may mint.
fn read_config(data: &[u8]) -> (Pubkey, Pubkey, u64, u64, bool) {
    let mut r = Reader::new(data, 8);
    (r.pubkey(), r.pubkey(), r.u64(), r.u64(), r.bool())
}

/// A tree account's buffer size, depth, authority, and the sequence number
/// of its last change.
fn read_tree(data: &[u8]) -> (u32, u32, Pubkey, u64) {
    let mut r = Reader::new(data, 2);
    let buffer = r.u32();
    let depth = r.u32();
    let authority = r.pubkey();
    r.at = 56;
    (buffer, depth, authority, r.u64())
}

fn make_tree(desk: &TemplateDesk, answers: &[(&str, String)]) -> (Pubkey, Vec<String>) {
    let tree = KeypairSigner::new_random();
    let landed = desk
        .land("create-nft-tree", &[], answers, &[("tree", &tree)], &[])
        .unwrap_or_else(|logs| panic!("{logs}"));
    (tree.pubkey(), desk.sentences(&landed.instructions))
}

#[test]
fn a_tree_of_the_shape_chosen_is_made_for_this_wallet() {
    let desk = desk();
    let (tree, sentences) = make_tree(&desk, &[]);
    let account = desk.node.chain().account(&tree).unwrap();
    assert_eq!(account.owner, COMPRESSION);
    assert_eq!(account.data.len(), 31_800);
    let (buffer, depth, authority, sequence) = read_tree(&account.data);
    assert_eq!((depth, buffer, sequence), (14, 64, 0));
    assert_eq!(authority, config_of(&tree));
    let (creator, delegate, capacity, minted, public) = read_config(&desk.data(&config_of(&tree)));
    assert_eq!((creator, delegate), (desk.me(), desk.me()));
    assert_eq!((capacity, minted, public), (16_384, 0, false));
    eprintln!("{sentences:#?}");
}

#[test]
fn a_deep_tree_takes_the_canopy_it_needs() {
    let desk = desk();
    let (tree, _) = make_tree(
        &desk,
        &[
            ("shape", "1,048,576 NFTs (depth 20, buffer 256)".to_string()),
            ("canopy", "5".to_string()),
            ("public", "true".to_string()),
        ],
    );
    let data = desk.data(&tree);
    // Header, tree and a canopy of 2^6 - 2 nodes.
    assert_eq!(data.len(), 56 + 24 + 257 * (40 + 32 * 20) + 62 * 32);
    let (buffer, depth, _, _) = read_tree(&data);
    assert_eq!((depth, buffer), (20, 256));
    let (_, _, capacity, _, public) = read_config(&desk.data(&config_of(&tree)));
    assert_eq!((capacity, public), (1 << 20, true));
}

#[test]
fn a_compressed_nft_is_the_next_leaf_and_names_its_owner() {
    let desk = desk();
    let (tree, _) = make_tree(&desk, &[]);
    let owner = KeypairSigner::new_random().pubkey();
    let landed = desk
        .land(
            "mint-compressed-nft",
            &[],
            &[
                ("tree", tree.to_string()),
                ("nft_name", "Leaf #1".to_string()),
                ("symbol", "LEAF".to_string()),
                ("uri", "https://example.com/leaf/1.json".to_string()),
                ("royalty", "2.5".to_string()),
                ("owner", owner.to_string()),
            ],
            &[],
            &[],
        )
        .unwrap_or_else(|logs| panic!("{logs}"));
    let (_, _, _, minted, _) = read_config(&desk.data(&config_of(&tree)));
    assert_eq!(minted, 1);
    let (_, _, _, sequence) = read_tree(&desk.data(&tree));
    assert_eq!(sequence, 1);
    // Bubblegum records the leaf through the noop program: its owner is
    // the one asked for.
    assert!(
        landed
            .inner
            .iter()
            .any(|data| data.windows(32).any(|window| window == owner.as_ref())),
        "no call names the owner"
    );
    let sentences = desk.sentences(&landed.instructions);
    eprintln!("{sentences:#?}");
    assert!(sentences[0].contains("Leaf #1"), "{}", sentences[0]);

    // The next mint is the next leaf.
    desk.land(
        "mint-compressed-nft",
        &[],
        &[
            ("tree", tree.to_string()),
            ("nft_name", "Leaf #2".to_string()),
            ("symbol", "LEAF".to_string()),
            ("uri", "https://example.com/leaf/2.json".to_string()),
        ],
        &[],
        &[],
    )
    .unwrap_or_else(|logs| panic!("{logs}"));
    let (_, _, _, minted, _) = read_config(&desk.data(&config_of(&tree)));
    assert_eq!(minted, 2);
}

#[test]
fn only_the_trees_creator_mints_into_a_tree_that_is_not_public() {
    let desk = desk();
    let (tree, _) = make_tree(&desk, &[]);
    let other = TemplateDesk::new(
        IdlNetwork::Devnet,
        &[
            (BUBBLEGUM, "mpl_bubblegum.so"),
            (COMPRESSION, "mpl_account_compression.so"),
            (NOOP, "mpl_noop.so"),
            (CORE, "mpl_core.so"),
        ],
        &[],
    );
    // The same tree, on another wallet's desk: copy its two accounts over.
    for address in [tree, config_of(&tree)] {
        let account = desk.node.chain().account(&address).unwrap();
        other
            .node
            .chain()
            .plant_raw(address, account.owner, account.lamports, account.data);
    }
    let refused = other.land(
        "mint-compressed-nft",
        &[],
        &[
            ("tree", tree.to_string()),
            ("nft_name", "Stray".to_string()),
            ("symbol", "X".to_string()),
            ("uri", "https://example.com/x.json".to_string()),
        ],
        &[],
        &[],
    );
    assert!(refused.is_err());
}
