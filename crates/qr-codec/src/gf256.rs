//! Arithmetic in GF(2^8) with the QR field polynomial
//! x^8 + x^4 + x^3 + x^2 + 1 (0x11D) and generator α = 2.

const POLY: u16 = 0x11D;

/// α^i for i in 0..512, so a sum of two logarithms indexes it directly.
static EXP: [u8; 512] = exp_table();
/// log_α(x) for x in 1..256. `LOG[0]` is unused.
static LOG: [u8; 256] = log_table();

const fn exp_table() -> [u8; 512] {
    let mut t = [0u8; 512];
    let mut x: u16 = 1;
    let mut i = 0;
    while i < 255 {
        t[i] = x as u8;
        t[i + 255] = x as u8;
        x <<= 1;
        if x & 0x100 != 0 {
            x ^= POLY;
        }
        i += 1;
    }
    t[510] = t[0];
    t[511] = t[1];
    t
}

const fn log_table() -> [u8; 256] {
    let exp = exp_table();
    let mut t = [0u8; 256];
    let mut i = 0;
    while i < 255 {
        t[exp[i] as usize] = i as u8;
        i += 1;
    }
    t
}

pub(crate) fn mul(a: u8, b: u8) -> u8 {
    if a == 0 || b == 0 {
        0
    } else {
        EXP[LOG[a as usize] as usize + LOG[b as usize] as usize]
    }
}

/// `a / b`. Callers never divide by zero; if one did, the result is 0
/// rather than a panic.
pub(crate) fn div(a: u8, b: u8) -> u8 {
    if a == 0 || b == 0 {
        0
    } else {
        EXP[LOG[a as usize] as usize + 255 - LOG[b as usize] as usize]
    }
}

/// α^n.
pub(crate) fn exp(n: usize) -> u8 {
    EXP[n % 255]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alpha_generates_every_nonzero_element() {
        let mut seen = [false; 256];
        for i in 0..255 {
            let x = exp(i);
            assert!(!seen[x as usize], "α^{i} repeats");
            seen[x as usize] = true;
        }
        assert!(!seen[0]);
    }

    #[test]
    fn field_axioms() {
        for a in 0..=255u8 {
            assert_eq!(mul(a, 1), a);
            assert_eq!(mul(a, 0), 0);
            if a != 0 {
                assert_eq!(mul(a, div(1, a)), 1, "inverse of {a}");
            }
            for b in 0..=255u8 {
                assert_eq!(mul(a, b), mul(b, a));
                if b != 0 {
                    assert_eq!(mul(div(a, b), b), a);
                }
            }
        }
        // Distributivity over a sample; the full cube is 16M cases.
        for a in (0..=255u8).step_by(7) {
            for b in (0..=255u8).step_by(5) {
                for c in (0..=255u8).step_by(3) {
                    assert_eq!(mul(a, b ^ c), mul(a, b) ^ mul(a, c));
                }
            }
        }
    }

    #[test]
    fn known_powers() {
        // α^8 = α^4 + α^3 + α^2 + 1.
        assert_eq!(exp(8), 0x1D);
        assert_eq!(exp(255), 1);
        assert_eq!(mul(0x80, 2), 0x1D);
    }
}
