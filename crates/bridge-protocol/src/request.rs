//! Fields shared by a bridge request the extension and the desktop app both see.

/// The most transactions one `signTransaction` may carry. The extension
/// refuses more before asking, and the desktop app answers more with
/// `not_implemented`.
pub const MAX_TRANSACTIONS: usize = 16;

/// The most requests one site may have waiting for the user. The extension
/// refuses more before asking, and the desktop app refuses them again, since
/// several browsers can ask for one site.
pub const MAX_PENDING_PER_ORIGIN: usize = 8;

/// The most requests waiting for the user, from every site together.
pub const MAX_PENDING: usize = 32;

/// The four characters shown beside a request, so it can be matched with the
/// one the extension lists.
pub fn request_code(id: &str) -> String {
    id.chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .take(4)
        .collect::<String>()
        .to_uppercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_code_takes_the_first_four_alphanumeric() {
        assert_eq!(request_code("7f3ac0de-11"), "7F3A");
        assert_eq!(request_code("ab"), "AB");
    }
}
