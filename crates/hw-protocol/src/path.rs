//! BIP-32 paths, as the devices take them: indices with the top bit set for
//! a hardened one.

/// The bit that marks a hardened index.
pub const HARDENED: u32 = 0x8000_0000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PathError(pub String);

impl std::fmt::Display for PathError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "\"{}\" is not a path the Solana apps derive.", self.0)
    }
}

impl std::error::Error for PathError {}

/// `m/44'/501'/0'/0'` as indices. Only paths under `44'/501'`, of two to
/// five indices, are taken: the Solana apps derive nothing else.
pub fn parse_path(path: &str) -> Result<Vec<u32>, PathError> {
    let bad = || PathError(path.to_string());
    let rest = path.trim().strip_prefix("m/").ok_or_else(bad)?;
    let indices = rest
        .split('/')
        .map(|part| {
            let (number, hardened) = match part.strip_suffix(['\'', 'h', 'H']) {
                Some(number) => (number, true),
                None => (part, false),
            };
            if number.is_empty() || !number.bytes().all(|byte| byte.is_ascii_digit()) {
                return Err(bad());
            }
            let index = number.parse::<u32>().map_err(|_| bad())?;
            if index >= HARDENED {
                return Err(bad());
            }
            Ok(if hardened { index | HARDENED } else { index })
        })
        .collect::<Result<Vec<u32>, PathError>>()?;
    if !(2..=5).contains(&indices.len()) || indices[..2] != [44 | HARDENED, 501 | HARDENED] {
        return Err(bad());
    }
    Ok(indices)
}

/// Indices back to `m/44'/501'/0'/0'`.
pub fn format_path(indices: &[u32]) -> String {
    let mut out = String::from("m");
    for index in indices {
        out.push('/');
        out.push_str(&(index & !HARDENED).to_string());
        if index & HARDENED != 0 {
            out.push('\'');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn solana_paths_round_trip() {
        let path = parse_path("m/44'/501'/3'/0'").unwrap();
        assert_eq!(
            path,
            [44 | HARDENED, 501 | HARDENED, 3 | HARDENED, HARDENED]
        );
        assert_eq!(format_path(&path), "m/44'/501'/3'/0'");
        assert_eq!(parse_path("m/44h/501h").unwrap().len(), 2);
    }

    #[test]
    fn other_paths_are_refused() {
        for path in [
            "m/44'/60'/0'",
            "44'/501'/0'",
            "m/44'/501'/0'/0'/0'/0'",
            "m/44'/501'/+1'",
            "m/44'/501'/2147483648'",
            "m/44'/501'//",
        ] {
            assert!(parse_path(path).is_err(), "{path}");
        }
    }
}
