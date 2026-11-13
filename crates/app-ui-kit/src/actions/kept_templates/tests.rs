//! The user's templates through the real form and actions, on a window with
//! no screen drawn and the core's memory store: saving Build's blocks,
//! opening what was saved, replacing, removing, importing and exporting.

use super::*;
use crate::actions::template::tests::Screen;
use crate::actions::template::{expand_open, open_template, template_entries};
use crate::actions::template_text::{
    read_text, save_text, EditTarget, Refusal, TextRead, TextSaved,
};
use solana_pubkey::Pubkey;
use wallet_core::edit::RawDraft;
use wallet_core::{IdlDraft, SYSTEM_PROGRAM_ID};

fn pk(n: u8) -> Pubkey {
    Pubkey::new_from_array([n; 32])
}

/// A System transfer from the wallet, as Build makes one.
fn transfer(screen: &Screen, to: Pubkey, amount: &str) -> IdlDraft {
    screen.act(|| {
        let idls = screen.session.idls.read();
        let client = idls.program_client(&SYSTEM_PROGRAM_ID).unwrap();
        let mut draft =
            IdlDraft::for_instruction(SYSTEM_PROGRAM_ID, "system", &client, Some("transferSol"));
        draft.set_account("source", Some(screen.key));
        draft.set_account("destination", Some(to));
        draft.set_argument("amount", amount.to_string());
        draft
    })
}

fn put_blocks(screen: &Screen, drafts: &[IdlDraft]) {
    screen.act(|| {
        for draft in drafts {
            screen
                .form()
                .add_instruction(None, IxBody::Idl(draft.clone()));
        }
    });
}

/// The destination and the amount of the first block asked each time.
fn asked() -> BTreeMap<FieldAt, Choice> {
    BTreeMap::from([
        (FieldAt::new(0, "destination"), Choice::Ask),
        (FieldAt::new(0, "amount"), Choice::Ask),
    ])
}

fn save(screen: &Screen, name: &str, replace: bool) -> Result<Kept, Vec<String>> {
    screen.act(|| {
        let plan = save_plan(&screen.session).unwrap();
        save_as_template(&screen.session, &plan, name, "", &asked(), replace)
    })
}

fn entries(screen: &Screen) -> Vec<crate::actions::template::TemplateEntry> {
    screen.act(|| template_entries(&screen.session))
}

fn yours(screen: &Screen) -> Vec<crate::actions::template::TemplateEntry> {
    entries(screen)
        .into_iter()
        .filter(|entry| entry.source.chip() == "yours")
        .collect()
}

fn stored(screen: &Screen, slug: &str) -> Option<String> {
    screen
        .kit
        .storage
        .read(&app_core::storage::template_file(slug))
        .map(|bytes| String::from_utf8(bytes).unwrap())
}

const GOOD: &str = r#"
format = 1
name = "Pay a friend"

[programs]
system = "11111111111111111111111111111111"

[[input]]
name = "to"
label = "To"

[[block]]
program = "$system"
instruction = "transferSol"
accounts = { source = "$wallet", destination = "$to" }
arguments = { amount = "0.25" }
"#;

#[tokio::test]
async fn saved_blocks_open_as_a_template_that_writes_them_back() {
    let screen = Screen::offline().await;
    let drafts = [
        transfer(&screen, pk(5), "1.5"),
        transfer(&screen, pk(6), "2"),
    ];
    put_blocks(&screen, &drafts);

    assert_eq!(
        save(&screen, "Pay rent", false),
        Ok(Kept::Saved("Pay rent".into()))
    );
    // Kept as its file, listed after the built-in ones, and Build says so
    // with its blocks left as they were.
    let text = stored(&screen, "pay-rent").expect("kept as templates/pay-rent.toml");
    let template = Template::parse(&text).unwrap();
    assert_eq!(template.name, "Pay rent");
    let entries = entries(&screen);
    assert_eq!(entries.len(), wallet_core::BUILTIN_TEMPLATES.len() + 1);
    let mine = entries.last().unwrap();
    assert_eq!(mine.source.chip(), "yours");
    assert_eq!(mine.problem, None);
    assert_eq!(screen.drafts().len(), 2);
    let note = screen.act(|| screen.form().compiled_note.peek().clone());
    assert_eq!(
        note.as_deref(),
        Some("Saved as the template “Pay rent”. Open it from Templates.")
    );

    // Opened and answered as the blocks were, it writes them back.
    screen.open(mine.source.clone());
    assert!(screen.drafts().is_empty());
    screen.answer("destination", &pk(5).to_string());
    screen.answer("amount", "1.5");
    let back: Vec<IdlDraft> = screen
        .expansion()
        .blocks
        .into_iter()
        .map(|block| block.draft)
        .collect();
    assert_eq!(back, drafts);
    let open = screen.act(|| expand_open(&screen.session)).unwrap();
    assert_eq!(open.form.inputs.len(), 2);
}

#[tokio::test]
async fn a_name_already_kept_asks_before_it_is_replaced() {
    let screen = Screen::offline().await;
    put_blocks(&screen, &[transfer(&screen, pk(5), "1")]);
    save(&screen, "Pay rent", false).unwrap();
    let first = stored(&screen, "pay-rent").unwrap();

    screen.act(|| screen.session.clear_build());
    put_blocks(&screen, &[transfer(&screen, pk(7), "3")]);
    // Another spelling of the same slug is the same template.
    assert_eq!(
        save(&screen, "pay  RENT", false),
        Ok(Kept::WouldReplace("Pay rent".into()))
    );
    assert_eq!(
        stored(&screen, "pay-rent").unwrap(),
        first,
        "nothing written"
    );
    assert_eq!(
        save(&screen, "pay  RENT", true),
        Ok(Kept::Saved("pay  RENT".into()))
    );
    assert_ne!(stored(&screen, "pay-rent").unwrap(), first);
    let mine = yours(&screen);
    assert_eq!(mine.len(), 1);
    assert_eq!(mine[0].name, "pay  RENT");
}

#[tokio::test]
async fn a_built_in_templates_name_is_refused() {
    let screen = Screen::offline().await;
    put_blocks(&screen, &[transfer(&screen, pk(5), "1")]);
    for name in ["Create a token", "mint-tokens"] {
        let refused = save(&screen, name, true).unwrap_err();
        assert!(refused[0].contains("is a built-in template"), "{refused:?}");
    }
    assert!(yours(&screen).is_empty());
}

#[tokio::test]
async fn a_save_that_fails_is_reported_and_nothing_is_listed() {
    let screen = Screen::offline().await;
    put_blocks(&screen, &[transfer(&screen, pk(5), "1")]);
    screen.kit.storage.refuse_saves(true);
    let refused = save(&screen, "Pay rent", false).unwrap_err();
    assert!(
        refused[0].starts_with("The template was not saved"),
        "{refused:?}"
    );
    assert!(yours(&screen).is_empty());
    assert_eq!(stored(&screen, "pay-rent"), None);
    let note = screen.act(|| screen.form().compiled_note.peek().clone());
    assert_eq!(note, None);
}

#[tokio::test]
async fn a_block_written_by_hand_cannot_be_saved() {
    let screen = Screen::offline().await;
    put_blocks(&screen, &[transfer(&screen, pk(5), "1")]);
    screen.act(|| {
        screen
            .form()
            .add_instruction(None, IxBody::Raw(RawDraft::default()))
    });
    let refused = screen.act(|| save_plan(&screen.session)).unwrap_err();
    assert_eq!(
        refused,
        "Block 2 is written by hand, and a template holds only instructions filled from an IDL."
    );
}

#[tokio::test]
async fn a_template_is_removed_from_the_store_and_the_list() {
    let screen = Screen::offline().await;
    put_blocks(&screen, &[transfer(&screen, pk(5), "1")]);
    save(&screen, "Pay rent", false).unwrap();
    screen
        .act(|| remove_kept(&screen.session, "pay-rent"))
        .unwrap();
    assert_eq!(stored(&screen, "pay-rent"), None);
    assert!(yours(&screen).is_empty());
    // Read again from the store, it is still gone.
    screen.act(|| load_kept(&screen.session));
    assert!(yours(&screen).is_empty());
}

#[tokio::test]
async fn a_broken_file_is_listed_with_why_and_can_be_removed() {
    let screen = Screen::offline().await;
    let storage = &screen.kit.storage;
    storage.put(&app_core::storage::template_file("bad"), b"format = 2");
    storage.put(&app_core::storage::template_file("good"), GOOD.as_bytes());
    screen.act(|| ensure_kept(&screen.session));
    let mine = yours(&screen);
    assert_eq!(mine.len(), 2, "{mine:#?}");
    assert!(mine[0].is_broken());
    assert_eq!(mine[0].name, "bad.toml");
    assert!(mine[0].problem.as_deref().unwrap().contains("format 2"));
    assert!(!mine[1].is_broken());
    assert_eq!(mine[1].name, "Pay a friend");
    assert_eq!(mine[1].problem, None);

    screen.act(|| remove_kept(&screen.session, "bad")).unwrap();
    let mine = yours(&screen);
    assert_eq!(mine.len(), 1);
    assert_eq!(mine[0].name, "Pay a friend");
    assert_eq!(stored(&screen, "bad"), None);
}

fn importing() -> EditTarget {
    EditTarget::Import {
        file: "pay-a-friend.toml".into(),
    }
}

/// Read `text` as the editor reads an imported file.
fn read(screen: &Screen, text: &str) -> TextRead {
    screen.act(|| read_text(&screen.session, &importing(), text))
}

fn keep_text(screen: &Screen, text: &str, replace: bool) -> Result<TextSaved, String> {
    screen.act(|| save_text(&screen.session, &importing(), text, replace))
}

#[tokio::test]
async fn an_imported_file_is_shown_before_it_is_kept() {
    let screen = Screen::offline().await;
    let read = read(&screen, GOOD);
    let template = read.view().unwrap().template.clone();
    assert_eq!(template.name, "Pay a friend");
    assert!(read.view().unwrap().problems.is_empty());
    assert!(yours(&screen).is_empty(), "nothing kept before it is asked");

    let kept = keep_text(&screen, GOOD, false);
    assert!(matches!(kept, Ok(TextSaved::Saved(_))), "{kept:?}");
    // Kept as the file was written, and read back to what was shown.
    assert_eq!(stored(&screen, "pay-a-friend").unwrap(), GOOD);
    assert_eq!(
        Template::parse(&stored(&screen, "pay-a-friend").unwrap()).unwrap(),
        *template
    );
    // The same again asks first.
    let again = keep_text(&screen, GOOD, false);
    assert_eq!(again, Ok(TextSaved::WouldReplace("Pay a friend".into())));
}

/// The click that asks for a file returns before the file is picked, and
/// the editor opens from the task that waits for it. A dialog waited on
/// inside the click let the window render under the handler on desktop,
/// which crashed the app on a second import.
#[tokio::test]
async fn a_file_is_picked_after_the_click_that_asked_for_it_returns() {
    let mut screen = Screen::offline().await;
    let (editing, error, busy) =
        screen.act(|| (Signal::new(None), Signal::new(None), Signal::new(false)));
    let (pick, picked) = tokio::sync::oneshot::channel();
    screen.act(|| import_from(editing, error, busy, async move { picked.await.ok() }));
    screen.settle().await;
    assert_eq!(
        screen.act(|| editing.peek().clone()),
        None,
        "nothing is read while the dialog is open"
    );
    assert!(screen.act(|| *busy.peek()), "held while the dialog is open");
    // Asking again while one is open opens nothing.
    let (_, never) = tokio::sync::oneshot::channel();
    screen.act(|| import_from(editing, error, busy, async move { never.await.ok() }));

    pick.send(Ok(("pay.toml".to_string(), GOOD.as_bytes().to_vec())))
        .unwrap();
    screen.settle().await;
    let opened = screen
        .act(|| editing.peek().clone())
        .expect("open once picked");
    assert_eq!(opened.text, GOOD);
    assert_eq!(opened.target.title(), "Importing pay.toml");
    assert!(!opened.dirty());
    assert_eq!(screen.act(|| error.peek().clone()), None::<String>);
    assert!(!screen.act(|| *busy.peek()), "let go once it returns");
    assert!(yours(&screen).is_empty(), "nothing kept yet");

    // A file that is not text opens nothing, and says why.
    let (pick, picked) = tokio::sync::oneshot::channel();
    screen.act(|| {
        let mut editing = editing;
        editing.set(None);
        import_from(editing, error, busy, async move { picked.await.ok() })
    });
    pick.send(Ok(("bytes.toml".to_string(), vec![0xff, 0xfe])))
        .unwrap();
    screen.settle().await;
    assert_eq!(screen.act(|| editing.peek().clone()), None);
    assert_eq!(
        screen.act(|| error.peek().clone()).as_deref(),
        Some("bytes.toml is not text, so it is not a template.")
    );
}

#[tokio::test]
async fn a_file_that_does_not_parse_is_not_a_template() {
    let screen = Screen::offline().await;
    match read(&screen, "format = 1\nname = 3") {
        TextRead::Refused(Refusal::NotATemplate { reason, excerpt }) => {
            assert!(reason.contains("string"), "{reason}");
            assert_eq!(excerpt.unwrap().line, 2);
        }
        other => panic!("{other:?}"),
    }
    assert!(keep_text(&screen, "format = 1\nname = 3", false)
        .unwrap_err()
        .starts_with("This is not a template file"));
    assert!(yours(&screen).is_empty());
}

#[tokio::test]
async fn a_file_that_does_not_fit_this_network_can_still_be_kept() {
    let screen = Screen::offline().await;
    let text = GOOD
        .replace("11111111111111111111111111111111", &pk(9).to_string())
        .replace("Pay a friend", "Elsewhere");
    let read = read(&screen, &text);
    assert!(read.can_save());
    let problems = &read.view().unwrap().problems;
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(
        problems[0].contains("Airlock has no description of"),
        "{problems:?}"
    );
    match keep_text(&screen, &text, false).unwrap() {
        TextSaved::Saved(saved) => assert!(saved.note.contains("listed greyed"), "{saved:?}"),
        other => panic!("{other:?}"),
    }
    let mine = yours(&screen);
    assert_eq!(mine.len(), 1);
    assert!(mine[0]
        .problem
        .as_deref()
        .unwrap()
        .contains("Airlock has no description of there"));
    assert!(
        !mine[0].is_broken(),
        "listed disabled, as a built-in would be"
    );
}

#[tokio::test]
async fn an_exported_file_imports_to_the_same_template() {
    let screen = Screen::offline().await;
    for builtin in &wallet_core::BUILTIN_TEMPLATES {
        let (name, text) = export_file(&TemplateSource::Builtin(builtin.id)).unwrap();
        assert_eq!(name, format!("{}.toml", builtin.id));
        let read = read(&screen, &text);
        assert_eq!(*read.view().unwrap().template, builtin.template().unwrap());
    }

    put_blocks(&screen, &[transfer(&screen, pk(5), "1")]);
    save(&screen, "Pay rent", false).unwrap();
    let mine = yours(&screen).remove(0);
    let (name, text) = export_file(&mine.source).unwrap();
    assert_eq!(name, "pay-rent.toml");
    let read = read(&screen, &text);
    assert_eq!(
        Ok(read.view().unwrap().template.as_ref().clone()),
        mine.source.template()
    );
}

#[tokio::test]
async fn a_template_of_yours_opens_with_its_keypairs_made_anew() {
    let screen = Screen::offline().await;
    let key = crate::state::forms::GeneratedKey::new();
    let made = key.pubkey;
    screen.act(|| {
        let mut generated = screen.form().generated;
        generated.write().push(key);
    });
    let mut create = screen.act(|| {
        let idls = screen.session.idls.read();
        let client = idls.program_client(&SYSTEM_PROGRAM_ID).unwrap();
        IdlDraft::for_instruction(SYSTEM_PROGRAM_ID, "system", &client, Some("createAccount"))
    });
    create.set_account("payer", Some(screen.key));
    create.set_account("newAccount", Some(made));
    create.set_argument("lamports", "0.001".into());
    create.set_argument("space", "0".into());
    create.set_argument("programAddress", SYSTEM_PROGRAM_ID.to_string());
    put_blocks(&screen, &[create]);
    screen
        .act(|| {
            let plan = save_plan(&screen.session).unwrap();
            save_as_template(
                &screen.session,
                &plan,
                "New account",
                "",
                &BTreeMap::new(),
                false,
            )
        })
        .unwrap();
    let mine = yours(&screen).remove(0);
    screen.act(|| open_template(&screen.session, mine.source.clone()).unwrap());
    let fresh = *screen.generated().last().unwrap();
    assert_ne!(fresh, made, "a new key each time");
    let expansion = screen.expansion();
    assert_eq!(expansion.blocks[0].draft.account("newAccount"), Some(fresh));
}
