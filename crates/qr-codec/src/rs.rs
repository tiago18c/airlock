//! Reed–Solomon over GF(2^8) as QR uses it: the generator's roots are
//! α^0 … α^(n-1), and a block is written highest degree first.

use crate::gf256::{div, exp, mul};

/// The monic generator polynomial of degree `degree`, highest degree first.
fn generator(degree: usize) -> Vec<u8> {
    let mut g = vec![1u8];
    for i in 0..degree {
        let root = exp(i);
        let mut next = vec![0u8; g.len() + 1];
        for (j, &c) in g.iter().enumerate() {
            next[j] ^= c;
            next[j + 1] ^= mul(c, root);
        }
        g = next;
    }
    g
}

/// The `degree` error-correction codewords for `data`.
pub(crate) fn ecc(data: &[u8], degree: usize) -> Vec<u8> {
    if degree == 0 {
        return Vec::new();
    }
    let g = generator(degree);
    let mut rem = vec![0u8; degree];
    for &d in data {
        let factor = d ^ rem[0];
        rem.rotate_left(1);
        rem[degree - 1] = 0;
        for (r, &c) in rem.iter_mut().zip(&g[1..]) {
            *r ^= mul(c, factor);
        }
    }
    rem
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Uncorrectable;

/// Corrects `block` (data then `degree` ECC codewords) in place and returns
/// how many codewords were wrong. At most `max_errors` are corrected; a block
/// with more is rejected, never guessed.
pub(crate) fn correct(
    block: &mut [u8],
    degree: usize,
    max_errors: usize,
) -> Result<usize, Uncorrectable> {
    let n = block.len();
    if degree == 0 || degree >= n || n > 255 {
        return Err(Uncorrectable);
    }
    let synd = syndromes(block, degree);
    if synd.iter().all(|&s| s == 0) {
        return Ok(0);
    }

    let locator = berlekamp_massey(&synd);
    let errors = locator.len() - 1;
    if errors == 0 || errors > max_errors || 2 * errors > degree {
        return Err(Uncorrectable);
    }

    // Chien search: position i holds the coefficient of x^(n-1-i), so an
    // error there is a root of the locator at α^-(n-1-i).
    let mut positions = Vec::with_capacity(errors);
    for i in 0..n {
        let power = n - 1 - i;
        if eval(&locator, exp(255 - power)) == 0 {
            positions.push(i);
        }
    }
    if positions.len() != errors {
        return Err(Uncorrectable);
    }

    // Forney, for a first consecutive root of α^0: e = X·Ω(X⁻¹) / Λ'(X⁻¹).
    let omega = evaluator(&synd, &locator, degree);
    let derivative = derivative(&locator);
    for &i in &positions {
        let power = n - 1 - i;
        let x_inv = exp(255 - power);
        let den = eval(&derivative, x_inv);
        if den == 0 {
            return Err(Uncorrectable);
        }
        block[i] ^= div(mul(exp(power), eval(&omega, x_inv)), den);
    }

    if syndromes(block, degree).iter().any(|&s| s != 0) {
        return Err(Uncorrectable);
    }
    Ok(errors)
}

/// S_j = r(α^j) for j in 0..degree.
fn syndromes(block: &[u8], degree: usize) -> Vec<u8> {
    (0..degree)
        .map(|j| {
            let a = exp(j);
            block.iter().fold(0, |s, &b| mul(s, a) ^ b)
        })
        .collect()
}

/// Evaluates a polynomial stored lowest degree first.
fn eval(p: &[u8], x: u8) -> u8 {
    p.iter().rev().fold(0, |acc, &c| mul(acc, x) ^ c)
}

/// The error locator Λ, lowest degree first, of length L + 1.
fn berlekamp_massey(s: &[u8]) -> Vec<u8> {
    let mut c = vec![1u8];
    let mut b = vec![1u8];
    let mut l = 0usize;
    let mut m = 1usize;
    let mut last = 1u8;
    for k in 0..s.len() {
        let mut d = s[k];
        for i in 1..=l.min(c.len() - 1) {
            d ^= mul(c[i], s[k - i]);
        }
        if d == 0 {
            m += 1;
            continue;
        }
        let coef = div(d, last);
        let mut next = c.clone();
        if next.len() < b.len() + m {
            next.resize(b.len() + m, 0);
        }
        for (i, &bi) in b.iter().enumerate() {
            next[i + m] ^= mul(coef, bi);
        }
        if 2 * l <= k {
            l = k + 1 - l;
            b = c;
            last = d;
            m = 1;
        } else {
            m += 1;
        }
        c = next;
    }
    c.resize(l + 1, 0);
    c
}

/// Ω(x) = S(x)·Λ(x) mod x^degree, lowest degree first.
fn evaluator(s: &[u8], locator: &[u8], degree: usize) -> Vec<u8> {
    let mut o = vec![0u8; degree];
    for (i, &li) in locator.iter().enumerate() {
        for (j, &sj) in s.iter().enumerate() {
            if i + j < degree {
                o[i + j] ^= mul(li, sj);
            }
        }
    }
    o
}

/// The formal derivative. In characteristic 2 only odd powers survive.
fn derivative(p: &[u8]) -> Vec<u8> {
    (1..p.len())
        .map(|i| if i % 2 == 1 { p[i] } else { 0 })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
        fn below(&mut self, n: usize) -> usize {
            (self.next() % n as u64) as usize
        }
    }

    fn codeword(data: &[u8], degree: usize) -> Vec<u8> {
        let mut block = data.to_vec();
        block.extend(ecc(data, degree));
        block
    }

    #[test]
    fn generator_of_degree_7_matches_the_spec() {
        // ISO/IEC 18004 Annex A lists g(x) for 7 ECC codewords as exponents
        // of α: x^7 + α^87 x^6 + α^229 x^5 + α^146 x^4 + α^149 x^3
        // + α^238 x^2 + α^102 x + α^21.
        let expected: Vec<u8> = [0usize, 87, 229, 146, 149, 238, 102, 21]
            .iter()
            .map(|&e| exp(e))
            .collect();
        assert_eq!(generator(7), expected);
    }

    #[test]
    fn version_1_m_hello_world_ecc() {
        // The ECC of "HELLO WORLD" at 1-M, as published in Thonky's tutorial
        // and reproduced by every implementation.
        let data = [
            32, 91, 11, 120, 209, 114, 220, 77, 67, 64, 236, 17, 236, 17, 236, 17,
        ];
        assert_eq!(
            ecc(&data, 10),
            vec![196, 35, 39, 119, 235, 215, 231, 226, 93, 23]
        );
    }

    #[test]
    fn clean_block_has_no_errors() {
        let block = codeword(b"hello world, hello", 18);
        let mut copy = block.clone();
        assert_eq!(correct(&mut copy, 18, 9), Ok(0));
        assert_eq!(copy, block);
    }

    #[test]
    fn corrects_up_to_half_the_ecc() {
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
        for &(len, degree) in &[
            (19usize, 7usize),
            (16, 10),
            (13, 13),
            (9, 17),
            (118, 30),
            (15, 22),
        ] {
            for _ in 0..200 {
                let data: Vec<u8> = (0..len).map(|_| rng.next() as u8).collect();
                let block = codeword(&data, degree);
                let t = degree / 2;
                let errors = 1 + rng.below(t);
                let mut bad = block.clone();
                let mut hit = Vec::new();
                while hit.len() < errors {
                    let i = rng.below(bad.len());
                    if !hit.contains(&i) {
                        hit.push(i);
                        bad[i] ^= 1 + rng.below(255) as u8;
                    }
                }
                assert_eq!(correct(&mut bad, degree, t), Ok(errors));
                assert_eq!(bad, block);
            }
        }
    }

    #[test]
    fn too_many_errors_are_never_silently_accepted_as_the_original() {
        let mut rng = Rng(42);
        let mut rejected = 0;
        for _ in 0..2000 {
            let data: Vec<u8> = (0..20).map(|_| rng.next() as u8).collect();
            let degree = 10;
            let block = codeword(&data, degree);
            let mut bad = block.clone();
            let mut hit = Vec::new();
            let errors = degree / 2 + 1 + rng.below(3);
            while hit.len() < errors {
                let i = rng.below(bad.len());
                if !hit.contains(&i) {
                    hit.push(i);
                    bad[i] ^= 1 + rng.below(255) as u8;
                }
            }
            match correct(&mut bad, degree, degree / 2) {
                Err(Uncorrectable) => rejected += 1,
                // Past the bound a decoder can land on another valid
                // codeword; it can never land back on the original.
                Ok(_) => assert_ne!(bad, block),
            }
        }
        assert!(rejected > 1900, "only {rejected} rejected");
    }

    #[test]
    fn respects_max_errors_below_the_ecc_bound() {
        let block = codeword(
            &[
                1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19,
            ],
            7,
        );
        let mut bad = block.clone();
        bad[0] ^= 1;
        bad[5] ^= 1;
        bad[9] ^= 1;
        // Three errors fit in 7 ECC codewords, but 1-L reserves three for
        // misdecode protection and corrects only two.
        assert_eq!(correct(&mut bad.clone(), 7, 2), Err(Uncorrectable));
        assert_eq!(correct(&mut bad, 7, 3), Ok(3));
        assert_eq!(bad, block);
    }
}
