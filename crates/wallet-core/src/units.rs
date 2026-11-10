//! Amounts as people read them.

pub const LAMPORTS_PER_SOL: u64 = 1_000_000_000;

/// `1500000000` as "1.5 SOL", and `1250000000000` as "1,250 SOL". Trailing
/// zeros are dropped, and the whole part has thousands separators.
pub fn format_sol(lamports: u64) -> String {
    let whole = group_thousands(lamports / LAMPORTS_PER_SOL);
    let frac = lamports % LAMPORTS_PER_SOL;
    if frac == 0 {
        format!("{whole} SOL")
    } else {
        let frac = format!("{frac:09}");
        let frac = frac.trim_end_matches('0');
        format!("{whole}.{frac} SOL")
    }
}

/// `1250000` as "1,250,000".
fn group_thousands(value: u64) -> String {
    let digits = value.to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (at, digit) in digits.chars().enumerate() {
        if at > 0 && (digits.len() - at).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    grouped
}

/// A token amount in the token's own units, by its number of decimals.
pub fn format_token_amount(amount: u64, decimals: u8) -> String {
    let (whole, frac) = split_digits(&amount.to_string(), decimals);
    let frac = frac.trim_end_matches('0');
    if frac.is_empty() {
        whole
    } else {
        format!("{whole}.{frac}")
    }
}

/// The decimal digits of an amount split `decimals` places from the right:
/// the whole part, "0" when there is none, and the fraction, padded with
/// zeros to `decimals` digits. It works on the digits rather than dividing
/// by ten to the `decimals`, since a mint can claim up to 255 decimals and
/// no integer holds that factor.
pub(crate) fn split_digits(digits: &str, decimals: u8) -> (String, String) {
    let decimals = usize::from(decimals);
    match digits.len().checked_sub(decimals) {
        Some(split) if split > 0 => {
            let (whole, frac) = digits.split_at(split);
            (whole.to_string(), frac.to_string())
        }
        _ => ("0".to_string(), format!("{digits:0>decimals$}")),
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
        assert_eq!(format_sol(999 * LAMPORTS_PER_SOL), "999 SOL");
        assert_eq!(format_sol(1_250 * LAMPORTS_PER_SOL), "1,250 SOL");
        assert_eq!(format_sol(1_250_500_000_000), "1,250.5 SOL");
        assert_eq!(format_sol(1_234_567 * LAMPORTS_PER_SOL), "1,234,567 SOL");
        assert_eq!(format_sol(u64::MAX), "18,446,744,073.709551615 SOL");
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

    #[test]
    fn any_number_of_decimals_prints_the_amount() {
        let max = u64::MAX; // 18446744073709551615, 20 digits
        assert_eq!(format_token_amount(max, 0), "18446744073709551615");
        assert_eq!(format_token_amount(max, 9), "18446744073.709551615");
        assert_eq!(format_token_amount(max, 19), "1.8446744073709551615");
        assert_eq!(format_token_amount(max, 20), "0.18446744073709551615");
        assert_eq!(
            format_token_amount(max, 38),
            "0.00000000000000000018446744073709551615"
        );
        assert_eq!(format_token_amount(5, 39), format!("0.{}5", "0".repeat(38)));
        assert_eq!(format_token_amount(5, 64), format!("0.{}5", "0".repeat(63)));
        assert_eq!(
            format_token_amount(120, 255),
            format!("0.{}12", "0".repeat(252))
        );
        for decimals in [0, 9, 19, 20, 38, 39, 64, 255] {
            assert_eq!(format_token_amount(0, decimals), "0", "{decimals}");
        }
        assert_eq!(format_token_amount(10_000_000_000_000_000_000, 19), "1");
        assert_eq!(format_token_amount(10, 1), "1");
    }
}
