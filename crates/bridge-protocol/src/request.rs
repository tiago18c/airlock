//! Fields shared by a bridge request the extension and the desktop app both see.

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
