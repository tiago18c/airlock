//! The generator the fountain code draws from: Xoshiro256**, seeded with a
//! SHA-256, and the weighted sampler (Vose's alias method) that picks how
//! many fragments a part mixes. Encoder and decoder draw the same numbers
//! from the same seed, so a part does not have to say which fragments it
//! holds.

use crate::hash::sha256;

pub(crate) struct Xoshiro256 {
    state: [u64; 4],
}

impl Xoshiro256 {
    /// Seeded with the SHA-256 of `seed`, read as four big-endian words.
    pub(crate) fn from_seed(seed: &[u8]) -> Self {
        let hash = sha256(seed);
        let mut state = [0u64; 4];
        for (word, bytes) in state.iter_mut().zip(hash.as_chunks::<8>().0) {
            *word = u64::from_be_bytes(*bytes);
        }
        Self { state }
    }

    pub(crate) fn next_u64(&mut self) -> u64 {
        let [s0, s1, s2, s3] = &mut self.state;
        let result = s1.wrapping_mul(5).rotate_left(7).wrapping_mul(9);
        let t = *s1 << 17;
        *s2 ^= *s0;
        *s3 ^= *s1;
        *s1 ^= *s2;
        *s0 ^= *s3;
        *s2 ^= t;
        *s3 = s3.rotate_left(45);
        result
    }

    /// A number in `[0, 1)`, from the top 53 bits.
    pub(crate) fn next_double(&mut self) -> f64 {
        const SCALE: f64 = 1.0 / ((1u64 << 53) as f64);
        ((self.next_u64() >> 11) as f64) * SCALE
    }

    /// A number in `low..=high`.
    pub(crate) fn next_int(&mut self, low: u64, high: u64) -> u64 {
        (self.next_double() * ((high - low + 1) as f64)) as u64 + low
    }

    /// `count` of `items`, drawn one at a time without putting any back.
    pub(crate) fn shuffled<T>(&mut self, mut items: Vec<T>, count: usize) -> Vec<T> {
        let mut out = Vec::with_capacity(count);
        while out.len() < count && !items.is_empty() {
            let index = self.next_int(0, (items.len() - 1) as u64) as usize;
            out.push(items.remove(index));
        }
        out
    }

    /// How many of `fragments` fragments a part mixes: `d` with weight `1/d`.
    pub(crate) fn choose_degree(&mut self, fragments: usize) -> usize {
        let weights: Vec<f64> = (1..=fragments).map(|d| 1.0 / d as f64).collect();
        Sampler::new(&weights).next(self) + 1
    }
}

/// Draws an index with the probability its weight gives it.
struct Sampler {
    probs: Vec<f64>,
    aliases: Vec<usize>,
}

impl Sampler {
    /// `weights` must be positive. The order of the work lists follows the
    /// reference implementation, so the draws match it.
    fn new(weights: &[f64]) -> Self {
        let count = weights.len();
        let sum: f64 = weights.iter().sum();
        let mut scaled: Vec<f64> = weights.iter().map(|w| w * count as f64 / sum).collect();
        let (mut small, mut large): (Vec<usize>, Vec<usize>) =
            (0..count).rev().partition(|&j| scaled[j] < 1.0);
        let mut probs = vec![0.0; count];
        let mut aliases = vec![0; count];
        while let (Some(&a), Some(&g)) = (small.last(), large.last()) {
            small.pop();
            large.pop();
            probs[a] = scaled[a];
            aliases[a] = g;
            scaled[g] += scaled[a] - 1.0;
            if scaled[g] < 1.0 {
                small.push(g);
            } else {
                large.push(g);
            }
        }
        for index in large.into_iter().chain(small) {
            probs[index] = 1.0;
        }
        Self { probs, aliases }
    }

    fn next(&self, rng: &mut Xoshiro256) -> usize {
        let r1 = rng.next_double();
        let r2 = rng.next_double();
        let index = (self.probs.len() as f64 * r1) as usize;
        if r2 < self.probs[index] {
            index
        } else {
            self.aliases[index]
        }
    }
}

#[cfg(test)]
pub(crate) fn make_message(seed: &str, size: usize) -> Vec<u8> {
    let mut rng = Xoshiro256::from_seed(seed.as_bytes());
    (0..size).map(|_| rng.next_int(0, 255) as u8).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_generator_matches_the_reference() {
        let mut rng = Xoshiro256::from_seed(b"Wolf");
        let first: Vec<u64> = (0..12).map(|_| rng.next_u64() % 100).collect();
        assert_eq!(first, [42, 81, 85, 8, 82, 84, 76, 73, 70, 88, 2, 74]);

        let checksum = crate::hash::crc32(b"Wolf").to_be_bytes();
        let mut rng = Xoshiro256::from_seed(&checksum);
        let first: Vec<u64> = (0..12).map(|_| rng.next_u64() % 100).collect();
        assert_eq!(first, [88, 44, 94, 74, 0, 99, 7, 77, 68, 35, 47, 78]);

        let mut rng = Xoshiro256::from_seed(b"Wolf");
        let first: Vec<u64> = (0..12).map(|_| rng.next_int(1, 10)).collect();
        assert_eq!(first, [6, 5, 8, 4, 10, 5, 7, 10, 4, 9, 10, 9]);
    }

    #[test]
    fn shuffles_match_the_reference() {
        let mut rng = Xoshiro256::from_seed(b"Wolf");
        let values: Vec<u32> = (1..=10).collect();
        assert_eq!(
            rng.shuffled(values.clone(), 10),
            [6, 4, 9, 3, 10, 5, 7, 8, 1, 2]
        );
        assert_eq!(rng.shuffled(values, 10), [10, 8, 6, 5, 1, 2, 3, 9, 7, 4]);
    }

    #[test]
    fn the_sampler_matches_the_reference() {
        let sampler = Sampler::new(&[1.0, 2.0, 4.0, 8.0]);
        let mut rng = Xoshiro256::from_seed(b"Wolf");
        let first: Vec<usize> = (0..20).map(|_| sampler.next(&mut rng)).collect();
        assert_eq!(
            first,
            [3, 3, 3, 3, 3, 3, 3, 0, 2, 3, 3, 3, 3, 1, 2, 2, 1, 3, 3, 2]
        );
    }

    #[test]
    fn degrees_match_the_reference() {
        // A 1024-byte message in fragments of at most 100 bytes: eleven.
        let expected = [
            11, 3, 6, 5, 2, 1, 2, 11, 1, 3, 9, 10, 10, 4, 2, 1, 1, 2, 1, 1,
        ];
        for (nonce, want) in (1..=20).zip(expected) {
            let mut rng = Xoshiro256::from_seed(format!("Wolf-{nonce}").as_bytes());
            assert_eq!(rng.choose_degree(11), want);
        }
    }
}
