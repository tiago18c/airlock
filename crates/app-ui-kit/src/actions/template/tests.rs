//! A template on Build through the real form and actions, on a window with
//! no screen drawn: opening it, answering it, the facts it needs, and what
//! it turns into.

use super::*;
use crate::actions::build::current_instructions;
use crate::state::forms::{BuildForm, IxDraft};
use crate::state::session::OnlineHooks;
use crate::state::submit::SubmitHandle;
use crate::state::use_session;
use app_core::testing::{kit, Kit};
use app_core::Core;
use dioxus::dioxus_core::{NoOpMutations, ScopeId, VirtualDom};
use solana_pubkey::Pubkey;
use std::cell::Cell;
use std::rc::Rc;
use wallet_core::template::Fact;
use wallet_core::{compile_draft, IdlDraft, Instruction, SolanaRpc, TxSettings};
use wallet_flows::testing::FakeRpc;

const TOKEN_2022: TemplateSource = TemplateSource::Builtin("create-token-2022");
const BLOCKHASH: &str = "4vJ9JU1bJJE96FWSJKvHsmmFADCg4gpZQff4P3bkLKi";

#[derive(Clone)]
struct Held {
    core: Core,
    rpc: Option<Arc<FakeRpc>>,
    session: Rc<Cell<Option<Session>>>,
}

fn app(held: Held) -> Element {
    let core = use_hook(|| CopyValue::new(held.core.clone()));
    let hooks = match held.rpc.clone() {
        Some(rpc) => OnlineHooks::new(
            move || Ok(rpc.clone() as Arc<dyn SolanaRpc>),
            || {},
            || {},
            || Box::pin(async { Ok(String::new()) }),
        ),
        None => OnlineHooks::none(),
    };
    let session = use_session(core, hooks, SubmitHandle::none());
    held.session.set(Some(session));
    rsx! {}
}

/// A window with a vault that holds one hot key, selected.
pub(crate) struct Screen {
    pub(crate) kit: Kit,
    dom: VirtualDom,
    pub(crate) session: Session,
    pub(crate) key: Pubkey,
}

impl Screen {
    /// No node: facts are typed.
    pub(crate) async fn offline() -> Self {
        Self::on(None).await
    }

    pub(crate) async fn online(rpc: FakeRpc) -> Self {
        Self::on(Some(Arc::new(rpc))).await
    }

    async fn on(rpc: Option<Arc<FakeRpc>>) -> Self {
        let kit = kit();
        kit.core.create_vault("pw").unwrap();
        let wallet = wallet_core::WalletRecord::from_keypair_seed("Hot", [7; 32]);
        let key = wallet.address().unwrap();
        let signers = vec![wallet_core::NewSigner {
            pubkey: key,
            label: wallet.label.clone(),
            origin: wallet_core::Origin::Imported,
            wallet,
        }];
        kit.core
            .enroll(
                "add_signers",
                |data| data.add_signers(&signers),
                |book, _| wallet_core::name_signers(book, &signers),
            )
            .unwrap();
        let session = Rc::new(Cell::new(None));
        let mut dom = VirtualDom::new_with_props(
            app,
            Held {
                core: kit.core.clone(),
                rpc,
                session: session.clone(),
            },
        );
        dom.rebuild_in_place();
        let session = session.get().unwrap();
        let mut screen = Self {
            kit,
            dom,
            session,
            key,
        };
        screen.settle().await;
        screen.act(|| screen.session.default_build_fee_payer());
        screen
    }

    pub(crate) fn form(&self) -> BuildForm {
        self.session.forms.build
    }

    pub(crate) fn act<R>(&self, then: impl FnOnce() -> R) -> R {
        self.dom.in_scope(ScopeId::ROOT, then)
    }

    pub(crate) async fn settle(&mut self) {
        for _ in 0..4 {
            let _ = tokio::time::timeout(
                std::time::Duration::from_millis(40),
                self.dom.wait_for_work(),
            )
            .await;
            self.dom.render_immediate(&mut NoOpMutations);
        }
    }

    /// Runs the window past [`FACT_READ_DELAY`], and what it then starts.
    async fn wait_out_the_delay(&mut self) {
        let until = std::time::Instant::now() + FACT_READ_DELAY * 2;
        while std::time::Instant::now() < until {
            self.settle().await;
        }
    }

    pub(crate) fn open(&self, source: TemplateSource) {
        self.act(|| open_template(&self.session, source)).unwrap();
    }

    pub(crate) fn answer(&self, input: &str, text: &str) {
        self.act(|| answer(&self.session, input, text.to_string()));
    }

    fn turn_on(&self, option: &str) {
        self.act(|| set_option(&self.session, option, true));
    }

    pub(crate) fn opened(&self) -> OpenExpansion {
        self.act(|| expand_open(&self.session))
            .expect("a template is open")
    }

    pub(crate) fn expansion(&self) -> Expansion {
        self.opened().expansion.expect("the template checks")
    }

    pub(crate) fn open_template(&self) -> Option<OpenTemplate> {
        self.act(|| self.form().template.peek().clone())
    }

    pub(crate) fn drafts(&self) -> Vec<IxDraft> {
        self.act(|| self.form().drafts.peek().clone())
    }

    pub(crate) fn generated(&self) -> Vec<Pubkey> {
        self.act(|| {
            self.form()
                .generated
                .peek()
                .iter()
                .map(|key| key.pubkey)
                .collect()
        })
    }

    pub(crate) fn error(&self) -> Option<String> {
        self.act(|| self.form().error.peek().clone())
    }

    fn sign_wire(&self) -> Option<Vec<u8>> {
        self.act(|| self.session.forms.sign.wire.peek().clone())
    }

    fn sign_note(&self) -> Option<String> {
        self.act(|| {
            self.session
                .forms
                .sign
                .note
                .peek()
                .as_ref()
                .map(|note| note.plain())
        })
    }

    /// Types the rent of every size the template still needs.
    fn type_rent(&self) {
        for fact in self.expansion().needs {
            self.act(|| type_fact(&self.session, fact, "2000000".to_string()));
        }
    }

    fn compile(&self) {
        self.act(|| {
            let compiling = Signal::new(false);
            compile_template(self.session, compiling)
        });
    }
}

fn block<'a>(expansion: &'a Expansion, instruction: &str) -> &'a IdlDraft {
    &expansion
        .blocks
        .iter()
        .find(|block| block.draft.instruction == instruction)
        .unwrap_or_else(|| panic!("no {instruction} block"))
        .draft
}

fn instructions_of(expansion: &Expansion) -> Vec<String> {
    expansion
        .blocks
        .iter()
        .map(|block| block.draft.instruction.clone())
        .collect()
}

/// The template, answered so that it builds: a name and symbol, and a
/// transfer fee.
fn fill_token_2022(screen: &Screen) {
    screen.open(TOKEN_2022);
    screen.turn_on("metadata");
    screen.turn_on("transfer_fee");
    screen.answer("token_name", "Airlock test");
    screen.answer("symbol", "AIR");
    screen.answer("uri", "https://example.com/air.json");
    screen.answer("fee", "0.5");
    screen.answer("max_fee", "1000");
}

#[tokio::test]
async fn a_built_in_template_opens_in_place_of_the_blocks() {
    let screen = Screen::offline().await;
    screen.act(|| {
        screen
            .form()
            .add_instruction(None, IxBody::Idl(IdlDraft::empty()))
    });
    assert_eq!(screen.drafts().len(), 1);

    screen.open(TOKEN_2022);
    let open = screen.open_template().unwrap();
    assert_eq!(open.source, TOKEN_2022);
    assert_eq!(open.template.name, "Create a Token-2022 token");
    assert!(screen.drafts().is_empty(), "the blocks give way");
    assert_eq!(
        instructions_of(&screen.expansion()),
        ["createAccount", "initializeMint2"],
        "no option is on by default"
    );
    let entries = screen.act(|| template_entries(&screen.session));
    assert_eq!(entries.len(), wallet_core::BUILTIN_TEMPLATES.len());
    assert!(entries.iter().all(|entry| entry.problem.is_none()));
    assert!(entries
        .iter()
        .all(|entry| entry.source.chip() == "built-in"));
}

#[tokio::test]
async fn answers_and_options_change_the_expansion() {
    let screen = Screen::offline().await;
    screen.open(TOKEN_2022);
    screen.answer("decimals", "6");
    screen.turn_on("metadata");
    let expansion = screen.expansion();
    assert_eq!(
        instructions_of(&expansion),
        [
            "createAccount",
            "initializeMetadataPointer",
            "initializeMint2",
            "initializeTokenMetadata"
        ]
    );
    assert_eq!(
        block(&expansion, "initializeMint2").argument("decimals"),
        "6"
    );

    screen.act(|| set_option(&screen.session, "metadata", false));
    assert_eq!(
        instructions_of(&screen.expansion()),
        ["createAccount", "initializeMint2"]
    );
}

#[tokio::test]
async fn defaults_resolve_from_the_selected_wallet_and_can_be_cleared() {
    let screen = Screen::offline().await;
    screen.open(TOKEN_2022);
    let opened = screen.opened();
    assert_eq!(opened.env.wallet, Some(screen.key));
    assert_eq!(opened.env.fee_payer, Some(screen.key));
    assert_eq!(
        opened.open.input_text("authority", &opened.env),
        screen.key.to_string()
    );
    assert_eq!(opened.open.input_text("decimals", &opened.env), "9");
    let mint2 = block(&screen.expansion(), "initializeMint2").clone();
    assert_eq!(mint2.argument("mintAuthority"), screen.key.to_string());

    screen.answer("authority", "");
    let cleared = screen.opened();
    assert_eq!(cleared.open.input_text("authority", &cleared.env), "");
    assert!(cleared
        .expansion
        .unwrap()
        .missing
        .contains(&Missing::Input("authority".into())));
}

#[tokio::test]
async fn a_new_keypair_is_made_once_and_kept_while_the_form_changes() {
    let screen = Screen::offline().await;
    let before = screen.generated().len();
    screen.open(TOKEN_2022);
    let generated = screen.generated();
    assert_eq!(
        generated.len(),
        before + 1,
        "one key for its one `new-keypair`"
    );
    let mint = *generated.last().unwrap();
    assert_eq!(
        screen.open_template().unwrap().keypairs.get("mint"),
        Some(&mint)
    );

    fill_token_2022(&screen);
    // Opening again made a second key; answering makes none.
    let generated = screen.generated();
    let mint = *generated.last().unwrap();
    for _ in 0..3 {
        screen.answer("decimals", "2");
        screen.answer("decimals", "6");
        let expansion = screen.expansion();
        assert_eq!(
            block(&expansion, "createAccount").account("newAccount"),
            Some(mint)
        );
        assert_eq!(
            block(&expansion, "initializeMint2").account("mint"),
            Some(mint)
        );
    }
    assert_eq!(screen.generated(), generated);
    // Sign finds the key where it finds Build's own.
    assert!(screen
        .act(|| screen.session.local_signer_pubkeys())
        .contains(&mint));
}

#[tokio::test]
async fn facts_typed_by_hand_feed_the_expansion() {
    let screen = Screen::offline().await;
    screen.open(TOKEN_2022);
    let expansion = screen.expansion();
    let [Fact::Rent(bytes)] = expansion.needs.as_slice() else {
        panic!("{:?}", expansion.needs);
    };
    let bytes = *bytes;
    assert_eq!(block(&expansion, "createAccount").argument("lamports"), "");

    screen.act(|| type_fact(&screen.session, Fact::Rent(bytes), "nonsense".into()));
    assert_eq!(screen.expansion().needs, [Fact::Rent(bytes)]);

    screen.act(|| type_fact(&screen.session, Fact::Rent(bytes), "1461600".into()));
    let expansion = screen.expansion();
    assert!(expansion.needs.is_empty());
    assert_eq!(expansion.uses, [Fact::Rent(bytes)], "the field stays");
    assert_eq!(
        block(&expansion, "createAccount").argument("lamports"),
        "0.0014616",
        "in SOL, as Build types lamports"
    );
}

#[tokio::test]
async fn online_the_facts_are_read_from_the_node_once_the_answers_settle() {
    let mut screen = Screen::online(FakeRpc::default()).await;
    screen.open(TOKEN_2022);
    let needs = screen.expansion().needs;
    assert_eq!(needs.len(), 1);
    screen.act(|| request_facts(screen.session, needs.clone()));
    assert_eq!(
        screen.act(|| screen.form().fact_read.peek().state.clone()),
        FactReadState::Reading
    );
    // Asked again while it waits: nothing more is asked.
    screen.act(|| request_facts(screen.session, needs.clone()));
    screen.wait_out_the_delay().await;
    let expansion = screen.expansion();
    assert!(expansion.needs.is_empty(), "{:?}", expansion.needs);
    assert_eq!(
        block(&expansion, "createAccount").argument("lamports"),
        "0.00228288"
    );
    assert_eq!(
        screen.act(|| screen.form().fact_read.peek().state.clone()),
        FactReadState::Idle
    );
}

#[tokio::test]
async fn a_reply_for_a_template_closed_meanwhile_is_dropped() {
    let mut screen = Screen::online(FakeRpc::default()).await;
    screen.open(TOKEN_2022);
    let needs = screen.expansion().needs;
    screen.act(|| request_facts(screen.session, needs));
    screen.open(TOKEN_2022);
    screen.wait_out_the_delay().await;
    let open = screen.open_template().unwrap();
    assert_eq!(open.read_facts, Default::default());
}

#[tokio::test]
async fn show_as_blocks_leaves_the_template_with_the_same_drafts() {
    let screen = Screen::offline().await;
    fill_token_2022(&screen);
    screen.type_rent();
    let expansion = screen.expansion();
    screen.act(|| show_as_blocks(&screen.session));

    assert!(screen.open_template().is_none());
    let drafts: Vec<IdlDraft> = screen
        .drafts()
        .iter()
        .map(|draft| draft.idl().unwrap().clone())
        .collect();
    let expected: Vec<IdlDraft> = expansion
        .blocks
        .iter()
        .map(|block| block.draft.clone())
        .collect();
    assert_eq!(drafts, expected);
    let note = screen.act(|| screen.form().compiled_note.peek().clone());
    assert!(note.unwrap().contains("“Create a Token-2022 token”"));
    assert_eq!(
        screen.act(|| *screen.form().selected.peek()),
        screen.drafts().first().map(|draft| draft.id)
    );
}

#[tokio::test]
async fn a_complete_form_compiles_to_the_instructions_its_blocks_build() {
    let mut screen = Screen::offline().await;
    fill_token_2022(&screen);
    screen.type_rent();
    screen.act(|| {
        let mut blockhash = screen.form().blockhash;
        blockhash.set(BLOCKHASH.to_string());
    });
    // Its blocks, built one by one with their IDLs.
    let mut expansion = screen.expansion();
    let by_hand: Vec<Instruction> = screen.act(|| {
        let idls = screen.session.idls.read();
        expansion
            .blocks
            .iter_mut()
            .map(|block| {
                let client = idls
                    .program_client(&block.draft.program_id.unwrap())
                    .unwrap();
                block.draft.build(&client).unwrap()
            })
            .collect()
    });
    let settings = TxSettings {
        blockhash: BLOCKHASH.to_string(),
        ..TxSettings::default()
    };
    let expected = compile_draft(Some(screen.key), by_hand.clone(), &settings).unwrap();

    screen.compile();
    screen.settle().await;
    assert_eq!(screen.error(), None);
    assert_eq!(screen.sign_wire(), Some(expected.wire));

    // Shown as blocks, Build's own button builds the same.
    screen.act(|| show_as_blocks(&screen.session));
    let shown = screen
        .act(|| current_instructions(&screen.session))
        .unwrap();
    assert_eq!(shown, by_hand);
}

#[tokio::test]
async fn a_required_input_left_blank_stops_review_and_is_named() {
    let screen = Screen::offline().await;
    fill_token_2022(&screen);
    screen.type_rent();
    screen.answer("token_name", "");
    screen.answer("max_fee", "");
    screen.compile();
    let error = screen.error().unwrap();
    assert!(error.contains("Name"), "{error}");
    assert!(error.contains("Maximum fee"), "{error}");
    assert_eq!(screen.sign_wire(), None);
    let marked = screen.open_template().unwrap().marked;
    assert_eq!(
        marked,
        ["max_fee", "token_name"]
            .into_iter()
            .map(String::from)
            .collect()
    );
}

#[tokio::test]
async fn clear_closes_the_template_and_keeps_the_keys() {
    let screen = Screen::offline().await;
    fill_token_2022(&screen);
    let generated = screen.generated();
    screen.act(|| screen.session.clear_build());
    assert!(screen.open_template().is_none());
    assert!(screen.drafts().is_empty());
    assert_eq!(screen.generated(), generated);
}

#[tokio::test]
async fn the_sign_note_names_the_template_and_nothing_else_of_it() {
    let mut screen = Screen::offline().await;
    fill_token_2022(&screen);
    screen.type_rent();
    screen.act(|| {
        let mut blockhash = screen.form().blockhash;
        blockhash.set(BLOCKHASH.to_string());
    });
    screen.compile();
    screen.settle().await;
    let note = screen.sign_note().expect("a note");
    assert_eq!(
        note,
        "Compiled on Build from the template “Create a Token-2022 token”."
    );
    let template = screen.open_template().unwrap().template;
    let mut words: Vec<&str> = template.summary.iter().map(String::as_str).collect();
    for input in &template.inputs {
        words.push(&input.label);
        words.extend(input.hint.as_deref());
    }
    for option in &template.options {
        words.push(&option.label);
        words.extend(option.hint.as_deref());
    }
    // Its name is there once; nothing else of it is.
    let rest = note.replacen(&template.name, "", 1);
    for word in words {
        assert!(!rest.contains(word), "{word:?} is in {note:?}");
    }
}
