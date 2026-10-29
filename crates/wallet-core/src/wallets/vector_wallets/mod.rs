//! What the wallets of this device do with their Vector accounts: whether
//! the app offers them, the keys it holds for them, the leaves a hash-based
//! key has spent, and the move to a next key.
//!
//! The protocol itself, which needs no wallet, is [`super::vector`].

mod held;
mod leaves;
mod rotate;
mod rotation;

#[cfg(test)]
mod held_tests;
#[cfg(test)]
mod keys_to_sign_tests;
#[cfg(test)]
mod leaves_tests;
#[cfg(test)]
mod rotation_tests;

pub use held::{held_vector_accounts, local_vector_keys};
pub use leaves::{
    install_landed_rotations, keys_to_sign_with, reserve, reserve_leaf, LeafError, LeafFor,
    LeafMarks, Reserved, LEAVES_SKIPPED_WITHOUT_MARK,
};
pub use rotate::{account_state, adopt_account_state, sign_rotation, Adopted};
pub use rotation::{next_key_target, RotationError};

use crate::book::workspace::Workspace;

/// Whether the screens offer Vector wallets: to add one, to show one to a
/// site, and to rebuild a transaction for one.
pub fn offered(workspace: &Workspace) -> bool {
    workspace.vector_wallets
}

#[cfg(test)]
mod tests {
    use super::offered;

    #[test]
    fn vector_wallets_are_offered_only_once_turned_on() {
        let mut workspace = crate::book::workspace::Workspace::default();
        assert!(!offered(&workspace));
        workspace.vector_wallets = true;
        assert!(offered(&workspace));
        // A workspace file from before the setting reads as off.
        let json = serde_json::to_value(crate::book::workspace::Workspace::default()).unwrap();
        let mut older = json.as_object().unwrap().clone();
        older.remove("vectorWallets");
        let read: crate::book::workspace::Workspace =
            serde_json::from_value(serde_json::Value::Object(older)).unwrap();
        assert!(!offered(&read));
    }
}
