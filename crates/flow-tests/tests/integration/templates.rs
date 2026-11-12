//! The templates that ship with the app, against the real Token, Token-2022
//! and associated-token programs.
//!
//! Each run expands a template with the facts read from the chain through
//! `wallet_flows::template_facts`, builds every block with the embedded
//! IDLs, signs with the wallet and the mint's new keypair, lands it, and
//! reads the mint back. Creating a Token-2022 mint is what checks the size
//! table: `InitializeMint` refuses an account whose length is not exactly
//! the one its extensions take, and the account must hold the rent of its
//! size once token metadata has grown it.
//!
//! Token-2022 refuses some extensions together, but none of the shipped
//! options: the program the VM carries (11.0.0) takes all six at once, a
//! transfer fee on a non-transferable mint included. Of the other extensions
//! the size table knows, scaled UI amount cannot go with interest-bearing,
//! a transfer fee beside a confidential transfer config needs the
//! confidential transfer fee too, and that fee needs both.

use crate::support::node::Node;
use crate::support::{logs, Chain, SOL};
use solana_pubkey::Pubkey;
use std::collections::BTreeMap;
use std::sync::Mutex;
use wallet_core::template::{expand, Answers, Env, Expansion, Facts, Template};
use wallet_core::{
    builtin_template, IdlNetwork, IdlStore, KeypairSigner, MemoryBodies, MemoryStore, WalletSigner,
    TOKEN_2022_PROGRAM_ID, TOKEN_PROGRAM_ID,
};

const ACCOUNT_LEN: usize = 165;

// Token-2022 extension types.
const TRANSFER_FEE_CONFIG: u16 = 1;
const MINT_CLOSE_AUTHORITY: u16 = 3;
const NON_TRANSFERABLE: u16 = 9;
const INTEREST_BEARING: u16 = 10;
const PERMANENT_DELEGATE: u16 = 12;
const TRANSFER_HOOK: u16 = 14;
const METADATA_POINTER: u16 = 18;
const TOKEN_METADATA: u16 = 19;
const GROUP_POINTER: u16 = 20;
const GROUP_MEMBER_POINTER: u16 = 22;
const SCALED_UI_AMOUNT: u16 = 25;
const PAUSABLE: u16 = 26;

struct Desk {
    node: Node,
    wallet: KeypairSigner,
    idls: IdlStore<MemoryStore, MemoryBodies>,
}

/// What one landed template left on the chain.
struct Made {
    mint: Pubkey,
    expansion: Expansion,
}

impl Desk {
    fn new() -> Self {
        let wallet = KeypairSigner::new_random();
        let mut chain = Chain::new();
        chain.fund(&wallet.pubkey(), 10 * SOL);
        Self {
            node: Node(Mutex::new(chain)),
            wallet,
            idls: IdlStore::open_on(
                MemoryStore::new(),
                MemoryBodies::new(),
                IdlNetwork::Localnet,
            )
            .with_builtins(),
        }
    }

    fn me(&self) -> Pubkey {
        self.wallet.pubkey()
    }

    /// Expands, asking the chain for each fact until none is needed.
    fn expand(&self, template: &Template, answers: &Answers, env: &Env) -> Expansion {
        let mut facts = Facts::default();
        for _ in 0..4 {
            let expansion = expand(template, answers, env, &facts, &self.idls)
                .unwrap_or_else(|problems| panic!("{problems:#?}"));
            if expansion.needs.is_empty() {
                assert!(expansion.missing.is_empty(), "{:?}", expansion.missing);
                return expansion;
            }
            let read =
                pollster::block_on(wallet_flows::template_facts(&self.node, &expansion.needs))
                    .unwrap();
            facts.merge(read);
        }
        panic!("facts kept being needed");
    }

    /// Expands, builds every block with its IDL, and lands the transaction.
    fn land(
        &self,
        template: &Template,
        answers: &Answers,
        keys: &[&KeypairSigner],
    ) -> Result<Expansion, String> {
        let env = Env {
            wallet: Some(self.me()),
            fee_payer: Some(self.me()),
            keypairs: keys
                .iter()
                .map(|key| ("mint".to_string(), key.pubkey()))
                .collect(),
        };
        let mut expansion = self.expand(template, answers, &env);
        let mut instructions = Vec::new();
        for block in &mut expansion.blocks {
            let program = block.draft.program_id.expect("every program is known");
            let client = self.idls.program_client(&program).unwrap();
            instructions.push(
                block
                    .draft
                    .build(&client)
                    .unwrap_or_else(|err| panic!("block {}: {err}", block.number)),
            );
        }
        let mut signers = vec![&self.wallet];
        signers.extend(keys.iter().copied());
        self.node
            .chain()
            .send(&signers, instructions)
            .map_err(|failure| logs(&failure))?;
        Ok(expansion)
    }

    /// Lands a template that creates a mint, with these options on and
    /// these answers.
    fn create(&self, id: &str, options: &[&str], answers: &[(&str, String)]) -> Made {
        self.try_create(id, options, answers)
            .unwrap_or_else(|logs| panic!("{id} with {options:?}: {logs}"))
    }

    fn try_create(
        &self,
        id: &str,
        options: &[&str],
        answers: &[(&str, String)],
    ) -> Result<Made, String> {
        let template = builtin_template(id).unwrap().template().unwrap();
        self.create_from(&template, options, answers)
    }

    fn create_from(
        &self,
        template: &Template,
        options: &[&str],
        answers: &[(&str, String)],
    ) -> Result<Made, String> {
        let mut form = Answers::for_template(template);
        for option in options {
            form = form.turn_on(option);
        }
        for (input, text) in answers {
            form = form.answer(input, text);
        }
        let mint = KeypairSigner::new_random();
        let expansion = self.land(template, &form, &[&mint])?;
        Ok(Made {
            mint: mint.pubkey(),
            expansion,
        })
    }

    fn data(&self, address: &Pubkey) -> Vec<u8> {
        self.node.chain().data(address)
    }

    fn owner(&self, address: &Pubkey) -> Pubkey {
        self.node.chain().account(address).expect("exists").owner
    }

    /// A token account's balance.
    fn balance(&self, account: &Pubkey) -> u64 {
        u64::from_le_bytes(self.data(account)[64..72].try_into().unwrap())
    }
}

/// A mint as the program wrote it.
struct MintState {
    len: usize,
    lamports: u64,
    authority: Option<Pubkey>,
    supply: u64,
    decimals: u8,
    freeze_authority: Option<Pubkey>,
    extensions: BTreeMap<u16, Vec<u8>>,
}

fn coption(bytes: &[u8]) -> Option<Pubkey> {
    (bytes[0] == 1).then(|| Pubkey::try_from(&bytes[4..36]).unwrap())
}

fn read_mint(desk: &Desk, mint: &Pubkey) -> MintState {
    let account = desk.node.chain().account(mint).expect("the mint exists");
    let data = account.data;
    assert_eq!(data[45], 1, "initialized");
    let mut extensions = BTreeMap::new();
    if data.len() > 82 {
        assert_eq!(data[ACCOUNT_LEN], 1, "a mint's account type");
        let mut at = ACCOUNT_LEN + 1;
        while at + 4 <= data.len() {
            let kind = u16::from_le_bytes([data[at], data[at + 1]]);
            let len = usize::from(u16::from_le_bytes([data[at + 2], data[at + 3]]));
            if kind == 0 {
                break;
            }
            let value = data[at + 4..at + 4 + len].to_vec();
            assert!(extensions.insert(kind, value).is_none(), "{kind} twice");
            at += 4 + len;
        }
        assert_eq!(at, data.len(), "the extensions fill the account exactly");
    }
    MintState {
        len: data.len(),
        lamports: account.lamports,
        authority: coption(&data[0..36]),
        supply: u64::from_le_bytes(data[36..44].try_into().unwrap()),
        decimals: data[44],
        freeze_authority: coption(&data[46..82]),
        extensions,
    }
}

fn key(bytes: &[u8]) -> Pubkey {
    Pubkey::try_from(bytes).unwrap()
}

/// Length-prefixed strings, from `at`.
fn strings(bytes: &[u8], mut at: usize, count: usize) -> (Vec<String>, usize) {
    let mut out = Vec::new();
    for _ in 0..count {
        let len = u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
        out.push(String::from_utf8(bytes[at + 4..at + 4 + len].to_vec()).unwrap());
        at += 4 + len;
    }
    (out, at)
}

/// The size the template created the mint at and funded it for.
fn sizes(made: &Made) -> (usize, u64) {
    let create = &made.expansion.blocks[0].draft;
    assert_eq!(create.instruction, "createAccount");
    let space: usize = create.argument("space").parse().unwrap();
    let lamports = wallet_core::parse_amount(create.argument("lamports"), 9).unwrap();
    (space, lamports)
}

/// The mint holds the rent of its final size: no more, no less.
fn exactly_funded(desk: &Desk, state: &MintState, made: &Made) {
    let (_, lamports) = sizes(made);
    assert_eq!(state.lamports, lamports);
    assert_eq!(lamports, desk.node.chain().rent(state.len));
}

#[test]
fn create_token_makes_a_mint_with_its_authorities() {
    let desk = Desk::new();
    let freezer = KeypairSigner::new_random().pubkey();
    let made = desk.create(
        "create-token",
        &[],
        &[
            ("decimals", "6".into()),
            ("freeze_authority", freezer.to_string()),
        ],
    );
    assert_eq!(desk.owner(&made.mint), TOKEN_PROGRAM_ID);
    let state = read_mint(&desk, &made.mint);
    assert_eq!(state.len, 82);
    assert_eq!(state.decimals, 6);
    assert_eq!(state.authority, Some(desk.me()));
    assert_eq!(state.freeze_authority, Some(freezer));
    assert_eq!(state.supply, 0);
    exactly_funded(&desk, &state, &made);

    // The defaults: 9 decimals and no freeze authority.
    let made = desk.create("create-token", &[], &[]);
    let state = read_mint(&desk, &made.mint);
    assert_eq!(state.decimals, 9);
    assert_eq!(state.freeze_authority, None);
}

#[test]
fn create_token_can_mint_a_first_supply() {
    let desk = Desk::new();
    let to = KeypairSigner::new_random().pubkey();
    let made = desk.create(
        "create-token",
        &["first_supply"],
        &[
            ("decimals", "2".into()),
            ("to", to.to_string()),
            ("amount", "12.34".into()),
        ],
    );
    let state = read_mint(&desk, &made.mint);
    assert_eq!(state.supply, 1234);
    let account = wallet_core::spl::associated_token_address(&to, &made.mint, &TOKEN_PROGRAM_ID);
    assert_eq!(desk.balance(&account), 1234);
    assert_eq!(desk.owner(&account), TOKEN_PROGRAM_ID);
}

fn metadata_answers() -> Vec<(&'static str, String)> {
    vec![
        ("token_name", "Airlock Test Token".into()),
        ("symbol", "ALT".into()),
        ("uri", "https://example.com/airlock-test-token.json".into()),
    ]
}

/// Every answer every option of create-token-2022 asks for.
fn all_answers(to: &Pubkey, closer: &Pubkey, delegate: &Pubkey) -> Vec<(&'static str, String)> {
    let mut answers = metadata_answers();
    answers.extend([
        ("decimals", "6".to_string()),
        ("fee", "1.25".to_string()),
        ("max_fee", "5000".to_string()),
        ("close_authority", closer.to_string()),
        ("delegate", delegate.to_string()),
        ("to", to.to_string()),
        ("amount", "1000".to_string()),
    ]);
    answers
}

/// Checks each extension `options` turned on, with what `all_answers` gave.
fn assert_extensions(
    desk: &Desk,
    made: &Made,
    options: &[&str],
    to: &Pubkey,
    closer: &Pubkey,
    delegate: &Pubkey,
) {
    let me = desk.me();
    let state = read_mint(desk, &made.mint);
    let on = |option: &str| options.contains(&option);
    assert_eq!(desk.owner(&made.mint), TOKEN_2022_PROGRAM_ID);
    assert_eq!(state.decimals, 6);
    assert_eq!(state.authority, Some(me));
    let (space, _) = sizes(made);
    exactly_funded(desk, &state, made);

    let mut expected = Vec::new();
    if on("metadata") {
        expected.extend([METADATA_POINTER, TOKEN_METADATA]);
        let pointer = &state.extensions[&METADATA_POINTER];
        assert_eq!(key(&pointer[0..32]), me);
        assert_eq!(key(&pointer[32..64]), made.mint);
        let metadata = &state.extensions[&TOKEN_METADATA];
        assert_eq!(key(&metadata[0..32]), me, "update authority");
        assert_eq!(key(&metadata[32..64]), made.mint);
        let (text, end) = strings(metadata, 64, 3);
        assert_eq!(
            text,
            metadata_answers()
                .into_iter()
                .map(|(_, text)| text)
                .collect::<Vec<_>>()
        );
        assert_eq!(
            &metadata[end..],
            &0u32.to_le_bytes(),
            "no additional fields"
        );
        // Created without the metadata, which grew the account itself.
        assert_eq!(space, state.len - 4 - metadata.len());
    } else {
        assert_eq!(space, state.len);
    }
    if on("transfer_fee") {
        expected.push(TRANSFER_FEE_CONFIG);
        let config = &state.extensions[&TRANSFER_FEE_CONFIG];
        assert_eq!(key(&config[0..32]), me);
        assert_eq!(key(&config[32..64]), me);
        for fee in [72usize, 90] {
            assert_eq!(&config[fee + 8..fee + 16], &5000u64.to_le_bytes());
            assert_eq!(&config[fee + 16..fee + 18], &125u16.to_le_bytes());
        }
    }
    if on("closable") {
        expected.push(MINT_CLOSE_AUTHORITY);
        assert_eq!(key(&state.extensions[&MINT_CLOSE_AUTHORITY]), *closer);
    }
    if on("permanent_delegate") {
        expected.push(PERMANENT_DELEGATE);
        assert_eq!(key(&state.extensions[&PERMANENT_DELEGATE]), *delegate);
    }
    if on("non_transferable") {
        expected.push(NON_TRANSFERABLE);
        assert!(state.extensions[&NON_TRANSFERABLE].is_empty());
    }
    expected.sort();
    assert_eq!(
        state.extensions.keys().copied().collect::<Vec<_>>(),
        expected,
        "{options:?}"
    );

    if on("first_supply") {
        assert_eq!(state.supply, 1_000_000_000);
        let account =
            wallet_core::spl::associated_token_address(to, &made.mint, &TOKEN_2022_PROGRAM_ID);
        assert_eq!(desk.owner(&account), TOKEN_2022_PROGRAM_ID);
        assert_eq!(desk.balance(&account), 1_000_000_000);
    } else {
        assert_eq!(state.supply, 0);
    }
}

const TOKEN_2022_OPTIONS: [&str; 6] = [
    "metadata",
    "transfer_fee",
    "closable",
    "permanent_delegate",
    "non_transferable",
    "first_supply",
];

fn create_2022(desk: &Desk, options: &[&str]) {
    let to = KeypairSigner::new_random().pubkey();
    let closer = KeypairSigner::new_random().pubkey();
    let delegate = KeypairSigner::new_random().pubkey();
    let answers = all_answers(&to, &closer, &delegate);
    let made = desk.create("create-token-2022", options, &answers);
    assert_extensions(desk, &made, options, &to, &closer, &delegate);
}

#[test]
fn create_token_2022_with_no_option_is_a_bare_mint() {
    let desk = Desk::new();
    create_2022(&desk, &[]);
    let made = desk.create("create-token-2022", &[], &[]);
    assert_eq!(read_mint(&desk, &made.mint).len, 82);
}

#[test]
fn create_token_2022_with_each_option_alone() {
    let desk = Desk::new();
    for option in TOKEN_2022_OPTIONS {
        create_2022(&desk, &[option]);
    }
}

#[test]
fn create_token_2022_with_every_option_together() {
    let desk = Desk::new();
    create_2022(&desk, &TOKEN_2022_OPTIONS);
}

#[test]
fn mint_tokens_mints_to_a_new_or_existing_account_of_either_token_program() {
    let desk = Desk::new();
    let template = builtin_template("mint-tokens").unwrap().template().unwrap();
    for (id, program) in [
        ("create-token", TOKEN_PROGRAM_ID),
        ("create-token-2022", TOKEN_2022_PROGRAM_ID),
    ] {
        let made = desk.create(id, &[], &[("decimals", "3".into())]);
        let mint = made.mint;
        let to = KeypairSigner::new_random().pubkey();
        let account = wallet_core::spl::associated_token_address(&to, &mint, &program);
        let answers = Answers::for_template(&template)
            .answer("mint", &mint.to_string())
            .answer("to", &to.to_string())
            .answer("amount", "1.5");
        // No account yet: it is created.
        let expansion = desk.land(&template, &answers, &[]).unwrap();
        assert_eq!(expansion.blocks[1].draft.program_id, Some(program));
        assert_eq!(desk.balance(&account), 1_500);
        // Then it exists, and the same template mints into it again.
        desk.land(&template, &answers, &[]).unwrap();
        assert_eq!(desk.balance(&account), 3_000);
        assert_eq!(read_mint(&desk, &mint).supply, 3_000);

        // To the wallet by default.
        let mine = wallet_core::spl::associated_token_address(&desk.me(), &mint, &program);
        let answers = Answers::for_template(&template)
            .answer("mint", &mint.to_string())
            .answer("amount", "0.001");
        desk.land(&template, &answers, &[]).unwrap();
        assert_eq!(desk.balance(&mine), 1);
    }
}

/// The size table's extensions that no shipped template uses, each checked
/// against the program as the shipped ones are.
const MORE_EXTENSIONS: &str = r#"
format = 1
name = "Every extension"

[programs]
system = "11111111111111111111111111111111"
token = "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb"

[[option]]
name = "interest"
label = "Interest"

[[option]]
name = "hook"
label = "Transfer hook"

[[option]]
name = "group"
label = "Group pointer"

[[option]]
name = "member"
label = "Group member pointer"

[[option]]
name = "pausable"
label = "Pausable"

[[option]]
name = "scaled"
label = "Scaled UI amount"

[values]
funded = "token-2022-mint-funded-space $mint"
lamports = "rent-for $funded"
mint = "new-keypair"
space = "token-2022-mint-space $mint"

[[block]]
program = "$system"
instruction = "createAccount"
accounts = { newAccount = "$mint", payer = "$fee-payer" }
arguments = { lamports = "$lamports", programAddress = "$token", space = "$space" }

[[block]]
when = "interest"
program = "$token"
instruction = "initializeInterestBearingMint"
accounts = { mint = "$mint" }
arguments = { rate = "2.5", rateAuthority = "$wallet" }

[[block]]
when = "hook"
program = "$token"
instruction = "initializeTransferHook"
accounts = { mint = "$mint" }
arguments = { authority = "$wallet", programId = "$system" }

[[block]]
when = "group"
program = "$token"
instruction = "initializeGroupPointer"
accounts = { mint = "$mint" }
arguments = { authority = "$wallet", groupAddress = "$mint" }

[[block]]
when = "member"
program = "$token"
instruction = "initializeGroupMemberPointer"
accounts = { mint = "$mint" }
arguments = { authority = "$wallet", memberAddress = "$mint" }

[[block]]
when = "pausable"
program = "$token"
instruction = "initializePausableConfig"
accounts = { mint = "$mint" }
arguments = { authority = "$wallet" }

[[block]]
when = "scaled"
program = "$token"
instruction = "initializeScaledUiAmountMint"
accounts = { mint = "$mint" }
arguments = { authority = "$wallet", multiplier = "1.5" }

[[block]]
program = "$token"
instruction = "initializeMint2"
accounts = { mint = "$mint" }
arguments = { decimals = "0", mintAuthority = "$wallet" }
"#;

#[test]
fn every_other_extension_in_the_size_table_is_sized_as_the_program_sizes_it() {
    let desk = Desk::new();
    let template = Template::parse(MORE_EXTENSIONS).unwrap();
    let me = desk.me();
    let cases: [(&str, u16, usize); 6] = [
        ("interest", INTEREST_BEARING, 52),
        ("hook", TRANSFER_HOOK, 64),
        ("group", GROUP_POINTER, 64),
        ("member", GROUP_MEMBER_POINTER, 64),
        ("pausable", PAUSABLE, 33),
        ("scaled", SCALED_UI_AMOUNT, 56),
    ];
    for (option, kind, len) in cases {
        let made = desk
            .create_from(&template, &[option], &[])
            .unwrap_or_else(|logs| panic!("{option}: {logs}"));
        let state = read_mint(&desk, &made.mint);
        assert_eq!(state.extensions.len(), 1, "{option}");
        assert_eq!(state.extensions[&kind].len(), len, "{option}");
        assert_eq!(key(&state.extensions[&kind][0..32]), me, "{option}");
        exactly_funded(&desk, &state, &made);
    }
    // Together, but for scaled UI amount, which refuses interest.
    let together = ["interest", "hook", "group", "member", "pausable"];
    let made = desk
        .create_from(&template, &together, &[])
        .unwrap_or_else(|logs| panic!("{logs}"));
    let state = read_mint(&desk, &made.mint);
    assert_eq!(state.extensions.len(), together.len());
    exactly_funded(&desk, &state, &made);
    assert!(desk
        .create_from(&template, &["interest", "scaled"], &[])
        .is_err());
}

// Token-2022 extension types of the extensions below.
const CONFIDENTIAL_TRANSFER_MINT: u16 = 4;
const DEFAULT_ACCOUNT_STATE: u16 = 6;
const CONFIDENTIAL_TRANSFER_FEE_CONFIG: u16 = 16;

/// Default account state and confidential transfers, which no shipped
/// template offers, each checked against the program as the shipped
/// extensions are. The freeze authority is the wallet unless cleared: a
/// mint whose accounts start frozen needs one.
const STATE_AND_CONFIDENTIAL: &str = r#"
format = 1
name = "Default state and confidential transfers"

[programs]
system = "11111111111111111111111111111111"
token = "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb"

[[option]]
name = "default_state"
label = "Default account state"

[[option]]
name = "transfer_fee"
label = "Transfer fee"

[[option]]
name = "confidential"
label = "Confidential transfers"

[[option]]
name = "confidential_fee"
label = "Confidential transfer fee"

[[input]]
name = "freezer"
label = "Freeze authority"
optional = true
default = "$wallet"

[[input]]
name = "state"
label = "State"
option = "default_state"

[[input]]
name = "auditor"
label = "Auditor"
optional = true
option = "confidential"

[[input]]
name = "withdraw_key"
label = "Withdraw key"
option = "confidential_fee"

[values]
funded = "token-2022-mint-funded-space $mint"
lamports = "rent-for $funded"
mint = "new-keypair"
space = "token-2022-mint-space $mint"

[[block]]
program = "$system"
instruction = "createAccount"
accounts = { newAccount = "$mint", payer = "$fee-payer" }
arguments = { lamports = "$lamports", programAddress = "$token", space = "$space" }

[[block]]
when = "default_state"
program = "$token"
instruction = "initializeDefaultAccountState"
accounts = { mint = "$mint" }
arguments = { state = "$state" }

[[block]]
when = "transfer_fee"
program = "$token"
instruction = "initializeTransferFeeConfig"
accounts = { mint = "$mint" }
arguments = { transferFeeBasisPoints = "1", maximumFee = "100", transferFeeConfigAuthority = "$wallet", withdrawWithheldAuthority = "$wallet" }

[[block]]
when = "confidential"
program = "$token"
instruction = "initializeConfidentialTransferMint"
accounts = { mint = "$mint" }
arguments = { authority = "$wallet", autoApproveNewAccounts = "true", auditorElgamalPubkey = "$auditor" }

[[block]]
when = "confidential_fee"
program = "$token"
instruction = "initializeConfidentialTransferFee"
accounts = { mint = "$mint" }
arguments = { authority = "$wallet", withdrawWithheldAuthorityElGamalPubkey = "$withdraw_key" }

[[block]]
program = "$token"
instruction = "initializeMint2"
accounts = { mint = "$mint" }
arguments = { decimals = "0", mintAuthority = "$wallet", freezeAuthority = "$freezer" }
"#;

/// The Ristretto basepoint, compressed: a valid ElGamal public key.
const BASEPOINT: &str = "GGumV86X6FZzHRo8bLvbW2LJ3PZ45EqRPWeogP8ufcm3";

fn state_and_confidential() -> Template {
    Template::parse(STATE_AND_CONFIDENTIAL).unwrap()
}

#[test]
fn default_account_state_is_sized_as_the_program_sizes_it() {
    let desk = Desk::new();
    let template = state_and_confidential();
    for (state, byte) in [("initialized", 1), ("frozen", 2)] {
        let made = desk
            .create_from(&template, &["default_state"], &[("state", state.into())])
            .unwrap_or_else(|logs| panic!("{state}: {logs}"));
        let mint = read_mint(&desk, &made.mint);
        assert_eq!(mint.extensions.len(), 1, "{state}");
        assert_eq!(
            mint.extensions[&DEFAULT_ACCOUNT_STATE],
            vec![byte],
            "{state}"
        );
        assert_eq!(mint.freeze_authority, Some(desk.me()));
        exactly_funded(&desk, &mint, &made);
    }
    // The program takes neither a state accounts cannot be in, nor frozen
    // accounts on a mint nobody can thaw.
    let refused = |answers: &[(&str, String)]| {
        desk.create_from(&template, &["default_state"], answers)
            .err()
            .unwrap_or_else(|| panic!("{answers:?} landed"))
    };
    let logs = refused(&[("state", "uninitialized".into())]);
    assert!(logs.contains("Invalid account state"), "{logs}");
    let logs = refused(&[("state", "frozen".into()), ("freezer", String::new())]);
    assert!(logs.contains("cannot freeze accounts"), "{logs}");
}

#[test]
fn a_confidential_mint_is_sized_as_the_program_sizes_it() {
    let desk = Desk::new();
    let template = state_and_confidential();
    let basepoint: Pubkey = BASEPOINT.parse().unwrap();
    assert_eq!(basepoint.to_bytes()[..4], [0xe2, 0xf2, 0xae, 0x0a]);
    // Not a point: the program keeps the 32 bytes without reading them.
    let not_a_point = Pubkey::new_from_array([255; 32]);
    for auditor in [None, Some(basepoint), Some(not_a_point)] {
        let answers: Vec<(&str, String)> = auditor
            .map(|key| vec![("auditor", key.to_string())])
            .unwrap_or_default();
        let made = desk
            .create_from(&template, &["confidential"], &answers)
            .unwrap_or_else(|logs| panic!("{auditor:?}: {logs}"));
        let mint = read_mint(&desk, &made.mint);
        assert_eq!(mint.extensions.len(), 1);
        let config = &mint.extensions[&CONFIDENTIAL_TRANSFER_MINT];
        assert_eq!(config.len(), 65);
        assert_eq!(key(&config[0..32]), desk.me(), "authority");
        assert_eq!(config[32], 1, "approves new accounts");
        let held = auditor.map_or([0; 32], |key| key.to_bytes());
        assert_eq!(config[33..65], held, "auditor");
        exactly_funded(&desk, &mint, &made);
    }
}

/// The confidential transfer fee goes on a mint with a transfer fee and a
/// confidential transfer config. The program refuses it without both, and
/// refuses the two together without it.
#[test]
fn the_confidential_transfer_fee_is_sized_as_the_program_sizes_it() {
    let desk = Desk::new();
    let template = state_and_confidential();
    let basepoint: Pubkey = BASEPOINT.parse().unwrap();
    let key_answer = [("withdraw_key", BASEPOINT.to_string())];
    let all = ["transfer_fee", "confidential", "confidential_fee"];
    let made = desk
        .create_from(&template, &all, &key_answer)
        .unwrap_or_else(|logs| panic!("{logs}"));
    let mint = read_mint(&desk, &made.mint);
    assert_eq!(
        mint.extensions.keys().copied().collect::<Vec<_>>(),
        vec![
            TRANSFER_FEE_CONFIG,
            CONFIDENTIAL_TRANSFER_MINT,
            CONFIDENTIAL_TRANSFER_FEE_CONFIG
        ]
    );
    let config = &mint.extensions[&CONFIDENTIAL_TRANSFER_FEE_CONFIG];
    assert_eq!(config.len(), 129);
    assert_eq!(key(&config[0..32]), desk.me(), "authority");
    assert_eq!(key(&config[32..64]), basepoint, "withdraw key");
    assert_eq!(config[64], 1, "harvest to the mint");
    assert!(
        config[65..].iter().all(|byte| *byte == 0),
        "nothing withheld"
    );
    exactly_funded(&desk, &mint, &made);

    for options in [
        &["transfer_fee", "confidential"][..],
        &["confidential_fee"],
        &["transfer_fee", "confidential_fee"],
        &["confidential", "confidential_fee"],
    ] {
        let logs = desk
            .create_from(&template, options, &key_answer)
            .err()
            .unwrap_or_else(|| panic!("{options:?} landed"));
        assert!(logs.contains("invalid combination of extensions"), "{logs}");
    }
}
