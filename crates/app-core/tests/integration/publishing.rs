//! A slice goes out when its value changed, and only then.

use app_core::testing::kit;

#[tokio::test]
async fn a_change_of_reach_leaves_the_book_where_it_was_and_the_same_one_says_nothing() {
    use std::sync::Arc;

    let kit = kit();
    let public = kit.core.public();
    public.set_reach(app_core::Reach::Probing);
    let book = kit.core.slices().book.get();
    let settings = kit.core.slices().settings.get();
    let mut network = kit.core.slices().network.subscribe();

    public.set_reach(app_core::Reach::Answered);
    assert!(network.has_changed().unwrap());
    assert_eq!(network.borrow_and_update().reach, app_core::Reach::Answered);
    assert!(Arc::ptr_eq(&book, &kit.core.slices().book.get()));
    assert!(Arc::ptr_eq(&settings, &kit.core.slices().settings.get()));

    public.set_reach(app_core::Reach::Answered);
    assert!(!network.has_changed().unwrap());
}

#[tokio::test]
async fn a_write_that_changes_nothing_wakes_nobody() {
    use std::sync::Arc;

    let kit = kit();
    let public = kit.core.public();
    let (book, holdings) = (
        kit.core.slices().book.get(),
        kit.core.slices().holdings.get(),
    );
    let book_reader = kit.core.slices().book.subscribe();
    let holdings_reader = kit.core.slices().holdings.subscribe();

    public.workspace_deferred("nothing", |_| {});
    public.holdings_deferred("nothing", |_| {});

    assert!(!book_reader.has_changed().unwrap());
    assert!(!holdings_reader.has_changed().unwrap());
    assert!(Arc::ptr_eq(&book, &kit.core.slices().book.get()));
    assert!(Arc::ptr_eq(&holdings, &kit.core.slices().holdings.get()));

    // One that does change something goes out.
    public.workspace_deferred("something", |workspace| {
        workspace.close_to_tray = !workspace.close_to_tray;
    });
    let settings = kit.core.slices().settings.subscribe();
    public.workspace_deferred("again", |workspace| {
        workspace.close_to_tray = !workspace.close_to_tray;
    });
    assert!(settings.has_changed().unwrap());
    assert!(!book_reader.has_changed().unwrap());
}
