//! Amounts as people read them.

pub const LAMPORTS_PER_SOL: u64 = 1_000_000_000;

/// `1500000000` as "1.5 SOL". Trailing zeros are dropped.
pub fn format_sol(lamports: u64) -> String {
    let whole = lamports / LAMPORTS_PER_SOL;
    let frac = lamports % LAMPORTS_PER_SOL;
    if frac == 0 {
        format!("{whole} SOL")
    } else {
        let frac = format!("{frac:09}");
        let frac = frac.trim_end_matches('0');
        format!("{whole}.{frac} SOL")
    }
}

/// A token amount in the token's own units, by its number of decimals.
pub fn format_token_amount(amount: u64, decimals: u8) -> String {
    if decimals == 0 {
        return amount.to_string();
    }
    let factor = 10u64.pow(u32::from(decimals));
    let whole = amount / factor;
    let frac = amount % factor;
    if frac == 0 {
        whole.to_string()
    } else {
        let frac = format!("{frac:0width$}", width = decimals as usize);
        format!("{whole}.{}", frac.trim_end_matches('0'))
    }
}

/// Read an amount typed in a token's units, such as "1.5", into its smallest
/// units. `None` for anything but digits with at most `decimals` of them
/// after the point, or an amount too large for a u64.
pub fn parse_amount(text: &str, decimals: u8) -> Option<u64> {
    let text = text.trim();
    let (whole, frac) = text.split_once('.').unwrap_or((text, ""));
    let digits = |part: &str| part.chars().all(|c| c.is_ascii_digit());
    if (whole.is_empty() && frac.is_empty())
        || !digits(whole)
        || !digits(frac)
        || frac.len() > usize::from(decimals)
    {
        return None;
    }
    let factor = 10u64.checked_pow(u32::from(decimals))?;
    let whole: u64 = if whole.is_empty() {
        0
    } else {
        whole.parse().ok()?
    };
    let frac: u64 = if decimals == 0 {
        0
    } else {
        format!("{frac:0<width$}", width = usize::from(decimals))
            .parse()
            .ok()?
    };
    whole.checked_mul(factor)?.checked_add(frac)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amounts_drop_trailing_zeros() {
        assert_eq!(format_sol(1_500_000_000), "1.5 SOL");
        assert_eq!(format_sol(2_000_000_000), "2 SOL");
        assert_eq!(format_sol(1), "0.000000001 SOL");
        assert_eq!(format_token_amount(1_500_000, 6), "1.5");
        assert_eq!(format_token_amount(7, 0), "7");
        assert_eq!(format_token_amount(2_000_000, 6), "2");
    }

    #[test]
    fn amounts_read_in_the_token_units() {
        assert_eq!(parse_amount("1.5", 6), Some(1_500_000));
        assert_eq!(parse_amount(" .25 ", 2), Some(25));
        assert_eq!(parse_amount("3", 0), Some(3));
        assert_eq!(parse_amount("3.", 0), Some(3));
        assert_eq!(parse_amount("0.000000001", 9), Some(1));
        assert_eq!(parse_amount("18446744073709551615", 0), Some(u64::MAX));
        for bad in [
            "",
            ".",
            "1.2.3",
            "-1",
            "1e3",
            "1.1234567",
            "18446744073709551616",
            "1,5",
        ] {
            assert_eq!(parse_amount(bad, 6), None, "{bad}");
        }
        assert_eq!(parse_amount("1", 20), None);
    }
}
