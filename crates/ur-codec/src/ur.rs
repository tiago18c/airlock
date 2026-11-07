//! The text of a UR: `ur:<type>/<bytewords>` for a message in one code, and
//! `ur:<type>/<seq>-<count>/<bytewords>` for one part of a longer one. A QR
//! code carries it in upper case, which alphanumeric mode packs tighter.

use crate::bytewords::{self, BytewordsError};
use crate::fountain::{Decoder, Encoder, FountainError, Part};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UrError {
    /// Not `ur:` text, or a type or sequence that does not parse.
    NotUr,
    Bytewords(BytewordsError),
    /// A part's CBOR does not read as a part.
    BadPart,
    Fountain(FountainError),
    /// A part of a UR of another type than the ones before it.
    OtherType,
}

impl std::fmt::Display for UrError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotUr => write!(f, "This is not a UR code."),
            Self::Bytewords(_) => {
                write!(f, "This code was misread. Hold it steady and scan again.")
            }
            Self::BadPart | Self::Fountain(FountainError::BadPart) => {
                write!(f, "This code is not a part of a UR.")
            }
            Self::Fountain(FountainError::OtherMessage) => {
                write!(f, "This code belongs to another request.")
            }
            Self::Fountain(FountainError::BadMessage) => {
                write!(f, "The codes do not add up to a message. Scan them again.")
            }
            Self::OtherType => write!(f, "This code is of another kind than the ones before it."),
        }
    }
}

impl std::error::Error for UrError {}

/// One scanned UR, before its parts are put together.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Scanned {
    /// The whole message in one code.
    Single {
        ur_type: String,
        message: Vec<u8>,
    },
    Part {
        ur_type: String,
        part: Part,
    },
}

/// Whether `text` starts like a UR, in either case.
pub fn is_ur(text: &str) -> bool {
    text.get(..3)
        .is_some_and(|start| start.eq_ignore_ascii_case("ur:"))
}

fn valid_type(ur_type: &str) -> bool {
    !ur_type.is_empty()
        && ur_type
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

/// Read one code's text.
pub fn parse(text: &str) -> Result<Scanned, UrError> {
    let text = text.trim();
    if !is_ur(text) {
        return Err(UrError::NotUr);
    }
    let lower = text[3..].to_ascii_lowercase();
    let pieces: Vec<&str> = lower.split('/').collect();
    let ur_type = pieces[0];
    if !valid_type(ur_type) {
        return Err(UrError::NotUr);
    }
    match pieces.as_slice() {
        [_, words] => Ok(Scanned::Single {
            ur_type: ur_type.to_string(),
            message: bytewords::decode(words).map_err(UrError::Bytewords)?,
        }),
        [_, sequence, words] => {
            let (seq, count) = sequence.split_once('-').ok_or(UrError::NotUr)?;
            let digits = |text: &str| -> Result<u32, UrError> {
                if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
                    return Err(UrError::NotUr);
                }
                text.parse().map_err(|_| UrError::NotUr)
            };
            let (seq, count) = (digits(seq)?, digits(count)?);
            let bytes = bytewords::decode(words).map_err(UrError::Bytewords)?;
            let part = Part::from_cbor(&bytes).map_err(|_| UrError::BadPart)?;
            if part.sequence != seq || part.fragments != count {
                return Err(UrError::BadPart);
            }
            Ok(Scanned::Part {
                ur_type: ur_type.to_string(),
                part,
            })
        }
        _ => Err(UrError::NotUr),
    }
}

/// `message` as a single-part UR, in upper case.
pub fn encode_single(ur_type: &str, message: &[u8]) -> String {
    format!("ur:{ur_type}/{}", bytewords::encode(message)).to_ascii_uppercase()
}

/// The codes of one UR: a single one when the message fits, otherwise
/// parts that go on for as long as the reader needs them.
pub struct UrEncoder {
    ur_type: String,
    message: Vec<u8>,
    fountain: Encoder,
}

impl UrEncoder {
    /// `None` for an empty message, an invalid type or a zero part length.
    pub fn new(ur_type: &str, message: &[u8], max_fragment: usize) -> Option<Self> {
        if !valid_type(ur_type) {
            return None;
        }
        Some(Self {
            ur_type: ur_type.to_string(),
            message: message.to_vec(),
            fountain: Encoder::new(message, max_fragment)?,
        })
    }

    pub fn is_single_part(&self) -> bool {
        self.fountain.is_single_part()
    }

    pub fn fragment_count(&self) -> usize {
        self.fountain.fragment_count()
    }

    /// The next code, in upper case. A single-part UR is the same code each
    /// time.
    pub fn next_code(&mut self) -> String {
        if self.is_single_part() {
            return encode_single(&self.ur_type, &self.message);
        }
        let part = self.fountain.next_part();
        format!(
            "ur:{}/{}-{}/{}",
            self.ur_type,
            part.sequence,
            part.fragments,
            bytewords::encode(&part.to_cbor())
        )
        .to_ascii_uppercase()
    }
}

/// Codes scanned so far, until they make one UR.
#[derive(Default)]
pub struct UrDecoder {
    ur_type: Option<String>,
    fountain: Decoder,
    single: Option<Vec<u8>>,
}

impl UrDecoder {
    /// Take one code's text. `Ok(true)` when it was new.
    pub fn receive(&mut self, text: &str) -> Result<bool, UrError> {
        let scanned = parse(text)?;
        let ur_type = match &scanned {
            Scanned::Single { ur_type, .. } | Scanned::Part { ur_type, .. } => ur_type.clone(),
        };
        match &self.ur_type {
            Some(known) if *known != ur_type => return Err(UrError::OtherType),
            Some(_) => {}
            None => self.ur_type = Some(ur_type),
        }
        match scanned {
            Scanned::Single { message, .. } => {
                let new = self.single.is_none();
                self.single = Some(message);
                Ok(new)
            }
            Scanned::Part { part, .. } => self.fountain.receive(&part).map_err(UrError::Fountain),
        }
    }

    pub fn ur_type(&self) -> Option<&str> {
        self.ur_type.as_deref()
    }

    pub fn is_complete(&self) -> bool {
        self.single.is_some() || self.fountain.is_complete()
    }

    /// Fragments known and fragments in all, for a progress bar.
    pub fn progress(&self) -> Option<(usize, usize)> {
        if self.single.is_some() {
            return Some((1, 1));
        }
        self.fountain.progress()
    }

    /// The type and message, once complete.
    pub fn result(&self) -> Result<Option<(String, Vec<u8>)>, UrError> {
        let Some(ur_type) = self.ur_type.clone() else {
            return Ok(None);
        };
        if let Some(message) = &self.single {
            return Ok(Some((ur_type, message.clone())));
        }
        Ok(self
            .fountain
            .message()
            .map_err(UrError::Fountain)?
            .map(|message| (ur_type, message)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cbor::Value;
    use crate::random::make_message;

    fn message_ur(len: usize) -> Vec<u8> {
        Value::Bytes(make_message("Wolf", len)).encode()
    }

    #[test]
    fn a_single_part_ur_matches_the_reference() {
        let message = message_ur(50);
        let want = "ur:bytes/hdeymejtswhhylkepmykhhtsytsnoyoyaxaedsuttydmmhhpktpmsrjtgwdpfnsboxgwlbaawzuefywkdplrsrjynbvygabwjldapfcsdwkbrkch";
        assert_eq!(encode_single("bytes", &message), want.to_ascii_uppercase());
        assert_eq!(
            parse(want),
            Ok(Scanned::Single {
                ur_type: "bytes".into(),
                message
            })
        );
    }

    #[test]
    fn multi_part_codes_match_the_reference() {
        let mut encoder = UrEncoder::new("bytes", &message_ur(256), 30).unwrap();
        assert_eq!(encoder.fragment_count(), 9);
        let want = [
            "ur:bytes/1-9/lpadascfadaxcywenbpljkhdcahkadaemejtswhhylkepmykhhtsytsnoyoyaxaedsuttydmmhhpktpmsrjtdkgslpgh",
            "ur:bytes/2-9/lpaoascfadaxcywenbpljkhdcagwdpfnsboxgwlbaawzuefywkdplrsrjynbvygabwjldapfcsgmghhkhstlrdcxaefz",
            "ur:bytes/3-9/lpaxascfadaxcywenbpljkhdcahelbknlkuejnbadmssfhfrdpsbiegecpasvssovlgeykssjykklronvsjksopdzmol",
        ];
        for want in want {
            assert_eq!(encoder.next_code(), want.to_ascii_uppercase());
        }
        let tenth = (4..=10).map(|_| encoder.next_code()).last().unwrap();
        assert_eq!(
            tenth,
            "ur:bytes/10-9/lpbkascfadaxcywenbpljkhdcahkadaemejtswhhylkepmykhhtsytsnoyoyaxaedsuttydmmhhpktpmsrjtwdkiplzs"
                .to_ascii_uppercase()
        );
    }

    #[test]
    fn codes_read_in_any_order_and_any_case() {
        let message = message_ur(2000);
        let mut encoder = UrEncoder::new("sol-sign-request", &message, 200).unwrap();
        let mut codes: Vec<String> = (0..40).map(|_| encoder.next_code()).collect();
        codes.reverse();
        let mut decoder = UrDecoder::default();
        for code in &codes {
            let code = if code.len() % 2 == 0 {
                code.to_ascii_lowercase()
            } else {
                code.clone()
            };
            decoder.receive(&code).unwrap();
            if decoder.is_complete() {
                break;
            }
        }
        assert_eq!(
            decoder.result(),
            Ok(Some(("sol-sign-request".into(), message)))
        );
    }

    #[test]
    fn bad_codes_are_refused() {
        assert_eq!(parse("hello"), Err(UrError::NotUr));
        assert_eq!(parse("ur:bytes#4/aeadaolazmjendeoti"), Err(UrError::NotUr));
        assert_eq!(
            parse("ur:bytes/1-1a/aeadaolazmjendeoti"),
            Err(UrError::NotUr)
        );
        assert_eq!(
            parse("ur:bytes/1-1/toomuch/aeadaolazmjendeoti"),
            Err(UrError::NotUr)
        );
        assert!(parse("ur:bytes/aeadaolazmjendeoti").is_ok());
        let mut decoder = UrDecoder::default();
        decoder.receive(&encode_single("bytes", b"a")).unwrap();
        assert_eq!(
            decoder.receive(&encode_single("crypto-hdkey", b"b")),
            Err(UrError::OtherType)
        );
    }
}
