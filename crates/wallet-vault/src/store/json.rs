//! Opening a JSON store without ever replacing what could not be read.

use super::VaultStore;
use crate::VaultError;

/// What went wrong while opening a JSON store.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoadProblem {
    /// The stored bytes were not valid. A copy was kept and the store started empty.
    SetAside(String),
    /// The stored bytes could not be read or kept. Saving is refused so they survive.
    Blocked(String),
}

impl LoadProblem {
    pub fn message(&self) -> &str {
        match self {
            Self::SetAside(message) | Self::Blocked(message) => message,
        }
    }
}

/// Read `what` as JSON. Missing data is the default. Unreadable data is never
/// silently replaced: it is set aside, or saving is blocked.
pub fn load_json<T, S>(store: &S, what: &str) -> (T, Option<LoadProblem>)
where
    T: serde::de::DeserializeOwned + Default,
    S: VaultStore + ?Sized,
{
    let bytes = match store.load() {
        Ok(Some(bytes)) => bytes,
        Ok(None) => return (T::default(), None),
        Err(err) => {
            return (
                T::default(),
                Some(LoadProblem::Blocked(format!(
                    "Could not read the {what} ({err}). Changes to it will not be saved."
                ))),
            );
        }
    };
    match serde_json::from_slice(&bytes) {
        Ok(data) => (data, None),
        Err(err) => match store.set_aside(&bytes) {
            Ok(()) => (
                T::default(),
                Some(LoadProblem::SetAside(format!(
                    "The {what} was not readable ({err}). A copy was kept and it starts empty."
                ))),
            ),
            Err(_) => (
                T::default(),
                Some(LoadProblem::Blocked(format!(
                    "The {what} was not readable ({err}). Changes to it will not be saved."
                ))),
            ),
        },
    }
}

/// Refuse to save over data that could not be read.
pub fn check_saveable(problem: &Option<LoadProblem>) -> Result<(), VaultError> {
    match problem {
        Some(LoadProblem::Blocked(message)) => Err(VaultError::Io(message.clone())),
        _ => Ok(()),
    }
}
