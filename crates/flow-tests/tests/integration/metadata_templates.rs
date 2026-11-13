//! The shipped templates on Metaplex Token Metadata, against its mainnet
//! build: a name and symbol for a token, made with it or added to one that
//! exists, and an NFT of the classic standard.
//!
//! Each run lands what the template writes and reads back the metadata
//! account, the master edition, the mint and the token account as the
//! program left them, and what Review and Sign says of each instruction.

use crate::support::templates::{Reader, TemplateDesk};
use solana_pubkey::Pubkey;
use wallet_core::{IdlNetwork, KeypairSigner, WalletSigner, TOKEN_PROGRAM_ID};

const TOKEN_METADATA: Pubkey =
    Pubkey::from_str_const("metaqbxxUerdq28cj1RbAWkYQm3ybzjb6a8bt518x1s");

// Token Metadata's account keys and token standards.
const METADATA_V1: u8 = 4;
const MASTER_EDITION_V2: u8 = 6;
const NON_FUNGIBLE: u8 = 0;
const FUNGIBLE: u8 = 2;

fn desk() -> TemplateDesk {
    TemplateDesk::new(
        IdlNetwork::Mainnet,
        &[(TOKEN_METADATA, "mpl_token_metadata.so")],
        &[],
    )
}

fn metadata_of(mint: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[b"metadata", TOKEN_METADATA.as_ref(), mint.as_ref()],
        &TOKEN_METADATA,
    )
    .0
}

fn edition_of(mint: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[
            b"metadata",
            TOKEN_METADATA.as_ref(),
            mint.as_ref(),
            b"edition",
        ],
        &TOKEN_METADATA,
    )
    .0
}

/// A metadata account, as far as these templates write it.
#[derive(Debug)]
struct Metadata {
    update_authority: Pubkey,
    mint: Pubkey,
    name: String,
    symbol: String,
    uri: String,
    royalty: u16,
    creators: Option<Vec<(Pubkey, bool, u8)>>,
    mutable: bool,
    token_standard: Option<u8>,
    collection: Option<(bool, Pubkey)>,
}

fn read_metadata(data: &[u8]) -> Metadata {
    let mut r = Reader::new(data, 0);
    assert_eq!(r.u8(), METADATA_V1);
    let update_authority = r.pubkey();
    let mint = r.pubkey();
    let name = r.string();
    let symbol = r.string();
    let uri = r.string();
    let royalty = r.u16();
    let creators = r.some().then(|| {
        (0..r.u32())
            .map(|_| (r.pubkey(), r.bool(), r.u8()))
            .collect()
    });
    let _primary_sale_happened = r.bool();
    let mutable = r.bool();
    if r.some() {
        r.u8(); // edition nonce
    }
    let token_standard = r.some().then(|| r.u8());
    let collection = r.some().then(|| (r.bool(), r.pubkey()));
    Metadata {
        update_authority,
        mint,
        name,
        symbol,
        uri,
        royalty,
        creators,
        mutable,
        token_standard,
        collection,
    }
}

/// A mint's authorities, supply and decimals.
fn read_mint(data: &[u8]) -> (Option<Pubkey>, u64, u8, Option<Pubkey>) {
    let mut r = Reader::new(data, 0);
    let authority = (r.u32() == 1).then(|| r.pubkey());
    r.at = 36;
    let supply = r.u64();
    let decimals = r.u8();
    r.at = 46;
    let freeze = (r.u32() == 1).then(|| r.pubkey());
    (authority, supply, decimals, freeze)
}

fn text(value: &str) -> String {
    value.to_string()
}

#[test]
fn a_new_token_with_a_name_and_symbol_lands_its_metadata() {
    let desk = desk();
    let mint = KeypairSigner::new_random();
    let landed = desk
        .land(
            "create-token",
            &["metadata", "first_supply"],
            &[
                ("decimals", text("6")),
                ("token_name", text("Gold")),
                ("symbol", text("GLD")),
                ("uri", text("https://example.com/gold.json")),
                ("amount", text("1000")),
            ],
            &[("mint", &mint)],
            &[],
        )
        .unwrap_or_else(|logs| panic!("{logs}"));
    let metadata = read_metadata(&desk.data(&metadata_of(&mint.pubkey())));
    assert_eq!(metadata.mint, mint.pubkey());
    assert_eq!(metadata.update_authority, desk.me());
    assert_eq!(
        (&metadata.name[..], &metadata.symbol[..], &metadata.uri[..]),
        ("Gold", "GLD", "https://example.com/gold.json")
    );
    assert_eq!(metadata.token_standard, Some(FUNGIBLE));
    assert!(metadata.mutable);
    assert_eq!(metadata.creators, None);
    // The mint keeps its authority and the first supply is minted after.
    let (authority, supply, decimals, _) = read_mint(&desk.data(&mint.pubkey()));
    assert_eq!(authority, Some(desk.me()));
    assert_eq!((supply, decimals), (1_000_000_000, 6));

    let sentences = desk.sentences(&landed.instructions);
    eprintln!("{sentences:#?}");
    assert_eq!(sentences.len(), 5);
    assert!(sentences[2].contains("Gold"), "{}", sentences[2]);
    assert!(sentences[2].contains("GLD"), "{}", sentences[2]);
}

#[test]
fn a_name_and_symbol_added_to_a_token_names_its_update_authority() {
    let desk = desk();
    let mint = KeypairSigner::new_random();
    desk.land(
        "create-token",
        &[],
        &[("decimals", text("9"))],
        &[("mint", &mint)],
        &[],
    )
    .unwrap_or_else(|logs| panic!("{logs}"));
    let keeper = KeypairSigner::new_random().pubkey();
    let landed = desk
        .land(
            "add-token-metadata",
            &[],
            &[
                ("mint", mint.pubkey().to_string()),
                ("token_name", text("Silver")),
                ("symbol", text("SLV")),
                ("uri", text("https://example.com/silver.json")),
                ("update_authority", keeper.to_string()),
                ("mutable", text("false")),
            ],
            &[],
            &[],
        )
        .unwrap_or_else(|logs| panic!("{logs}"));
    let metadata = read_metadata(&desk.data(&metadata_of(&mint.pubkey())));
    assert_eq!(metadata.update_authority, keeper);
    assert_eq!(
        (&metadata.name[..], &metadata.symbol[..]),
        ("Silver", "SLV")
    );
    assert!(!metadata.mutable);
    assert_eq!(metadata.token_standard, Some(FUNGIBLE));

    let sentences = desk.sentences(&landed.instructions);
    eprintln!("{sentences:#?}");
    assert!(sentences[0].contains("Silver"), "{}", sentences[0]);
    assert!(
        sentences[0].contains(&keeper.to_string()),
        "{}",
        sentences[0]
    );
}

#[test]
fn only_the_mint_authority_can_add_a_name_and_symbol() {
    let desk = desk();
    let mint = KeypairSigner::new_random();
    let other = KeypairSigner::new_random();
    desk.land(
        "create-token",
        &[],
        &[("authority", other.pubkey().to_string())],
        &[("mint", &mint)],
        &[],
    )
    .unwrap_or_else(|logs| panic!("{logs}"));
    // The wallet in use signs as the mint authority it is not.
    let refused = desk
        .land(
            "add-token-metadata",
            &[],
            &[
                ("mint", mint.pubkey().to_string()),
                ("token_name", text("Fake")),
                ("symbol", text("FK")),
                ("uri", text("https://example.com/fake.json")),
            ],
            &[],
            &[],
        )
        .err()
        .expect("refused");
    assert!(refused.contains("authority"), "{refused}");
    assert!(!desk.exists(&metadata_of(&mint.pubkey())));
    // Named and signing, the mint authority can.
    desk.land(
        "add-token-metadata",
        &[],
        &[
            ("mint", mint.pubkey().to_string()),
            ("authority", other.pubkey().to_string()),
            ("token_name", text("Real")),
            ("symbol", text("RL")),
            ("uri", text("https://example.com/real.json")),
        ],
        &[],
        &[&other],
    )
    .unwrap_or_else(|logs| panic!("{logs}"));
    assert_eq!(
        read_metadata(&desk.data(&metadata_of(&mint.pubkey()))).name,
        "Real"
    );
}

#[test]
fn an_nft_is_one_token_with_its_metadata_master_edition_and_creator() {
    let desk = desk();
    let mint = KeypairSigner::new_random();
    let before = desk.balance(&desk.me());
    let landed = desk
        .land(
            "create-nft",
            &[],
            &[
                ("nft_name", text("Ape #1")),
                ("symbol", text("APE")),
                ("uri", text("https://example.com/ape/1.json")),
                ("royalty", text("5.5")),
            ],
            &[("mint", &mint)],
            &[],
        )
        .unwrap_or_else(|logs| panic!("{logs}"));
    let me = desk.me();
    assert_eq!(
        landed
            .expansion
            .values
            .get("royalty_points")
            .map(String::as_str),
        Some("550")
    );
    // The wallet paid the accounts' rent and Token Metadata's fee.
    let paid = before - desk.balance(&me);
    let metadata_account = metadata_of(&mint.pubkey());
    assert!(paid > desk.balance(&metadata_account), "{paid}");
    let metadata = read_metadata(&desk.data(&metadata_account));
    assert_eq!(metadata.name, "Ape #1");
    assert_eq!(metadata.royalty, 550);
    assert_eq!(metadata.creators, Some(vec![(me, true, 100)]));
    assert_eq!(metadata.token_standard, Some(NON_FUNGIBLE));
    assert_eq!(metadata.update_authority, me);
    assert_eq!(metadata.collection, None);

    // The master edition holds the mint's authorities and allows no prints.
    let edition = edition_of(&mint.pubkey());
    let data = desk.data(&edition);
    let mut r = Reader::new(&data, 0);
    assert_eq!(r.u8(), MASTER_EDITION_V2);
    assert_eq!(r.u64(), 0, "editions printed");
    assert!(r.some());
    assert_eq!(r.u64(), 0, "max supply");
    let (authority, supply, decimals, freeze) = read_mint(&desk.data(&mint.pubkey()));
    assert_eq!((authority, freeze), (Some(edition), Some(edition)));
    assert_eq!((supply, decimals), (1, 0));

    // The one token is in the wallet's own token account.
    let account =
        wallet_core::spl::associated_token_address(&me, &mint.pubkey(), &TOKEN_PROGRAM_ID);
    let data = desk.data(&account);
    assert_eq!(Pubkey::try_from(&data[32..64]).unwrap(), me);
    assert_eq!(u64::from_le_bytes(data[64..72].try_into().unwrap()), 1);

    let sentences = desk.sentences(&landed.instructions);
    eprintln!("{sentences:#?}");
    assert_eq!(sentences.len(), 4);
    assert!(sentences[2].contains("Ape #1"), "{}", sentences[2]);
    assert!(sentences[3].contains("mints 1 of"), "{}", sentences[3]);
    assert!(sentences[2].contains("5.5"), "{}", sentences[2]);
}

#[test]
fn an_nft_can_claim_a_collection_it_is_not_verified_in() {
    let desk = desk();
    let mint = KeypairSigner::new_random();
    let collection = KeypairSigner::new_random().pubkey();
    desk.land(
        "create-nft",
        &["in_collection"],
        &[
            ("nft_name", text("Ape #2")),
            ("symbol", text("APE")),
            ("uri", text("https://example.com/ape/2.json")),
            ("collection", collection.to_string()),
        ],
        &[("mint", &mint)],
        &[],
    )
    .unwrap_or_else(|logs| panic!("{logs}"));
    let metadata = read_metadata(&desk.data(&metadata_of(&mint.pubkey())));
    assert_eq!(metadata.collection, Some((false, collection)));
    assert_eq!(metadata.royalty, 0);
}
