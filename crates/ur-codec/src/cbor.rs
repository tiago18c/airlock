//! The part of CBOR (RFC 8949) that UR types use: unsigned and negative
//! integers, byte and text strings, arrays, maps, tags, booleans and null.
//! Every length is definite, and the encoder writes the shortest head, so
//! what it writes is what Keystone writes. Floats and indefinite lengths are
//! refused when reading.

use std::fmt;

/// Nesting deeper than this is refused: nothing UR carries comes close.
const MAX_DEPTH: usize = 16;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    Uint(u64),
    /// `-1 - n`, as CBOR stores a negative number.
    Nint(u64),
    Bytes(Vec<u8>),
    Text(String),
    Array(Vec<Value>),
    /// Entries in the order they are written.
    Map(Vec<(Value, Value)>),
    Tag(u64, Box<Value>),
    Bool(bool),
    Null,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CborError {
    /// The bytes end inside a value.
    Truncated,
    /// Something this subset does not read: a float, an indefinite length,
    /// a reserved head.
    Unsupported(u8),
    TooDeep,
    BadText,
    /// Bytes are left after the value.
    Trailing,
    /// A value is not of the shape a type expects.
    Shape(&'static str),
}

impl fmt::Display for CborError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated => write!(f, "the data ends early"),
            Self::Unsupported(head) => write!(f, "unsupported CBOR head 0x{head:02x}"),
            Self::TooDeep => write!(f, "the data is nested too deep"),
            Self::BadText => write!(f, "a text is not UTF-8"),
            Self::Trailing => write!(f, "bytes are left after the data"),
            Self::Shape(what) => write!(f, "expected {what}"),
        }
    }
}

impl std::error::Error for CborError {}

impl Value {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        self.write(&mut out);
        out
    }

    fn write(&self, out: &mut Vec<u8>) {
        match self {
            Self::Uint(n) => head(out, 0, *n),
            Self::Nint(n) => head(out, 1, *n),
            Self::Bytes(bytes) => {
                head(out, 2, bytes.len() as u64);
                out.extend_from_slice(bytes);
            }
            Self::Text(text) => {
                head(out, 3, text.len() as u64);
                out.extend_from_slice(text.as_bytes());
            }
            Self::Array(items) => {
                head(out, 4, items.len() as u64);
                items.iter().for_each(|item| item.write(out));
            }
            Self::Map(entries) => {
                head(out, 5, entries.len() as u64);
                for (key, value) in entries {
                    key.write(out);
                    value.write(out);
                }
            }
            Self::Tag(tag, value) => {
                head(out, 6, *tag);
                value.write(out);
            }
            Self::Bool(false) => out.push(0xf4),
            Self::Bool(true) => out.push(0xf5),
            Self::Null => out.push(0xf6),
        }
    }

    /// Read exactly one value from `bytes`.
    pub fn decode(bytes: &[u8]) -> Result<Self, CborError> {
        let mut reader = Reader { bytes, at: 0 };
        let value = reader.value(0)?;
        if reader.at != bytes.len() {
            return Err(CborError::Trailing);
        }
        Ok(value)
    }

    pub fn as_uint(&self) -> Result<u64, CborError> {
        match self {
            Self::Uint(n) => Ok(*n),
            _ => Err(CborError::Shape("an unsigned integer")),
        }
    }

    pub fn as_u32(&self) -> Result<u32, CborError> {
        u32::try_from(self.as_uint()?).map_err(|_| CborError::Shape("a 32-bit integer"))
    }

    pub fn as_bytes(&self) -> Result<&[u8], CborError> {
        match self {
            Self::Bytes(bytes) => Ok(bytes),
            _ => Err(CborError::Shape("bytes")),
        }
    }

    pub fn as_text(&self) -> Result<&str, CborError> {
        match self {
            Self::Text(text) => Ok(text),
            _ => Err(CborError::Shape("text")),
        }
    }

    pub fn as_bool(&self) -> Result<bool, CborError> {
        match self {
            Self::Bool(value) => Ok(*value),
            _ => Err(CborError::Shape("a boolean")),
        }
    }

    pub fn as_array(&self) -> Result<&[Value], CborError> {
        match self {
            Self::Array(items) => Ok(items),
            _ => Err(CborError::Shape("an array")),
        }
    }

    pub fn as_map(&self) -> Result<&[(Value, Value)], CborError> {
        match self {
            Self::Map(entries) => Ok(entries),
            _ => Err(CborError::Shape("a map")),
        }
    }

    /// The value under tag `tag`. A missing tag is accepted: UR types embed
    /// their registry tags, but some writers leave nested ones out.
    pub fn untag(&self, tag: u64) -> Result<&Value, CborError> {
        match self {
            Self::Tag(found, inner) if *found == tag => Ok(inner),
            Self::Tag(..) => Err(CborError::Shape("another tag")),
            other => Ok(other),
        }
    }

    /// The entry of a map with integer key `key`.
    pub fn get(&self, key: u64) -> Result<Option<&Value>, CborError> {
        Ok(self
            .as_map()?
            .iter()
            .find(|(found, _)| *found == Value::Uint(key))
            .map(|(_, value)| value))
    }

    /// As [`Self::get`], for an entry that has to be there.
    pub fn need(&self, key: u64) -> Result<&Value, CborError> {
        self.get(key)?
            .ok_or(CborError::Shape("a required map entry"))
    }
}

fn head(out: &mut Vec<u8>, major: u8, n: u64) {
    let major = major << 5;
    match n {
        0..=23 => out.push(major | n as u8),
        24..=0xff => out.extend_from_slice(&[major | 24, n as u8]),
        0x100..=0xffff => {
            out.push(major | 25);
            out.extend_from_slice(&(n as u16).to_be_bytes());
        }
        0x1_0000..=0xffff_ffff => {
            out.push(major | 26);
            out.extend_from_slice(&(n as u32).to_be_bytes());
        }
        _ => {
            out.push(major | 27);
            out.extend_from_slice(&n.to_be_bytes());
        }
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, count: usize) -> Result<&'a [u8], CborError> {
        let end = self.at.checked_add(count).ok_or(CborError::Truncated)?;
        let slice = self.bytes.get(self.at..end).ok_or(CborError::Truncated)?;
        self.at = end;
        Ok(slice)
    }

    fn argument(&mut self, first: u8) -> Result<u64, CborError> {
        Ok(match first & 0x1f {
            n @ 0..=23 => u64::from(n),
            24 => u64::from(self.take(1)?[0]),
            25 => u64::from(u16::from_be_bytes(
                self.take(2)?.try_into().expect("two bytes"),
            )),
            26 => u64::from(u32::from_be_bytes(
                self.take(4)?.try_into().expect("four bytes"),
            )),
            27 => u64::from_be_bytes(self.take(8)?.try_into().expect("eight bytes")),
            _ => return Err(CborError::Unsupported(first)),
        })
    }

    /// A length that has to fit in what is left, so a forged length cannot
    /// make the reader allocate.
    fn length(&mut self, first: u8) -> Result<usize, CborError> {
        let n = self.argument(first)?;
        let left = (self.bytes.len() - self.at) as u64;
        if n > left {
            return Err(CborError::Truncated);
        }
        Ok(n as usize)
    }

    fn value(&mut self, depth: usize) -> Result<Value, CborError> {
        if depth > MAX_DEPTH {
            return Err(CborError::TooDeep);
        }
        let first = self.take(1)?[0];
        Ok(match first >> 5 {
            0 => Value::Uint(self.argument(first)?),
            1 => Value::Nint(self.argument(first)?),
            2 => {
                let len = self.length(first)?;
                Value::Bytes(self.take(len)?.to_vec())
            }
            3 => {
                let len = self.length(first)?;
                let text = std::str::from_utf8(self.take(len)?).map_err(|_| CborError::BadText)?;
                Value::Text(text.to_string())
            }
            4 => {
                let len = self.length(first)?;
                let mut items = Vec::with_capacity(len);
                for _ in 0..len {
                    items.push(self.value(depth + 1)?);
                }
                Value::Array(items)
            }
            5 => {
                let len = self.length(first)?;
                let mut entries = Vec::with_capacity(len);
                for _ in 0..len {
                    let key = self.value(depth + 1)?;
                    let value = self.value(depth + 1)?;
                    entries.push((key, value));
                }
                Value::Map(entries)
            }
            6 => {
                let tag = self.argument(first)?;
                Value::Tag(tag, Box::new(self.value(depth + 1)?))
            }
            _ => match first {
                0xf4 => Value::Bool(false),
                0xf5 => Value::Bool(true),
                0xf6 => Value::Null,
                _ => return Err(CborError::Unsupported(first)),
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    #[test]
    fn heads_are_as_short_as_they_can_be() {
        let cases = [
            (Value::Uint(0), "00"),
            (Value::Uint(23), "17"),
            (Value::Uint(24), "1818"),
            (Value::Uint(256), "190100"),
            (Value::Uint(23_570_951), "1a0167aa07"),
            (Value::Uint(u64::MAX), "1bffffffffffffffff"),
            (Value::Nint(0), "20"),
            (Value::Bytes(vec![1, 2]), "420102"),
            (Value::Text("SOL".into()), "63534f4c"),
            (Value::Bool(true), "f5"),
            (Value::Tag(304, Box::new(Value::Null)), "d90130f6"),
        ];
        for (value, want) in cases {
            assert_eq!(hex(&value.encode()), want);
            assert_eq!(Value::decode(&value.encode()), Ok(value));
        }
    }

    #[test]
    fn maps_keep_their_order_and_answer_by_key() {
        let value = Value::Map(vec![
            (Value::Uint(2), Value::Bytes(vec![9])),
            (Value::Uint(1), Value::Text("a".into())),
        ]);
        let back = Value::decode(&value.encode()).unwrap();
        assert_eq!(back, value);
        assert_eq!(back.need(1).unwrap().as_text(), Ok("a"));
        assert_eq!(back.get(3), Ok(None));
    }

    #[test]
    fn bad_input_is_refused_without_allocating() {
        // A byte string that says it is four gigabytes long.
        assert_eq!(
            Value::decode(&[0x5a, 0xff, 0xff, 0xff, 0xff]),
            Err(CborError::Truncated)
        );
        // An indefinite-length array, and a float.
        assert_eq!(Value::decode(&[0x9f]), Err(CborError::Unsupported(0x9f)));
        assert_eq!(
            Value::decode(&[0xfb, 0, 0, 0, 0, 0, 0, 0, 0]),
            Err(CborError::Unsupported(0xfb))
        );
        assert_eq!(Value::decode(&[0x00, 0x00]), Err(CborError::Trailing));
        assert_eq!(Value::decode(&[0x81; 40]), Err(CborError::TooDeep));
        assert_eq!(Value::decode(&[0x62, 0xff, 0xfe]), Err(CborError::BadText));
    }
}
