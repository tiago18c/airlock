//! The shipped Metaplex Core templates, against the Core program (devnet
//! runs mainnet's code; its IDL ships for both): a collection, and an NFT
//! alone or added to a collection.

use crate::support::templates::{Reader, TemplateDesk};
use solana_pubkey::Pubkey;
use wallet_core::{IdlNetwork, KeypairSigner, WalletSigner};

const CORE: Pubkey = Pubkey::from_str_const("CoREENxT6tW1HoK8ypY1SxRMZTcVPm7R94rH4PZNhX7d");

// Core's account keys and update authority kinds.
const ASSET_V1: u8 = 1;
const COLLECTION_V1: u8 = 5;
const BY_ADDRESS: u8 = 1;
const BY_COLLECTION: u8 = 2;

fn desk() -> TemplateDesk {
    TemplateDesk::new(IdlNetwork::Devnet, &[(CORE, "mpl_core.so")], &[])
}

/// An asset's owner, its update authority (kind and address), name and link.
fn read_asset(data: &[u8]) -> (Pubkey, u8, Pubkey, String, String) {
    let mut r = Reader::new(data, 0);
    assert_eq!(r.u8(), ASSET_V1);
    let owner = r.pubkey();
    let kind = r.u8();
    let authority = r.pubkey();
    (owner, kind, authority, r.string(), r.string())
}

/// A collection's update authority, name, link, and NFTs minted and held.
fn read_collection(data: &[u8]) -> (Pubkey, String, String, u32, u32) {
    let mut r = Reader::new(data, 0);
    assert_eq!(r.u8(), COLLECTION_V1);
    (r.pubkey(), r.string(), r.string(), r.u32(), r.u32())
}

fn text(value: &str) -> String {
    value.to_string()
}

/// Lands Create a Core collection with the wallet as its update authority.
fn collection(desk: &TemplateDesk, answers: &[(&str, String)]) -> (Pubkey, Vec<String>) {
    let key = KeypairSigner::new_random();
    let landed = desk
        .land(
            "create-core-collection",
            &[],
            answers,
            &[("collection", &key)],
            &[],
        )
        .unwrap_or_else(|logs| panic!("{logs}"));
    (key.pubkey(), desk.sentences(&landed.instructions))
}

#[test]
fn a_collection_holds_its_name_link_and_update_authority() {
    let desk = desk();
    let keeper = KeypairSigner::new_random().pubkey();
    let (key, sentences) = collection(
        &desk,
        &[
            ("collection_name", text("Apes")),
            ("uri", text("https://example.com/apes.json")),
            ("update_authority", keeper.to_string()),
        ],
    );
    let (authority, name, uri, minted, size) = read_collection(&desk.data(&key));
    assert_eq!(authority, keeper);
    assert_eq!(
        (&name[..], &uri[..]),
        ("Apes", "https://example.com/apes.json")
    );
    assert_eq!((minted, size), (0, 0));
    eprintln!("{sentences:#?}");
    assert!(sentences[0].contains("Apes"), "{}", sentences[0]);
}

#[test]
fn an_nft_alone_names_its_owner_and_this_wallet_as_update_authority() {
    let desk = desk();
    let owner = KeypairSigner::new_random().pubkey();
    let asset = KeypairSigner::new_random();
    let landed = desk
        .land(
            "create-core-nft",
            &[],
            &[
                ("nft_name", text("Ape #1")),
                ("uri", text("https://example.com/ape/1.json")),
                ("owner", owner.to_string()),
            ],
            &[("asset", &asset)],
            &[],
        )
        .unwrap_or_else(|logs| panic!("{logs}"));
    let (held_by, kind, authority, name, uri) = read_asset(&desk.data(&asset.pubkey()));
    assert_eq!(held_by, owner);
    assert_eq!((kind, authority), (BY_ADDRESS, desk.me()));
    assert_eq!(
        (&name[..], &uri[..]),
        ("Ape #1", "https://example.com/ape/1.json")
    );
    let sentences = desk.sentences(&landed.instructions);
    eprintln!("{sentences:#?}");
    assert!(sentences[0].contains("Ape #1"), "{}", sentences[0]);
}

#[test]
fn an_nft_in_a_collection_takes_the_collection_as_update_authority() {
    let desk = desk();
    let (key, _) = collection(
        &desk,
        &[
            ("collection_name", text("Apes")),
            ("uri", text("https://example.com/apes.json")),
        ],
    );
    let asset = KeypairSigner::new_random();
    desk.land(
        "create-core-nft",
        &["in_collection"],
        &[
            ("nft_name", text("Ape #2")),
            ("uri", text("https://example.com/ape/2.json")),
            ("collection", key.to_string()),
        ],
        &[("asset", &asset)],
        &[],
    )
    .unwrap_or_else(|logs| panic!("{logs}"));
    let (owner, kind, authority, _, _) = read_asset(&desk.data(&asset.pubkey()));
    assert_eq!(owner, desk.me());
    assert_eq!((kind, authority), (BY_COLLECTION, key));
    let (_, _, _, minted, size) = read_collection(&desk.data(&key));
    assert_eq!((minted, size), (1, 1));
}

#[test]
fn only_the_collections_update_authority_can_add_to_it() {
    let desk = desk();
    let other = KeypairSigner::new_random().pubkey();
    let (key, _) = collection(
        &desk,
        &[
            ("collection_name", text("Theirs")),
            ("uri", text("https://example.com/theirs.json")),
            ("update_authority", other.to_string()),
        ],
    );
    let asset = KeypairSigner::new_random();
    let refused = desk.land(
        "create-core-nft",
        &["in_collection"],
        &[
            ("nft_name", text("Intruder")),
            ("uri", text("https://example.com/x.json")),
            ("collection", key.to_string()),
        ],
        &[("asset", &asset)],
        &[],
    );
    assert!(refused.is_err());
    assert!(!desk.exists(&asset.pubkey()));
}

/// Core keeps 0.0015 SOL in each NFT it creates, over the account's rent,
/// and nothing over a collection's. The templates say so, with the rent
/// in round terms.
#[test]
fn core_keeps_a_fee_on_an_nft_and_none_on_a_collection() {
    let desk = desk();
    let me = desk.me();
    // The account is read before the chain is locked again.
    let rent = |address: &Pubkey| {
        let len = desk.data(address).len();
        desk.node.chain().rent(len)
    };
    let text_of = |id: &str| wallet_core::builtin_template(id).unwrap().toml;

    let asset = KeypairSigner::new_random();
    let before = desk.balance(&me);
    desk.land(
        "create-core-nft",
        &[],
        &[
            ("nft_name", text("Ape #1")),
            ("uri", text("https://example.com/ape/1.json")),
        ],
        &[("asset", &asset)],
        &[],
    )
    .unwrap_or_else(|logs| panic!("{logs}"));
    let fee = desk.balance(&asset.pubkey()) - rent(&asset.pubkey());
    assert_eq!(fee, 1_500_000, "0.0015 SOL");
    assert_eq!(
        before - desk.balance(&me),
        desk.balance(&asset.pubkey()) + 10_000
    );
    // About 0.0012 SOL of rent for a short name and link.
    let held = rent(&asset.pubkey());
    assert_eq!((held + 50_000) / 100_000, 12, "{held}");
    let said = text_of("create-core-nft");
    assert!(said.contains("0.0015 SOL") && said.contains("about 0.0012 SOL"));

    let collection = KeypairSigner::new_random();
    desk.land(
        "create-core-collection",
        &[],
        &[
            ("collection_name", text("Apes")),
            ("uri", text("https://example.com/apes.json")),
        ],
        &[("collection", &collection)],
        &[],
    )
    .unwrap_or_else(|logs| panic!("{logs}"));
    let held = desk.balance(&collection.pubkey());
    assert_eq!(held, rent(&collection.pubkey()), "no fee");
    assert_eq!((held + 500_000) / 1_000_000, 1, "{held}");
    let said = text_of("create-core-collection");
    assert!(said.contains("about 0.001 SOL") && said.contains("no fee"));
}
