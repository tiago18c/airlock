//! Whether the window is laid out for a phone. The shell provides it; panes
//! read it to leave out what a phone has no room for.

use dioxus::prelude::*;

/// Provided by the shell: true when it was given a phone's More list.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PhoneLayout(pub bool);

/// Whether this window is laid out for a phone. False outside the shell.
pub fn use_phone_layout() -> bool {
    try_use_context::<PhoneLayout>().is_some_and(|layout| layout.0)
}
