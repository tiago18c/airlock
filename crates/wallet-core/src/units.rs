//! Amounts as people read them.

pub const LAMPORTS_PER_SOL: u64 = 1_000_000_000;

/// `1500000000` as "1.5 SOL", and `1250000000000` as "1,250 SOL". Trailing
/// zeros are dropped, and the whole part has thousands separators.
pub fn format_sol(lamports: u64) -> String {
    let whole = group_thousands(&(lamports / LAMPORTS_PER_SOL).to_string());
    let frac = lamports % LAMPORTS_PER_SOL;
    if frac == 0 {
        format!("{whole} SOL")
    } else {
        let frac = format!("{frac:09}");
        let frac = frac.trim_end_matches('0');
        format!("{whole}.{frac} SOL")
    }
}

/// `text` with a comma between each group of three digits of the number it
/// starts with: "1250" as "1,250", "200000 CUs" as "200,000 CUs", "1250.5
/// USDC" as "1,250.5 USDC". The fraction is left as it is. Text that does not
/// start with a number standing on its own, such as "2026-11-10", "0x1f" or
/// "v23", comes back unchanged, and so does a number with a leading zero.
pub fn group_thousands(text: &str) -> String {
    let (sign, rest) = match text.strip_prefix('-') {
        Some(rest) => ("-", rest),
        None => ("", text),
    };
    let whole_len = rest.bytes().take_while(u8::is_ascii_digit).count();
    let whole = &rest[..whole_len];
    let after = &rest[whole_len..];
    let fraction_len = match after.strip_prefix('.') {
        Some(frac) => {
            let digits = frac.bytes().take_while(u8::is_ascii_digit).count();
            if digits == 0 {
                return text.to_string();
            }
            1 + digits
        }
        None => 0,
    };
    let tail = &after[fraction_len..];
    let stands_alone = tail.is_empty() || tail.starts_with(' ');
    if whole_len < 4 || whole.starts_with('0') || !stands_alone {
        return text.to_string();
    }
    let mut grouped = String::with_capacity(text.len() + whole_len / 3);
    grouped.push_str(sign);
    for (i, digit) in whole.chars().enumerate() {
        if i > 0 && (whole_len - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    grouped.push_str(&after[..fraction_len]);
    grouped.push_str(tail);
    grouped
}

/// A token amount in the token's own units, by its number of decimals.
pub fn format_token_amount(amount: u64, decimals: u8) -> String {
    format_base_digits(&amount.to_string(), decimals)
}

/// The decimal digits of an amount in base units, written in the unit
/// `decimals` places up: "1500000" at 6 as "1.5". Trailing zeros of the
/// fraction are dropped, and there are no thousands separators, so the text
/// reads back through [`decimal_to_base`] to the same digits.
pub fn format_base_digits(digits: &str, decimals: u8) -> String {
    let (whole, frac) = split_digits(digits, decimals);
    let frac = frac.trim_end_matches('0');
    if frac.is_empty() {
        whole
    } else {
        format!("{whole}.{frac}")
    }
}

/// Why [`decimal_to_base`] refused a text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecimalError {
    /// Not digits with at most one point and an optional leading minus.
    NotANumber,
    /// More decimal places than the unit has, not counting trailing zeros.
    TooManyPlaces,
}

/// An amount typed in a unit with `decimals` decimal places, such as "1.5",
/// as the digits of the integer in base units ("1500000" at 6), with a
/// leading minus when it is below zero. It works on the digits, never
/// through a float, so the result is exact for any number of decimals; the
/// caller checks that the integer fits its field.
pub fn decimal_to_base(text: &str, decimals: u8) -> Result<String, DecimalError> {
    let text = text.trim();
    let (negative, body) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text),
    };
    let (whole, frac) = body.split_once('.').unwrap_or((body, ""));
    let digits = |part: &str| part.bytes().all(|b| b.is_ascii_digit());
    if (whole.is_empty() && frac.is_empty()) || !digits(whole) || !digits(frac) {
        return Err(DecimalError::NotANumber);
    }
    let frac = frac.trim_end_matches('0');
    let places = usize::from(decimals);
    if frac.len() > places {
        return Err(DecimalError::TooManyPlaces);
    }
    let joined = format!("{whole}{frac:0<places$}");
    let joined = joined.trim_start_matches('0');
    Ok(match (joined.is_empty(), negative) {
        (true, _) => "0".to_string(),
        (false, true) => format!("-{joined}"),
        (false, false) => joined.to_string(),
    })
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
    fn numbers_are_grouped_by_thousands() {
        assert_eq!(group_thousands("1250"), "1,250");
        assert_eq!(group_thousands("200000 CUs"), "200,000 CUs");
        assert_eq!(
            group_thousands("5000 micro-lamports/CU"),
            "5,000 micro-lamports/CU"
        );
        assert_eq!(group_thousands("1234567.891 USDC"), "1,234,567.891 USDC");
        assert_eq!(group_thousands("-1250000"), "-1,250,000");
        assert_eq!(group_thousands("1000000000 (raw)"), "1,000,000,000 (raw)");
        assert_eq!(
            group_thousands("18446744073709551615"),
            "18,446,744,073,709,551,615"
        );
        // Short numbers, fractions and text that is not a number stay as they are.
        for same in [
            "999",
            "2.5 SOL",
            "0.000006 SOL",
            "",
            "none",
            "2026-11-10",
            "1234abcd",
            "0x1234",
            "v23",
            "0123",
            "1250.",
            "12:3456",
        ] {
            assert_eq!(group_thousands(same), same, "{same}");
        }
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
    fn decimals_read_into_base_unit_digits_exactly() {
        assert_eq!(decimal_to_base("1.5", 9).unwrap(), "1500000000");
        assert_eq!(decimal_to_base("0.000000001", 9).unwrap(), "1");
        assert_eq!(decimal_to_base(" .25 ", 2).unwrap(), "25");
        assert_eq!(decimal_to_base("3.", 0).unwrap(), "3");
        assert_eq!(decimal_to_base("1.50", 1).unwrap(), "15");
        assert_eq!(decimal_to_base("007", 0).unwrap(), "7");
        assert_eq!(decimal_to_base("0.00", 2).unwrap(), "0");
        assert_eq!(decimal_to_base("-0", 2).unwrap(), "0");
        assert_eq!(decimal_to_base("-1.25", 2).unwrap(), "-125");
        assert_eq!(
            decimal_to_base("1", 40).unwrap(),
            format!("1{}", "0".repeat(40))
        );
        assert_eq!(
            decimal_to_base("0.0000000001", 9),
            Err(DecimalError::TooManyPlaces)
        );
        assert_eq!(decimal_to_base("1.5", 0), Err(DecimalError::TooManyPlaces));
        for bad in ["", ".", "-", "1.2.3", "1e3", "1,5", "+1", "--1", "0x10"] {
            assert_eq!(
                decimal_to_base(bad, 6),
                Err(DecimalError::NotANumber),
                "{bad}"
            );
        }
        for (digits, decimals) in [("1500000000", 9), ("1", 9), ("25", 2), ("7", 0)] {
            let text = format_base_digits(digits, decimals);
            assert_eq!(decimal_to_base(&text, decimals).unwrap(), digits);
        }
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
