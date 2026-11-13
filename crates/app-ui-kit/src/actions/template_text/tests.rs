//! Writing a template as text: the text read as it is typed into what the
//! editor shows (each refusal, the problems on this network, what the name
//! means), the ways the editor opens, and saving: kept byte for byte,
//! replacing, and renaming, the new file written before the old one goes.

use super::*;
use crate::actions::kept_templates::load_kept;
use crate::actions::template::tests::Screen;
use crate::actions::template::{expand_open, template_entries, TemplateEntry};
use crate::actions::template_browser::FieldOrigin;
use solana_pubkey::Pubkey;
use wallet_flows::testing::FakeRpc;

const FRIEND: &str = "# Pays a friend.
format = 1
name = \"Pay a friend\"

[programs]
system = \"11111111111111111111111111111111\"   # the System Program

[[input]]
name = \"to\"
label = \"To\"

[[block]]
program    = \"$system\"
instruction = \"transferSol\"
accounts = { source = \"$wallet\", destination = \"$to\" }
arguments = { amount = \"0.25\" }
";

fn read(screen: &Screen, target: &EditTarget, text: &str) -> TextRead {
    screen.act(|| read_text(&screen.session, target, text))
}

fn save(
    screen: &Screen,
    target: &EditTarget,
    text: &str,
    replace: bool,
) -> Result<TextSaved, String> {
    screen.act(|| save_text(&screen.session, target, text, replace))
}

fn saved(result: Result<TextSaved, String>) -> SavedText {
    match result {
        Ok(TextSaved::Saved(saved)) => saved,
        other => panic!("not saved: {other:?}"),
    }
}

fn stored(screen: &Screen, slug: &str) -> Option<String> {
    screen
        .kit
        .storage
        .read(&app_core::storage::template_file(slug))
        .map(|bytes| String::from_utf8(bytes).unwrap())
}

fn yours(screen: &Screen) -> Vec<TemplateEntry> {
    screen
        .act(|| template_entries(&screen.session))
        .into_iter()
        .filter(|entry| entry.source.chip() == "yours")
        .collect()
}

fn editing(slug: &str, name: &str) -> EditTarget {
    EditTarget::Edit {
        slug: slug.into(),
        name: name.into(),
    }
}

/// Keep FRIEND as one of the user's, as an import from the editor.
fn keep_friend(screen: &Screen) {
    let target = EditTarget::Import {
        file: "friend.toml".into(),
    };
    saved(save(screen, &target, FRIEND, false));
}

fn refused(read: TextRead) -> (String, Option<Excerpt>) {
    match read {
        TextRead::Refused(Refusal::NotATemplate { reason, excerpt }) => (reason, excerpt),
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[tokio::test]
async fn a_text_larger_than_a_template_is_refused_unread() {
    let screen = Screen::offline().await;
    let text = format!("{FRIEND}#{}", "x".repeat(MAX_TEMPLATE_BYTES));
    assert_eq!(
        read(&screen, &EditTarget::New, &text),
        TextRead::Refused(Refusal::TooLarge { bytes: text.len() })
    );
    assert!(!read(&screen, &EditTarget::New, &text).can_save());
    assert_eq!(
        save(&screen, &EditTarget::New, &text, false),
        Err(too_large(text.len()))
    );
    // A file that large opens nothing either.
    let refused = import_text("big.toml", text.into_bytes()).unwrap_err();
    assert!(refused.starts_with("big.toml is "), "{refused}");
    assert!(yours(&screen).is_empty());
}

#[tokio::test]
async fn a_toml_error_names_its_line_and_shows_it() {
    let screen = Screen::offline().await;
    let text = FRIEND.replace("label = \"To\"", "label = \"To");
    let read = read(&screen, &EditTarget::New, &text);
    assert!(!read.can_save());
    assert_eq!(read.view(), None);
    let (reason, excerpt) = refused(read);
    let excerpt = excerpt.expect("TOML says where");
    assert_eq!(excerpt.line, 10);
    assert_eq!(
        format!("{}{}{}", excerpt.before, excerpt.at, excerpt.after),
        "label = \"To"
    );
    assert!(reason.ends_with('.'), "{reason}");
    assert!(reason.chars().next().unwrap().is_uppercase(), "{reason}");
    // The words saving refuses with name the line too.
    let why = save(&screen, &EditTarget::New, &text, false).unwrap_err();
    assert!(why.contains("line 10"), "{why}");
}

#[tokio::test]
async fn a_hidden_character_is_shown_by_its_code_where_it_is() {
    let screen = Screen::offline().await;
    let text = FRIEND.replace("# Pays a friend.", "# Pays a \u{202E}friend.");
    let (reason, excerpt) = refused(read(&screen, &EditTarget::New, &text));
    assert!(reason.starts_with("line 1: holds U+202E"), "{reason}");
    let excerpt = excerpt.unwrap();
    assert_eq!(excerpt.line, 1);
    assert_eq!(excerpt.before, "# Pays a ");
    assert_eq!(excerpt.at, "⟨U+202E⟩");
    assert_eq!(excerpt.after, "friend.");
}

#[tokio::test]
async fn a_format_error_names_its_key() {
    let screen = Screen::offline().await;
    let text = FRIEND.replace("destination = \"$to\"", "destination = \"$too\"");
    let (reason, excerpt) = refused(read(&screen, &EditTarget::New, &text));
    assert!(
        reason.starts_with("block[1].accounts.destination: "),
        "{reason}"
    );
    assert_eq!(excerpt, None);
}

#[tokio::test]
async fn problems_on_this_network_are_warnings_that_do_not_stop_a_save() {
    let screen = Screen::offline().await;
    let text = FRIEND.replace(
        "11111111111111111111111111111111",
        &Pubkey::new_from_array([9; 32]).to_string(),
    );
    let read = read(&screen, &EditTarget::New, &text);
    assert!(read.can_save());
    assert_eq!(read.replaces(), None);
    let view = read.view().unwrap();
    assert_eq!(view.problems.len(), 1, "{:?}", view.problems);
    assert!(
        view.problems[0].contains("Airlock has no description of"),
        "{:?}",
        view.problems
    );
    // Its blocks still read, so what it writes is in sight.
    assert_eq!(view.blocks[0].instruction, "transferSol");

    let saved = saved(save(&screen, &EditTarget::New, &text, false));
    assert_eq!(saved.key, "yours:pay-a-friend");
    assert_eq!(
        saved.note,
        "Saved “Pay a friend”. It is listed with your templates. It cannot be used on this network, so it is listed greyed."
    );
    let mine = yours(&screen);
    assert_eq!(mine.len(), 1);
    assert!(mine[0].problem.is_some(), "listed greyed");
}

#[tokio::test]
async fn a_shipped_templates_name_is_refused_and_a_copy_says_so_until_it_changes() {
    let screen = Screen::offline().await;
    let shipped = TemplateSource::Builtin("create-token");
    assert_eq!(edit_kept(&shipped), None, "a shipped one is not edited");
    let copy = edit_copy(&shipped).unwrap();
    assert_eq!(copy.target.title(), "A copy of “Create a token”");
    assert_eq!(copy.text, shipped.text().unwrap());
    assert!(!copy.dirty());

    let refusal = "“Create a token” is a built-in template. Give yours another name.";
    let as_copied = read(&screen, &copy.target, &copy.text);
    assert!(!as_copied.can_save());
    match &as_copied {
        TextRead::Reads { name, view } => {
            assert_eq!(*name, NameCheck::Builtin(refusal.to_string()));
            assert!(view.problems.is_empty());
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(
        save(&screen, &copy.target, &copy.text, true),
        Err(refusal.to_string())
    );
    // A shipped one's id is refused as its name is.
    let by_id = copy
        .text
        .replace("name = \"Create a token\"", "name = \"mint-tokens\"");
    assert!(!read(&screen, &copy.target, &by_id).can_save());

    // Changed, it can be kept.
    let mine = copy
        .text
        .replace("name = \"Create a token\"", "name = \"My token\"");
    assert!(read(&screen, &copy.target, &mine).can_save());
    let saved = saved(save(&screen, &copy.target, &mine, false));
    assert_eq!(saved.key, "yours:my-token");
    assert_eq!(stored(&screen, "my-token").as_deref(), Some(mine.as_str()));
}

#[tokio::test]
async fn another_of_yours_under_the_same_name_is_replaced_only_when_asked() {
    let screen = Screen::offline().await;
    keep_friend(&screen);
    let text = FRIEND.replace("0.25", "0.5");
    // Another spelling of the same slug is the same template.
    let other = text.replace("name = \"Pay a friend\"", "name = \"pay A friend\"");
    let read = read(&screen, &EditTarget::New, &other);
    assert!(read.can_save());
    assert_eq!(read.replaces(), Some("Pay a friend"));

    assert_eq!(
        save(&screen, &EditTarget::New, &other, false),
        Ok(TextSaved::WouldReplace("Pay a friend".into()))
    );
    assert_eq!(stored(&screen, "pay-a-friend").as_deref(), Some(FRIEND));
    let saved = saved(save(&screen, &EditTarget::New, &other, true));
    assert_eq!(
        saved.note,
        "Saved “pay A friend” in place of the one you had."
    );
    assert_eq!(
        stored(&screen, "pay-a-friend").as_deref(),
        Some(other.as_str())
    );
    assert_eq!(yours(&screen).len(), 1);
}

#[tokio::test]
async fn the_template_being_edited_keeps_its_own_name() {
    let screen = Screen::offline().await;
    keep_friend(&screen);
    let mine = yours(&screen).remove(0);
    let edit = edit_kept(&mine.source).unwrap();
    assert_eq!(edit.target, editing("pay-a-friend", "Pay a friend"));
    assert_eq!(edit.target.title(), "Editing “Pay a friend”");
    assert_eq!(edit.text, FRIEND);

    let changed = FRIEND.replace("0.25", "0.3");
    for text in [
        changed.clone(),
        changed.replace("name = \"Pay a friend\"", "name = \"PAY a friend\""),
    ] {
        match read(&screen, &edit.target, &text) {
            TextRead::Reads { name, .. } => assert_eq!(name, NameCheck::Own),
            other => panic!("{other:?}"),
        }
    }
    let saved = saved(save(&screen, &edit.target, &changed, false));
    assert_eq!(saved.note, "Saved your changes to “Pay a friend”.");
    assert_eq!(saved.key, "yours:pay-a-friend");
    assert_eq!(
        stored(&screen, "pay-a-friend").as_deref(),
        Some(changed.as_str())
    );
}

#[tokio::test]
async fn a_commented_text_is_kept_and_read_back_byte_for_byte() {
    let screen = Screen::offline().await;
    // Comments, an order of keys and spacing to_toml would not write, and
    // a line ending a field never gives.
    let text = format!("{FRIEND}\n\n# The end.   \r\n");
    let saved = saved(save(&screen, &EditTarget::New, &text, false));
    assert_eq!(
        stored(&screen, "pay-a-friend").as_deref(),
        Some(text.as_str())
    );
    for reload in [false, true] {
        if reload {
            screen.act(|| load_kept(&screen.session));
        }
        let mine = yours(&screen).remove(0);
        assert_eq!(mine.source.key(), saved.key);
        assert_eq!(mine.source.text(), Some(text.as_str()));
        // Edit opens on exactly what is kept.
        assert_eq!(edit_kept(&mine.source).unwrap().text, text);
        assert_eq!(edit_copy(&mine.source).unwrap().text, text);
    }
}

#[tokio::test]
async fn a_new_name_renames_writing_the_new_file_before_the_old_goes() {
    let screen = Screen::offline().await;
    keep_friend(&screen);
    let edit = edit_kept(&yours(&screen)[0].source).unwrap();
    let renamed = FRIEND.replace("name = \"Pay a friend\"", "name = \"Pay a pal\"");
    match read(&screen, &edit.target, &renamed) {
        TextRead::Reads { name, .. } => assert_eq!(name, NameCheck::Free),
        other => panic!("{other:?}"),
    }

    // The new file cannot be written: the old one stays, kept and listed,
    // and nothing is written in its place.
    screen.kit.storage.refuse_saves_alone(true);
    let why = save(&screen, &edit.target, &renamed, false).unwrap_err();
    assert!(why.starts_with("The template was not saved"), "{why}");
    assert_eq!(stored(&screen, "pay-a-friend").as_deref(), Some(FRIEND));
    assert_eq!(stored(&screen, "pay-a-pal"), None);
    let names: Vec<String> = yours(&screen).into_iter().map(|e| e.name).collect();
    assert_eq!(names, ["Pay a friend"]);

    // Written, the old one goes.
    screen.kit.storage.refuse_saves_alone(false);
    let saved = saved(save(&screen, &edit.target, &renamed, false));
    assert_eq!(saved.key, "yours:pay-a-pal");
    assert_eq!(saved.note, "Saved “Pay a pal”, which was “Pay a friend”.");
    assert_eq!(
        stored(&screen, "pay-a-pal").as_deref(),
        Some(renamed.as_str())
    );
    assert_eq!(stored(&screen, "pay-a-friend"), None);
    let names: Vec<String> = yours(&screen).into_iter().map(|e| e.name).collect();
    assert_eq!(names, ["Pay a pal"]);
}

#[tokio::test]
async fn a_rename_whose_old_file_stays_says_so() {
    let screen = Screen::offline().await;
    keep_friend(&screen);
    let edit = edit_kept(&yours(&screen)[0].source).unwrap();
    let renamed = FRIEND.replace("name = \"Pay a friend\"", "name = \"Pay a pal\"");
    screen.kit.storage.refuse_removals_alone(true);
    let saved = saved(save(&screen, &edit.target, &renamed, false));
    assert!(
        saved
            .note
            .starts_with("Saved “Pay a pal”, which was “Pay a friend”. Its old file, pay-a-friend.toml, is still kept."),
        "{}",
        saved.note
    );
    assert_eq!(
        stored(&screen, "pay-a-pal").as_deref(),
        Some(renamed.as_str())
    );
    assert_eq!(stored(&screen, "pay-a-friend").as_deref(), Some(FRIEND));
    assert_eq!(yours(&screen).len(), 2);
}

#[tokio::test]
async fn a_rename_onto_another_of_yours_replaces_it() {
    let screen = Screen::offline().await;
    keep_friend(&screen);
    let pal = FRIEND.replace("name = \"Pay a friend\"", "name = \"Pay a pal\"");
    saved(save(&screen, &EditTarget::New, &pal, false));
    let friend = yours(&screen)
        .into_iter()
        .find(|entry| entry.name == "Pay a friend")
        .unwrap();
    let edit = edit_kept(&friend.source).unwrap();
    let onto = FRIEND
        .replace("name = \"Pay a friend\"", "name = \"Pay a pal\"")
        .replace("0.25", "1");
    let read = read(&screen, &edit.target, &onto);
    assert_eq!(read.replaces(), Some("Pay a pal"));
    assert_eq!(
        save(&screen, &edit.target, &onto, false),
        Ok(TextSaved::WouldReplace("Pay a pal".into()))
    );
    let saved = saved(save(&screen, &edit.target, &onto, true));
    assert_eq!(
        saved.note,
        "Saved “Pay a pal”, which was “Pay a friend”, in place of the one you had."
    );
    assert_eq!(stored(&screen, "pay-a-pal").as_deref(), Some(onto.as_str()));
    assert_eq!(stored(&screen, "pay-a-friend"), None);
    assert_eq!(yours(&screen).len(), 1);
}

#[tokio::test]
async fn a_broken_file_opens_to_be_mended() {
    let screen = Screen::offline().await;
    let half = "format = 1\nname = \"Half\"\n# no blocks yet\nsurprise = 1\n";
    let storage = &screen.kit.storage;
    storage.put(&app_core::storage::template_file("half"), half.as_bytes());
    storage.put(&app_core::storage::template_file("bytes"), &[0xff, 0xfe]);
    screen.act(|| load_kept(&screen.session));
    let broken: Vec<TemplateEntry> = yours(&screen);
    assert_eq!(broken.len(), 2);
    assert_eq!(
        edit_kept(&broken[0].source),
        None,
        "not text: nothing to mend"
    );
    assert_eq!(edit_copy(&broken[1].source), None);
    let edit = edit_kept(&broken[1].source).unwrap();
    assert_eq!(edit.target, editing("half", "half.toml"));
    assert_eq!(edit.text, half);
    let (reason, excerpt) = refused(read(&screen, &edit.target, &edit.text));
    assert!(reason.contains("surprise"), "{reason}");
    assert_eq!(excerpt.unwrap().line, 4);

    // Mended under the same name, it is written over its file.
    let mended = FRIEND.replace("name = \"Pay a friend\"", "name = \"Half\"");
    let saved = saved(save(&screen, &edit.target, &mended, false));
    assert_eq!(saved.key, "yours:half");
    assert_eq!(stored(&screen, "half").as_deref(), Some(mended.as_str()));
    let mine = yours(&screen);
    assert!(mine.iter().all(|entry| entry.name != "half.toml"));
    assert!(mine
        .iter()
        .any(|entry| entry.name == "Half" && !entry.is_broken()));
}

#[tokio::test]
async fn the_starter_reads_saves_and_opens_on_build() {
    let mut screen = Screen::offline().await;
    let starter = new_template();
    assert_eq!(starter.target, EditTarget::New);
    assert_eq!(starter.target.title(), "New template");
    assert_eq!(starter.text, STARTER_TEMPLATE);
    let read = read(&screen, &starter.target, &starter.text);
    let view = read.view().unwrap().clone();
    assert!(view.problems.is_empty(), "{:?}", view.problems);
    assert!(read.can_save());
    assert_eq!(
        reads_as(&view),
        "Reads as a template: 2 questions, 1 block."
    );
    assert_eq!(view.questions.always[1].until.as_deref(), Some("0.01"));
    assert_eq!(
        view.questions.always[1].takes.as_deref(),
        Some("a number in SOL")
    );

    let saved = saved(save(&screen, &starter.target, &starter.text, false));
    assert_eq!(saved.key, "yours:send-sol");
    let mine = yours(&screen).remove(0);
    screen.open(mine.source.clone());
    screen.answer("to", &Pubkey::new_from_array([4; 32]).to_string());
    screen.settle().await;
    let expansion = screen.expansion();
    assert!(expansion.missing.is_empty(), "{:?}", expansion.missing);
    assert_eq!(expansion.blocks.len(), 1);
    assert_eq!(expansion.blocks[0].draft.argument("amount"), "0.01");
}

#[tokio::test]
async fn a_change_in_line_endings_alone_is_no_change() {
    let mut edit = new_template();
    edit.text = edit.text.replace('\n', "\r\n");
    let lf = TextEdit {
        original: edit.text.as_str().into(),
        text: edit.text.replace("\r\n", "\n"),
        ..edit.clone()
    };
    assert!(!lf.dirty());
    let mut changed = lf;
    changed.text.push('x');
    assert!(changed.dirty());
    // Each opening is told from the one before.
    assert_ne!(new_template().opened, new_template().opened);
}

#[tokio::test]
async fn a_chain_fact_reads_from_the_node_only_where_there_is_one() {
    let online = Screen::online(FakeRpc::default()).await;
    let offline = Screen::offline().await;
    let mint = TemplateSource::Builtin("mint-tokens");
    for (screen, words) in [
        (&online, "the decimals of the mint “Mint”, from the node"),
        (
            &offline,
            "the decimals of the mint “Mint”, typed in as the chain has it",
        ),
    ] {
        let read = read(
            screen,
            &EditTarget::Copy {
                name: "Mint tokens".into(),
            },
            mint.text().unwrap(),
        );
        let decimals = read.view().unwrap().blocks[1]
            .fields()
            .find(|row| row.field == "decimals")
            .unwrap()
            .origin
            .clone();
        assert_eq!(
            decimals,
            FieldOrigin::Worked {
                value: Some("decimals".into()),
                words: words.into()
            }
        );
    }
}

/// The form open on Build keeps working on the template it opened while
/// its file is edited, renamed or removed in the dialog: it shows and
/// builds what it opened, never one thing and then another.
#[tokio::test]
async fn a_template_open_on_build_keeps_what_it_opened_while_its_file_changes() {
    let mut screen = Screen::offline().await;
    keep_friend(&screen);
    let mine = yours(&screen).remove(0);
    screen.open(mine.source.clone());
    let to = Pubkey::new_from_array([4; 32]).to_string();
    screen.answer("to", &to);
    screen.settle().await;
    let before = screen.expansion();
    let opened = |screen: &Screen| {
        screen
            .act(|| expand_open(&screen.session))
            .map(|open| (open.open.template.name.clone(), open.open.source.clone()))
    };
    let at_first = opened(&screen);

    let edit = edit_kept(&mine.source).unwrap();
    // Edited: a different amount.
    saved(save(
        &screen,
        &edit.target,
        &FRIEND.replace("0.25", "9"),
        false,
    ));
    // Renamed.
    let renamed = FRIEND.replace("name = \"Pay a friend\"", "name = \"Pay a pal\"");
    saved(save(&screen, &edit.target, &renamed, false));
    // Removed.
    screen
        .act(|| crate::actions::kept_templates::remove_kept(&screen.session, "pay-a-pal"))
        .unwrap();
    screen.settle().await;

    assert_eq!(opened(&screen), at_first);
    assert_eq!(screen.expansion(), before);
    assert_eq!(before.blocks[0].draft.argument("amount"), "0.25");
}
