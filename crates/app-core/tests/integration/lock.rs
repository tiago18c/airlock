//! The vault locks itself when the window stays hidden, or idle while shown.

use std::time::Duration;

use app_core::testing::{kit, kit_on, settle, Kit, MemoryStorage};
use app_core::{Event, LockReason, Settings, VaultState};

const SECOND: Duration = Duration::from_secs(1);

fn unlocked() -> Kit {
    let kit = kit();
    kit.core.create_vault("pw").unwrap();
    assert_eq!(*kit.core.slices().vault.get(), VaultState::Unlocked);
    kit
}

#[tokio::test]
async fn auto_lock_after_hidden_timeout() {
    let kit = unlocked();
    let mut events = kit.core.events();
    let wiped = wallet_vault::testing::wiped_keys();

    kit.core.set_ui_visible(false);
    settle().await;
    kit.clock.advance(Settings::DEFAULT_AUTO_LOCK - SECOND);
    settle().await;
    assert_eq!(*kit.core.slices().vault.get(), VaultState::Unlocked);
    assert!(events.try_recv().is_err());

    kit.clock.advance(2 * SECOND);
    settle().await;

    assert_eq!(*kit.core.slices().vault.get(), VaultState::Locked);
    assert_eq!(
        events.try_recv().unwrap(),
        Event::VaultLocked {
            reason: LockReason::Hidden
        }
    );
    // The secrets are gone: the document is dropped (its secrets wipe on
    // drop) and the sealing key was wiped, which the test hook counts.
    assert_eq!(wallet_vault::testing::wiped_keys(), wiped + 1);
    kit.core.with_vault(|vault| {
        assert!(!vault.is_unlocked());
        assert!(vault.data().is_err());
    });
    assert!(kit.core.slices().wallets.get().summaries.is_empty());
}

#[tokio::test]
async fn auto_lock_uses_the_timeout_in_settings() {
    let kit = unlocked();
    kit.core.set_auto_lock(Duration::from_secs(60)).unwrap();

    kit.core.set_ui_visible(false);
    settle().await;
    kit.clock.advance(61 * SECOND);
    settle().await;

    assert_eq!(*kit.core.slices().vault.get(), VaultState::Locked);
}

#[tokio::test]
async fn auto_lock_cancelled_when_shown() {
    let kit = unlocked();
    let mut events = kit.core.events();

    kit.core.set_ui_visible(false);
    settle().await;
    kit.clock.advance(Settings::DEFAULT_AUTO_LOCK - SECOND);
    settle().await;
    kit.core.set_ui_visible(true);
    settle().await;
    // Hidden timer cancelled. A short stretch while visible does not idle-lock;
    // activity keeps the idle clock from reaching the timeout.
    kit.core.ui_activity();
    settle().await;
    kit.clock.advance(Settings::DEFAULT_AUTO_LOCK - SECOND);
    settle().await;

    assert_eq!(*kit.core.slices().vault.get(), VaultState::Unlocked);
    assert!(events.try_recv().is_err());

    // Hiding again starts a fresh timeout.
    kit.core.set_ui_visible(false);
    settle().await;
    kit.clock.advance(Settings::DEFAULT_AUTO_LOCK + SECOND);
    settle().await;
    assert_eq!(*kit.core.slices().vault.get(), VaultState::Locked);
}

#[tokio::test]
async fn auto_lock_after_idle_while_visible() {
    let kit = unlocked();
    let mut events = kit.core.events();
    // Let LockPolicy arm the idle sleep before time moves.
    settle().await;

    // Visible by default: idle past the timeout locks.
    kit.clock.advance(Settings::DEFAULT_AUTO_LOCK - SECOND);
    settle().await;
    assert_eq!(*kit.core.slices().vault.get(), VaultState::Unlocked);

    kit.clock.advance(2 * SECOND);
    settle().await;
    assert_eq!(*kit.core.slices().vault.get(), VaultState::Locked);
    assert_eq!(
        events.try_recv().unwrap(),
        Event::VaultLocked {
            reason: LockReason::Idle
        }
    );
}

#[tokio::test]
async fn idle_timer_resets_on_activity() {
    let kit = unlocked();
    settle().await;

    kit.clock.advance(Settings::DEFAULT_AUTO_LOCK - SECOND);
    settle().await;
    kit.core.ui_activity();
    settle().await;
    kit.clock.advance(Settings::DEFAULT_AUTO_LOCK - SECOND);
    settle().await;

    assert_eq!(*kit.core.slices().vault.get(), VaultState::Unlocked);

    kit.clock.advance(2 * SECOND);
    settle().await;
    assert_eq!(*kit.core.slices().vault.get(), VaultState::Locked);
}

#[tokio::test]
async fn lock_now_says_it_was_the_user() {
    let kit = unlocked();
    let mut events = kit.core.events();

    kit.core.lock();
    kit.core.lock(); // already locked: nothing more is said

    assert_eq!(
        events.try_recv().unwrap(),
        Event::VaultLocked {
            reason: LockReason::User
        }
    );
    assert!(events.try_recv().is_err());
}

#[tokio::test]
async fn settings_survive_restart() {
    let storage = MemoryStorage::default();
    {
        let kit = kit_on(storage.clone());
        kit.core.set_auto_lock(Duration::from_secs(60)).unwrap();
        kit.core.set_close_to_tray(true).unwrap();
        let settings = kit.core.slices().settings.get();
        assert_eq!(settings.auto_lock, Duration::from_secs(60));
        assert!(settings.close_to_tray);
    }
    let kit = kit_on(storage);
    let settings = kit.core.slices().settings.get();
    assert_eq!(settings.auto_lock, Duration::from_secs(60));
    assert!(settings.close_to_tray);
}
