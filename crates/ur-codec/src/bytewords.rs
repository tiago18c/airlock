//! Bytewords (BCR-2020-012), in the minimal style UR uses: each byte as the
//! first and last letters of its word, with the CRC-32 of the bytes added
//! at the end, big-endian.

use crate::hash::crc32;
use crate::words::WORDS;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BytewordsError {
    /// Two letters that are no word's.
    BadWord,
    /// An odd number of letters, or too few to hold the checksum.
    BadLength,
    BadChecksum,
}

/// `data` and its checksum as minimal bytewords, lower case.
pub fn encode(data: &[u8]) -> String {
    let mut out = String::with_capacity((data.len() + 4) * 2);
    for &byte in data.iter().chain(&crc32(data).to_be_bytes()) {
        let word = WORDS[usize::from(byte)].as_bytes();
        out.push(char::from(word[0]));
        out.push(char::from(word[3]));
    }
    out
}

/// The bytes of minimal bytewords, in either case, once their checksum holds.
pub fn decode(text: &str) -> Result<Vec<u8>, BytewordsError> {
    let letters = text.as_bytes();
    if !letters.len().is_multiple_of(2) || letters.len() < 8 {
        return Err(BytewordsError::BadLength);
    }
    let mut bytes = Vec::with_capacity(letters.len() / 2);
    for [first, last] in letters.as_chunks::<2>().0 {
        let (first, last) = (first.to_ascii_lowercase(), last.to_ascii_lowercase());
        let byte = WORDS
            .iter()
            .position(|word| {
                let word = word.as_bytes();
                word[0] == first && word[3] == last
            })
            .ok_or(BytewordsError::BadWord)?;
        bytes.push(byte as u8);
    }
    let checksum = bytes.split_off(bytes.len() - 4);
    if crc32(&bytes).to_be_bytes()[..] != checksum[..] {
        return Err(BytewordsError::BadChecksum);
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimal_bytewords_match_the_reference() {
        assert_eq!(encode(&[0, 1, 2, 128, 255]), "aeadaolazmjendeoti");
        assert_eq!(decode("aeadaolazmjendeoti"), Ok(vec![0, 1, 2, 128, 255]));
        assert_eq!(decode("AEADAOLAZMJENDEOTI"), Ok(vec![0, 1, 2, 128, 255]));
        assert_eq!(decode(&encode(&[])), Ok(vec![]));
    }

    #[test]
    fn a_misread_is_caught() {
        assert_eq!(
            decode("aeadaolazojendeowf"),
            Err(BytewordsError::BadChecksum)
        );
        assert_eq!(decode("abmuammdwe"), Err(BytewordsError::BadWord));
        assert_eq!(decode("aea"), Err(BytewordsError::BadLength));
    }
}
