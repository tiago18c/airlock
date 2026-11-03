//! The fountain code that spreads one message over as many QR codes as it
//! takes. The message is cut into equal fragments. Parts 1 to n are the
//! fragments themselves; each part after that is a few fragments XORed
//! together, chosen by a generator seeded with the part's number and the
//! message's checksum. A reader that misses a code fills the hole from
//! later ones, whatever order it scans them in.

use crate::cbor::{CborError, Value};
use crate::hash::crc32;
use crate::random::Xoshiro256;
use std::collections::BTreeMap;

/// A message longer than this is refused when reading: nothing a hardware
/// wallet sends comes close, and it bounds what a forged part can ask for.
pub const MAX_MESSAGE: usize = 256 * 1024;
/// Fragments of a message, at most.
pub const MAX_FRAGMENTS: usize = 4096;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Part {
    /// From 1.
    pub sequence: u32,
    /// How many fragments the message has.
    pub fragments: u32,
    pub message_len: u32,
    pub checksum: u32,
    pub data: Vec<u8>,
}

impl Part {
    /// `[sequence, fragments, message_len, checksum, data]`.
    pub fn to_cbor(&self) -> Vec<u8> {
        Value::Array(vec![
            Value::Uint(self.sequence.into()),
            Value::Uint(self.fragments.into()),
            Value::Uint(self.message_len.into()),
            Value::Uint(self.checksum.into()),
            Value::Bytes(self.data.clone()),
        ])
        .encode()
    }

    pub fn from_cbor(bytes: &[u8]) -> Result<Self, CborError> {
        let value = Value::decode(bytes)?;
        let [sequence, fragments, message_len, checksum, data] = value.as_array()? else {
            return Err(CborError::Shape("a part of five entries"));
        };
        Ok(Self {
            sequence: sequence.as_u32()?,
            fragments: fragments.as_u32()?,
            message_len: message_len.as_u32()?,
            checksum: checksum.as_u32()?,
            data: data.as_bytes()?.to_vec(),
        })
    }

    /// The fragments this part is the XOR of.
    pub fn indexes(&self) -> Vec<usize> {
        choose_fragments(self.sequence, self.fragments as usize, self.checksum)
    }
}

/// The fragment length for `len` bytes cut into pieces of at most `max`:
/// as many pieces as `max` needs, made as even as they can be.
fn fragment_len(len: usize, max: usize) -> usize {
    let count = len.div_ceil(max);
    len.div_ceil(count)
}

fn choose_fragments(sequence: u32, fragments: usize, checksum: u32) -> Vec<usize> {
    if sequence as usize <= fragments {
        return vec![sequence as usize - 1];
    }
    let mut seed = [0u8; 8];
    seed[..4].copy_from_slice(&sequence.to_be_bytes());
    seed[4..].copy_from_slice(&checksum.to_be_bytes());
    let mut rng = Xoshiro256::from_seed(&seed);
    let degree = rng.choose_degree(fragments);
    rng.shuffled((0..fragments).collect(), degree)
}

fn xor_into(into: &mut [u8], other: &[u8]) {
    for (a, b) in into.iter_mut().zip(other) {
        *a ^= b;
    }
}

pub struct Encoder {
    fragments: Vec<Vec<u8>>,
    message_len: u32,
    checksum: u32,
    sequence: u32,
}

impl Encoder {
    /// `message` in fragments of at most `max_fragment` bytes. Empty
    /// messages and a zero length are refused.
    pub fn new(message: &[u8], max_fragment: usize) -> Option<Self> {
        if message.is_empty() || max_fragment == 0 || message.len() > MAX_MESSAGE {
            return None;
        }
        let len = fragment_len(message.len(), max_fragment);
        let mut padded = message.to_vec();
        padded.resize(message.len().div_ceil(len) * len, 0);
        Some(Self {
            fragments: padded.chunks(len).map(<[u8]>::to_vec).collect(),
            message_len: message.len() as u32,
            checksum: crc32(message),
            sequence: 0,
        })
    }

    pub fn fragment_count(&self) -> usize {
        self.fragments.len()
    }

    /// The message fits one part: it can go as a single-part UR.
    pub fn is_single_part(&self) -> bool {
        self.fragments.len() == 1
    }

    pub fn next_part(&mut self) -> Part {
        self.sequence += 1;
        let indexes = choose_fragments(self.sequence, self.fragments.len(), self.checksum);
        let mut data = vec![0; self.fragments[0].len()];
        for index in indexes {
            xor_into(&mut data, &self.fragments[index]);
        }
        Part {
            sequence: self.sequence,
            fragments: self.fragments.len() as u32,
            message_len: self.message_len,
            checksum: self.checksum,
            data,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FountainError {
    /// A part that says nothing, or more than the limits allow.
    BadPart,
    /// A part of another message than the ones before it.
    OtherMessage,
    /// Every fragment is in and they do not make the message.
    BadMessage,
}

/// Gathers parts until it has the message.
#[derive(Default)]
pub struct Decoder {
    /// Fragments, message length and checksum, from the first part.
    shape: Option<(usize, u32, u32, usize)>,
    decoded: BTreeMap<usize, Vec<u8>>,
    /// Mixed parts not reduced to one fragment yet, by what they mix.
    mixed: BTreeMap<Vec<usize>, Vec<u8>>,
    seen: std::collections::BTreeSet<Vec<usize>>,
}

impl Decoder {
    /// Take one part. `Ok(true)` when it told the decoder something new.
    pub fn receive(&mut self, part: &Part) -> Result<bool, FountainError> {
        if self.is_complete() {
            return Ok(false);
        }
        let fragments = part.fragments as usize;
        let message_len = part.message_len as usize;
        let fragment = part.data.len();
        if part.sequence == 0
            || fragments == 0
            || fragments > MAX_FRAGMENTS
            || message_len == 0
            || message_len > MAX_MESSAGE
            || fragment == 0
            || fragment * fragments < message_len
            || fragment * (fragments - 1) >= message_len
        {
            return Err(FountainError::BadPart);
        }
        let shape = (fragments, part.message_len, part.checksum, fragment);
        match self.shape {
            None => self.shape = Some(shape),
            Some(known) if known != shape => return Err(FountainError::OtherMessage),
            Some(_) => {}
        }
        let indexes = part.indexes();
        if !self.seen.insert(indexes.clone()) {
            return Ok(false);
        }
        self.reduce(indexes, part.data.clone());
        Ok(true)
    }

    /// Take out every fragment already known, then file what is left.
    fn reduce(&mut self, mut indexes: Vec<usize>, mut data: Vec<u8>) {
        indexes.retain(|index| match self.decoded.get(index) {
            Some(known) => {
                xor_into(&mut data, known);
                false
            }
            None => true,
        });
        match indexes.as_slice() {
            [] => {}
            [one] => self.solved(*one, data),
            _ => {
                indexes.sort_unstable();
                self.mixed.insert(indexes, data);
            }
        }
    }

    /// A fragment is known: take it out of every mixed part that holds it.
    fn solved(&mut self, index: usize, data: Vec<u8>) {
        let mut queue = vec![(index, data)];
        while let Some((index, data)) = queue.pop() {
            if self.decoded.contains_key(&index) {
                continue;
            }
            self.decoded.insert(index, data.clone());
            let holding: Vec<Vec<usize>> = self
                .mixed
                .keys()
                .filter(|indexes| indexes.contains(&index))
                .cloned()
                .collect();
            for indexes in holding {
                let mut mixed = self.mixed.remove(&indexes).expect("listed above");
                xor_into(&mut mixed, &data);
                let rest: Vec<usize> = indexes.into_iter().filter(|&i| i != index).collect();
                match rest.as_slice() {
                    [one] => queue.push((*one, mixed)),
                    _ => {
                        self.mixed.insert(rest, mixed);
                    }
                }
            }
        }
    }

    pub fn is_complete(&self) -> bool {
        self.shape
            .is_some_and(|(fragments, ..)| self.decoded.len() == fragments)
    }

    /// How far it is, as fragments known of fragments in all.
    pub fn progress(&self) -> Option<(usize, usize)> {
        self.shape
            .map(|(fragments, ..)| (self.decoded.len(), fragments))
    }

    /// The message, once every fragment is in and the checksum holds.
    pub fn message(&self) -> Result<Option<Vec<u8>>, FountainError> {
        let Some((_, message_len, checksum, _)) = self.shape else {
            return Ok(None);
        };
        if !self.is_complete() {
            return Ok(None);
        }
        let mut message: Vec<u8> = self.decoded.values().flatten().copied().collect();
        let padding = message.split_off(message_len as usize);
        if padding.iter().any(|&byte| byte != 0) || crc32(&message) != checksum {
            return Err(FountainError::BadMessage);
        }
        Ok(Some(message))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::random::make_message;

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    #[test]
    fn fragments_are_chosen_as_the_reference_chooses_them() {
        let message = make_message("Wolf", 1024);
        let checksum = crc32(&message);
        let expected: [&[usize]; 18] = [
            &[0],
            &[1],
            &[2],
            &[3],
            &[4],
            &[5],
            &[6],
            &[7],
            &[8],
            &[9],
            &[10],
            &[9],
            &[2, 5, 6, 8, 9, 10],
            &[8],
            &[1, 5],
            &[1],
            &[0, 2, 4, 5, 8, 10],
            &[5],
        ];
        assert_eq!(fragment_len(1024, 100), 94);
        for (sequence, want) in (1..).zip(expected) {
            let mut got = choose_fragments(sequence, 11, checksum);
            got.sort_unstable();
            assert_eq!(got, want, "part {sequence}");
        }
    }

    #[test]
    fn parts_match_the_reference() {
        let message = make_message("Wolf", 256);
        let mut encoder = Encoder::new(&message, 30).unwrap();
        assert_eq!(encoder.fragment_count(), 9);
        let expected = [
            "8501091901001a0167aa07581d916ec65cf77cadf55cd7f9cda1a1030026ddd42e905b77adc36e4f2d3c",
            "8502091901001a0167aa07581dcba44f7f04f2de44f42d84c374a0e149136f25b01852545961d55f7f7a",
        ];
        for want in expected {
            assert_eq!(hex(&encoder.next_part().to_cbor()), want);
        }
        // Past the nine fragments, parts mix them.
        let twelfth = (3..=12).map(|_| encoder.next_part()).last().unwrap();
        assert_eq!(twelfth.sequence, 12);
        assert_eq!(
            hex(&twelfth.data),
            "dd7bf725ea6c16d531b5f03254783803048ca08b87148daacd1cd7a006"
        );
        assert_eq!(Part::from_cbor(&twelfth.to_cbor()), Ok(twelfth));
    }

    #[test]
    fn a_reader_that_misses_parts_still_gets_the_message() {
        let message = make_message("Wolf", 32767);
        let mut encoder = Encoder::new(&message, 1000).unwrap();
        let mut decoder = Decoder::default();
        let mut sequence = 0;
        while !decoder.is_complete() {
            let part = encoder.next_part();
            sequence += 1;
            // Every third code is never scanned.
            if sequence % 3 != 0 {
                decoder.receive(&part).unwrap();
            }
            assert!(sequence < 200, "never completes");
        }
        assert_eq!(decoder.message(), Ok(Some(message)));
    }

    #[test]
    fn a_part_of_another_message_or_a_forged_one_is_refused() {
        let mut decoder = Decoder::default();
        let mut first = Encoder::new(&make_message("Wolf", 100), 30).unwrap();
        let mut second = Encoder::new(&make_message("Fox", 100), 30).unwrap();
        decoder.receive(&first.next_part()).unwrap();
        assert_eq!(
            decoder.receive(&second.next_part()),
            Err(FountainError::OtherMessage)
        );
        let mut forged = first.next_part();
        forged.message_len = u32::MAX;
        assert_eq!(decoder.receive(&forged), Err(FountainError::BadPart));
        forged.message_len = 100;
        forged.fragments = 0;
        assert_eq!(decoder.receive(&forged), Err(FountainError::BadPart));
    }
}
