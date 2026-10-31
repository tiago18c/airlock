use crate::value::{NumberValue, Value};
use codama_nodes::{Endianness, NumberFormat, PostOffsetStrategy, PreOffsetStrategy};
use codama_visitors_core::{DynamicError as CodamaError, DynamicResult as CodamaResult};
use indexmap::IndexMap;
use std::cell::Cell;
use std::rc::Rc;

/// A runtime codec able to encode a [`Value`] to bytes and decode it back.
///
/// `decode` advances the input slice past the bytes it consumed.
pub trait Codec {
    fn encode(&self, value: &Value, out: &mut Vec<u8>) -> CodamaResult<()>;
    fn decode(&self, input: &mut &[u8]) -> CodamaResult<Value>;

    /// The fixed byte size of this codec, if it is fixed-size.
    fn fixed_size(&self) -> Option<usize> {
        None
    }

    /// Convenience helper to encode into a fresh buffer.
    fn encode_to_vec(&self, value: &Value) -> CodamaResult<Vec<u8>> {
        let mut out = Vec::new();
        self.encode(value, &mut out)?;
        Ok(out)
    }
}

fn err(msg: impl Into<String>) -> CodamaError {
    CodamaError::InvalidBytesEncoding(msg.into())
}

/// Reads `n` bytes off the front of `input`, advancing it.
fn take<'a>(input: &mut &'a [u8], n: usize) -> CodamaResult<&'a [u8]> {
    if input.len() < n {
        return Err(err(format!(
            "unexpected end of input: needed {n} bytes, had {}",
            input.len()
        )));
    }
    let (head, tail) = input.split_at(n);
    *input = tail;
    Ok(head)
}

// --------------------------------------------------------------------------
// Number
// --------------------------------------------------------------------------

pub struct NumberCodec {
    pub format: NumberFormat,
    pub endian: Endianness,
}

impl NumberCodec {
    fn width(&self) -> Option<usize> {
        use NumberFormat::*;
        Some(match self.format {
            U8 | I8 => 1,
            U16 | I16 => 2,
            U32 | I32 | F32 => 4,
            U64 | I64 | F64 => 8,
            U128 | I128 => 16,
            ShortU16 => return None,
        })
    }
}

macro_rules! write_int {
    ($out:expr, $endian:expr, $val:expr, $t:ty) => {{
        let v = $val as $t;
        match $endian {
            Endianness::Le => $out.extend_from_slice(&v.to_le_bytes()),
            Endianness::Be => $out.extend_from_slice(&v.to_be_bytes()),
        }
    }};
}

macro_rules! read_int {
    ($bytes:expr, $endian:expr, $t:ty) => {{
        let arr: [u8; std::mem::size_of::<$t>()] = $bytes.try_into().unwrap();
        match $endian {
            Endianness::Le => <$t>::from_le_bytes(arr),
            Endianness::Be => <$t>::from_be_bytes(arr),
        }
    }};
}

impl Codec for NumberCodec {
    fn encode(&self, value: &Value, out: &mut Vec<u8>) -> CodamaResult<()> {
        use NumberFormat::*;
        let n = match value {
            Value::Number(n) => *n,
            other => return Err(err(format!("expected a number, got {other:?}"))),
        };
        match self.format {
            U8 => out.push(n.as_u128() as u8),
            U16 => write_int!(out, self.endian, n.as_u128(), u16),
            U32 => write_int!(out, self.endian, n.as_u128(), u32),
            U64 => write_int!(out, self.endian, n.as_u128(), u64),
            U128 => write_int!(out, self.endian, n.as_u128(), u128),
            I8 => out.push(n.as_i128() as u8),
            I16 => write_int!(out, self.endian, n.as_i128(), i16),
            I32 => write_int!(out, self.endian, n.as_i128(), i32),
            I64 => write_int!(out, self.endian, n.as_i128(), i64),
            I128 => write_int!(out, self.endian, n.as_i128(), i128),
            F32 => write_int!(out, self.endian, n.as_f64() as f32, f32),
            F64 => write_int!(out, self.endian, n.as_f64(), f64),
            ShortU16 => encode_short_u16(n.as_u128() as u16, out),
        }
        Ok(())
    }

    fn decode(&self, input: &mut &[u8]) -> CodamaResult<Value> {
        use NumberFormat::*;
        let n = match self.format {
            U8 => NumberValue::Unsigned(take(input, 1)?[0] as u128),
            U16 => NumberValue::Unsigned(read_int!(take(input, 2)?, self.endian, u16) as u128),
            U32 => NumberValue::Unsigned(read_int!(take(input, 4)?, self.endian, u32) as u128),
            U64 => NumberValue::Unsigned(read_int!(take(input, 8)?, self.endian, u64) as u128),
            U128 => NumberValue::Unsigned(read_int!(take(input, 16)?, self.endian, u128)),
            I8 => NumberValue::Signed(take(input, 1)?[0] as i8 as i128),
            I16 => NumberValue::Signed(read_int!(take(input, 2)?, self.endian, i16) as i128),
            I32 => NumberValue::Signed(read_int!(take(input, 4)?, self.endian, i32) as i128),
            I64 => NumberValue::Signed(read_int!(take(input, 8)?, self.endian, i64) as i128),
            I128 => NumberValue::Signed(read_int!(take(input, 16)?, self.endian, i128)),
            F32 => NumberValue::Float(read_int!(take(input, 4)?, self.endian, f32) as f64),
            F64 => NumberValue::Float(read_int!(take(input, 8)?, self.endian, f64)),
            ShortU16 => NumberValue::Unsigned(decode_short_u16(input)? as u128),
        };
        Ok(Value::Number(n))
    }

    fn fixed_size(&self) -> Option<usize> {
        self.width()
    }
}

fn encode_short_u16(mut value: u16, out: &mut Vec<u8>) {
    loop {
        let mut elem = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            out.push(elem);
            break;
        }
        elem |= 0x80;
        out.push(elem);
    }
}

fn decode_short_u16(input: &mut &[u8]) -> CodamaResult<u16> {
    let mut value: u32 = 0;
    let mut i = 0;
    loop {
        let byte = take(input, 1)?[0];
        value |= ((byte & 0x7f) as u32) << (i * 7);
        if byte & 0x80 == 0 {
            break;
        }
        i += 1;
        if i > 2 {
            return Err(err("invalid shortU16 encoding"));
        }
    }
    Ok(value as u16)
}

// --------------------------------------------------------------------------
// Boolean
// --------------------------------------------------------------------------

pub struct BoolCodec {
    pub size: Box<dyn Codec>,
}

impl Codec for BoolCodec {
    fn encode(&self, value: &Value, out: &mut Vec<u8>) -> CodamaResult<()> {
        let b = match value {
            Value::Bool(b) => *b,
            other => return Err(err(format!("expected a boolean, got {other:?}"))),
        };
        self.size
            .encode(&Value::Number(NumberValue::Unsigned(b as u128)), out)
    }

    fn decode(&self, input: &mut &[u8]) -> CodamaResult<Value> {
        let n = match self.size.decode(input)? {
            Value::Number(n) => n.as_u128(),
            _ => return Err(err("boolean prefix did not decode to a number")),
        };
        Ok(Value::Bool(n != 0))
    }

    fn fixed_size(&self) -> Option<usize> {
        self.size.fixed_size()
    }
}

// --------------------------------------------------------------------------
// Bytes (consumes the remainder of the input)
// --------------------------------------------------------------------------

pub struct BytesCodec;

impl Codec for BytesCodec {
    fn encode(&self, value: &Value, out: &mut Vec<u8>) -> CodamaResult<()> {
        match value {
            Value::Bytes(b) => {
                out.extend_from_slice(b);
                Ok(())
            }
            other => Err(err(format!("expected bytes, got {other:?}"))),
        }
    }

    fn decode(&self, input: &mut &[u8]) -> CodamaResult<Value> {
        let bytes = input.to_vec();
        *input = &input[input.len()..];
        Ok(Value::Bytes(bytes))
    }
}

// --------------------------------------------------------------------------
// String (uses a bytes encoding, consumes the remainder of the input)
// --------------------------------------------------------------------------

pub struct StringCodec {
    pub encoding: codama_nodes::BytesEncoding,
}

impl Codec for StringCodec {
    fn encode(&self, value: &Value, out: &mut Vec<u8>) -> CodamaResult<()> {
        let s = match value {
            Value::String(s) => s,
            other => return Err(err(format!("expected a string, got {other:?}"))),
        };
        out.extend_from_slice(&crate::encoding::decode_bytes(self.encoding, s)?);
        Ok(())
    }

    fn decode(&self, input: &mut &[u8]) -> CodamaResult<Value> {
        let bytes = input.to_vec();
        *input = &input[input.len()..];
        Ok(Value::String(crate::encoding::encode_bytes(
            self.encoding,
            &bytes,
        )?))
    }
}

// --------------------------------------------------------------------------
// Public key (32 raw bytes)
// --------------------------------------------------------------------------

pub struct PublicKeyCodec;

impl Codec for PublicKeyCodec {
    fn encode(&self, value: &Value, out: &mut Vec<u8>) -> CodamaResult<()> {
        match value {
            Value::PublicKey(k) => {
                out.extend_from_slice(k);
                Ok(())
            }
            other => Err(err(format!("expected a public key, got {other:?}"))),
        }
    }

    fn decode(&self, input: &mut &[u8]) -> CodamaResult<Value> {
        let bytes = take(input, 32)?;
        let mut key = [0u8; 32];
        key.copy_from_slice(bytes);
        Ok(Value::PublicKey(key))
    }

    fn fixed_size(&self) -> Option<usize> {
        Some(32)
    }
}

// --------------------------------------------------------------------------
// Struct
// --------------------------------------------------------------------------

pub struct StructCodec {
    pub fields: Vec<(String, Box<dyn Codec>)>,
}

impl Codec for StructCodec {
    fn encode(&self, value: &Value, out: &mut Vec<u8>) -> CodamaResult<()> {
        let map = match value {
            Value::Struct(map) => map,
            other => return Err(err(format!("expected a struct, got {other:?}"))),
        };
        for (name, codec) in &self.fields {
            let field = map
                .get(name)
                .ok_or_else(|| err(format!("missing struct field `{name}`")))?;
            codec.encode(field, out)?;
        }
        Ok(())
    }

    fn decode(&self, input: &mut &[u8]) -> CodamaResult<Value> {
        let mut map = IndexMap::new();
        for (name, codec) in &self.fields {
            map.insert(name.clone(), codec.decode(input)?);
        }
        Ok(Value::Struct(map))
    }

    fn fixed_size(&self) -> Option<usize> {
        self.fields
            .iter()
            .map(|(_, c)| c.fixed_size())
            .sum::<Option<usize>>()
    }
}

// --------------------------------------------------------------------------
// Array / Set
// --------------------------------------------------------------------------

pub enum CodecCount {
    Fixed(usize),
    Prefixed(Box<dyn Codec>),
    Remainder,
}

pub struct ArrayCodec {
    pub item: Box<dyn Codec>,
    pub count: CodecCount,
    /// When true, decodes into [`Value::Set`] instead of [`Value::Array`].
    pub as_set: bool,
}

impl ArrayCodec {
    fn items(value: &Value) -> CodamaResult<&Vec<Value>> {
        match value {
            Value::Array(items) | Value::Set(items) | Value::Tuple(items) => Ok(items),
            other => Err(err(format!("expected an array, got {other:?}"))),
        }
    }
}

impl Codec for ArrayCodec {
    fn encode(&self, value: &Value, out: &mut Vec<u8>) -> CodamaResult<()> {
        let items = Self::items(value)?;
        match &self.count {
            CodecCount::Fixed(n) => {
                if items.len() != *n {
                    return Err(err(format!(
                        "fixed array expected {n} items, got {}",
                        items.len()
                    )));
                }
            }
            CodecCount::Prefixed(prefix) => {
                prefix.encode(
                    &Value::Number(NumberValue::Unsigned(items.len() as u128)),
                    out,
                )?;
            }
            CodecCount::Remainder => {}
        }
        for item in items {
            self.item.encode(item, out)?;
        }
        Ok(())
    }

    fn decode(&self, input: &mut &[u8]) -> CodamaResult<Value> {
        let mut items = Vec::new();
        match &self.count {
            CodecCount::Fixed(n) => {
                for _ in 0..*n {
                    items.push(self.item.decode(input)?);
                }
            }
            CodecCount::Prefixed(prefix) => {
                let len = match prefix.decode(input)? {
                    Value::Number(n) => n.as_u128() as usize,
                    _ => return Err(err("array prefix did not decode to a number")),
                };
                for _ in 0..len {
                    items.push(self.item.decode(input)?);
                }
            }
            CodecCount::Remainder => {
                while !input.is_empty() {
                    items.push(self.item.decode(input)?);
                }
            }
        }
        Ok(if self.as_set {
            Value::Set(items)
        } else {
            Value::Array(items)
        })
    }

    fn fixed_size(&self) -> Option<usize> {
        match &self.count {
            CodecCount::Fixed(n) => self.item.fixed_size().map(|s| s * n),
            _ => None,
        }
    }
}

/// How many items that consume no bytes one codec reads, in all its lists.
const MAX_EMPTY_ITEMS: usize = 1 << 12;

/// An item of a list or a key of a map, counted when it consumes no bytes.
///
/// How many items a list holds comes from the IDL or from the bytes. An item
/// that consumes nothing never runs out of input, so nothing else stops four
/// billion empty structs, or a list of lists of them.
pub struct ListItemCodec {
    pub item: Box<dyn Codec>,
    /// Shared by every list of one codec.
    pub empty_items: Rc<Cell<usize>>,
}

impl Codec for ListItemCodec {
    fn encode(&self, value: &Value, out: &mut Vec<u8>) -> CodamaResult<()> {
        self.item.encode(value, out)
    }

    fn decode(&self, input: &mut &[u8]) -> CodamaResult<Value> {
        let before = input.len();
        let value = self.item.decode(input)?;
        if input.len() == before {
            let empty_items = self.empty_items.get() + 1;
            if empty_items > MAX_EMPTY_ITEMS {
                return Err(err(format!(
                    "more than {MAX_EMPTY_ITEMS} list items consumed no bytes"
                )));
            }
            self.empty_items.set(empty_items);
        }
        Ok(value)
    }

    fn fixed_size(&self) -> Option<usize> {
        self.item.fixed_size()
    }
}

// --------------------------------------------------------------------------
// Tuple
// --------------------------------------------------------------------------

pub struct TupleCodec {
    pub items: Vec<Box<dyn Codec>>,
}

impl Codec for TupleCodec {
    fn encode(&self, value: &Value, out: &mut Vec<u8>) -> CodamaResult<()> {
        let items = match value {
            Value::Tuple(items) | Value::Array(items) => items,
            other => return Err(err(format!("expected a tuple, got {other:?}"))),
        };
        if items.len() != self.items.len() {
            return Err(err(format!(
                "tuple expected {} items, got {}",
                self.items.len(),
                items.len()
            )));
        }
        for (codec, item) in self.items.iter().zip(items) {
            codec.encode(item, out)?;
        }
        Ok(())
    }

    fn decode(&self, input: &mut &[u8]) -> CodamaResult<Value> {
        let mut items = Vec::with_capacity(self.items.len());
        for codec in &self.items {
            items.push(codec.decode(input)?);
        }
        Ok(Value::Tuple(items))
    }

    fn fixed_size(&self) -> Option<usize> {
        self.items
            .iter()
            .map(|c| c.fixed_size())
            .sum::<Option<usize>>()
    }
}

// --------------------------------------------------------------------------
// Option
// --------------------------------------------------------------------------

pub struct OptionCodec {
    pub prefix: Box<dyn Codec>,
    pub item: Box<dyn Codec>,
    /// When true, the `None` case is padded with zeroes to the item's size and
    /// the present/absent flag occupies a fixed slot.
    pub fixed: bool,
}

impl Codec for OptionCodec {
    fn encode(&self, value: &Value, out: &mut Vec<u8>) -> CodamaResult<()> {
        let inner = match value {
            Value::Option(inner) => inner,
            other => return Err(err(format!("expected an option, got {other:?}"))),
        };
        match inner {
            Some(v) => {
                self.prefix
                    .encode(&Value::Number(NumberValue::Unsigned(1)), out)?;
                self.item.encode(v, out)?;
            }
            None => {
                self.prefix
                    .encode(&Value::Number(NumberValue::Unsigned(0)), out)?;
                if self.fixed {
                    let size = self
                        .item
                        .fixed_size()
                        .ok_or_else(|| err("fixed option requires a fixed-size item"))?;
                    out.extend(std::iter::repeat_n(0u8, size));
                }
            }
        }
        Ok(())
    }

    fn decode(&self, input: &mut &[u8]) -> CodamaResult<Value> {
        let present = match self.prefix.decode(input)? {
            Value::Number(n) => n.as_u128() != 0,
            _ => return Err(err("option prefix did not decode to a number")),
        };
        if present {
            let v = self.item.decode(input)?;
            Ok(Value::Option(Some(Box::new(v))))
        } else {
            if self.fixed {
                let size = self
                    .item
                    .fixed_size()
                    .ok_or_else(|| err("fixed option requires a fixed-size item"))?;
                take(input, size)?;
            }
            Ok(Value::Option(None))
        }
    }

    fn fixed_size(&self) -> Option<usize> {
        if self.fixed {
            Some(self.prefix.fixed_size()? + self.item.fixed_size()?)
        } else {
            None
        }
    }
}

// --------------------------------------------------------------------------
// Fixed-size wrapper
// --------------------------------------------------------------------------

pub struct FixedSizeCodec {
    pub inner: Box<dyn Codec>,
    pub size: usize,
}

impl Codec for FixedSizeCodec {
    fn encode(&self, value: &Value, out: &mut Vec<u8>) -> CodamaResult<()> {
        let start = out.len();
        self.inner.encode(value, out)?;
        let written = out.len() - start;
        if written > self.size {
            return Err(err(format!(
                "fixed-size codec overflow: wrote {written}, max {}",
                self.size
            )));
        }
        out.extend(std::iter::repeat_n(0u8, self.size - written));
        Ok(())
    }

    fn decode(&self, input: &mut &[u8]) -> CodamaResult<Value> {
        let frame = take(input, self.size)?;
        let mut sub = frame;
        match self.inner.decode(&mut sub)? {
            // The zeros `encode` pads a string with are not part of it, as
            // Codama's TypeScript decoder strips them.
            Value::String(s) => Ok(Value::String(s.trim_end_matches('\0').to_string())),
            value => Ok(value),
        }
    }

    fn fixed_size(&self) -> Option<usize> {
        Some(self.size)
    }
}

// --------------------------------------------------------------------------
// Size-prefix wrapper (length in bytes, not item count)
// --------------------------------------------------------------------------

pub struct SizePrefixCodec {
    pub inner: Box<dyn Codec>,
    pub prefix: Box<dyn Codec>,
}

impl Codec for SizePrefixCodec {
    fn encode(&self, value: &Value, out: &mut Vec<u8>) -> CodamaResult<()> {
        let mut body = Vec::new();
        self.inner.encode(value, &mut body)?;
        self.prefix.encode(
            &Value::Number(NumberValue::Unsigned(body.len() as u128)),
            out,
        )?;
        out.extend_from_slice(&body);
        Ok(())
    }

    fn decode(&self, input: &mut &[u8]) -> CodamaResult<Value> {
        let len = match self.prefix.decode(input)? {
            Value::Number(n) => n.as_u128() as usize,
            _ => return Err(err("size prefix did not decode to a number")),
        };
        let frame = take(input, len)?;
        let mut sub = frame;
        self.inner.decode(&mut sub)
    }
}

// --------------------------------------------------------------------------
// Scalar enum (encoded as a number index)
// --------------------------------------------------------------------------

pub struct ScalarEnumCodec {
    pub size: Box<dyn Codec>,
    pub variants: Vec<String>,
}

impl Codec for ScalarEnumCodec {
    fn encode(&self, value: &Value, out: &mut Vec<u8>) -> CodamaResult<()> {
        let index = match value {
            Value::Enum { variant, .. } => self
                .variants
                .iter()
                .position(|v| v == variant)
                .ok_or_else(|| err(format!("unknown enum variant `{variant}`")))?,
            Value::Number(n) => n.as_u128() as usize,
            other => return Err(err(format!("expected an enum variant, got {other:?}"))),
        };
        self.size
            .encode(&Value::Number(NumberValue::Unsigned(index as u128)), out)
    }

    fn decode(&self, input: &mut &[u8]) -> CodamaResult<Value> {
        let index = match self.size.decode(input)? {
            Value::Number(n) => n.as_u128() as usize,
            _ => return Err(err("enum discriminator did not decode to a number")),
        };
        let variant = self
            .variants
            .get(index)
            .ok_or_else(|| err(format!("enum index {index} out of range")))?
            .clone();
        Ok(Value::unit_enum(variant))
    }

    fn fixed_size(&self) -> Option<usize> {
        self.size.fixed_size()
    }
}

// --------------------------------------------------------------------------
// Data enum (discriminated union)
// --------------------------------------------------------------------------

pub struct DataEnumCodec {
    pub size: Box<dyn Codec>,
    /// One `(variant name, payload codec)` per variant, in declaration order.
    pub variants: Vec<(String, Box<dyn Codec>)>,
}

impl Codec for DataEnumCodec {
    fn encode(&self, value: &Value, out: &mut Vec<u8>) -> CodamaResult<()> {
        let (variant, fields) = match value {
            Value::Enum { variant, fields } => (variant, fields.as_ref()),
            other => return Err(err(format!("expected an enum variant, got {other:?}"))),
        };
        let index = self
            .variants
            .iter()
            .position(|(name, _)| name == variant)
            .ok_or_else(|| err(format!("unknown enum variant `{variant}`")))?;
        self.size
            .encode(&Value::Number(NumberValue::Unsigned(index as u128)), out)?;
        self.variants[index].1.encode(fields, out)
    }

    fn decode(&self, input: &mut &[u8]) -> CodamaResult<Value> {
        let index = match self.size.decode(input)? {
            Value::Number(n) => n.as_u128() as usize,
            _ => return Err(err("enum discriminator did not decode to a number")),
        };
        let (name, codec) = self
            .variants
            .get(index)
            .ok_or_else(|| err(format!("enum index {index} out of range")))?;
        let fields = codec.decode(input)?;
        Ok(Value::Enum {
            variant: name.clone(),
            fields: Box::new(fields),
        })
    }
}

// --------------------------------------------------------------------------
// Sentinel (terminated by a fixed byte sequence)
// --------------------------------------------------------------------------

pub struct SentinelCodec {
    pub inner: Box<dyn Codec>,
    /// Pre-computed bytes that terminate the encoded value.
    pub sentinel: Vec<u8>,
}

impl Codec for SentinelCodec {
    fn encode(&self, value: &Value, out: &mut Vec<u8>) -> CodamaResult<()> {
        self.inner.encode(value, out)?;
        out.extend_from_slice(&self.sentinel);
        Ok(())
    }

    fn decode(&self, input: &mut &[u8]) -> CodamaResult<Value> {
        let slen = self.sentinel.len();
        let pos = input
            .windows(slen)
            .position(|w| w == self.sentinel.as_slice())
            .ok_or_else(|| err("sentinel not found in input"))?;
        let mut sub = &input[..pos];
        let value = self.inner.decode(&mut sub)?;
        *input = &input[pos + slen..];
        Ok(value)
    }
}

// --------------------------------------------------------------------------
// Hidden prefix / suffix (constant bytes invisible to the caller)
// --------------------------------------------------------------------------

pub struct HiddenPrefixCodec {
    pub inner: Box<dyn Codec>,
    /// Pre-computed constant byte slices prepended on encode, skipped on decode.
    pub constants: Vec<Vec<u8>>,
}

impl Codec for HiddenPrefixCodec {
    fn encode(&self, value: &Value, out: &mut Vec<u8>) -> CodamaResult<()> {
        for c in &self.constants {
            out.extend_from_slice(c);
        }
        self.inner.encode(value, out)
    }

    fn decode(&self, input: &mut &[u8]) -> CodamaResult<Value> {
        for c in &self.constants {
            take(input, c.len())?;
        }
        self.inner.decode(input)
    }

    fn fixed_size(&self) -> Option<usize> {
        let prefix: usize = self.constants.iter().map(|c| c.len()).sum();
        self.inner.fixed_size().map(|s| s + prefix)
    }
}

pub struct HiddenSuffixCodec {
    pub inner: Box<dyn Codec>,
    /// Pre-computed constant byte slices appended on encode, skipped on decode.
    pub constants: Vec<Vec<u8>>,
}

impl Codec for HiddenSuffixCodec {
    fn encode(&self, value: &Value, out: &mut Vec<u8>) -> CodamaResult<()> {
        self.inner.encode(value, out)?;
        for c in &self.constants {
            out.extend_from_slice(c);
        }
        Ok(())
    }

    fn decode(&self, input: &mut &[u8]) -> CodamaResult<Value> {
        let value = self.inner.decode(input)?;
        for c in &self.constants {
            take(input, c.len())?;
        }
        Ok(value)
    }

    fn fixed_size(&self) -> Option<usize> {
        let suffix: usize = self.constants.iter().map(|c| c.len()).sum();
        self.inner.fixed_size().map(|s| s + suffix)
    }
}

// --------------------------------------------------------------------------
// Pre-offset / post-offset (alignment / padding wrappers)
//
// The TS implementations operate on random-access byte arrays. The `Absolute`
// strategy requires knowing the buffer start position, which is not available
// in our streaming (`&mut &[u8]`) model. Only `Padded` and `Relative` (with
// non-negative offsets) are supported here.
// --------------------------------------------------------------------------

pub struct PreOffsetCodec {
    pub inner: Box<dyn Codec>,
    pub offset: i32,
    pub strategy: PreOffsetStrategy,
}

impl Codec for PreOffsetCodec {
    fn encode(&self, value: &Value, out: &mut Vec<u8>) -> CodamaResult<()> {
        match self.strategy {
            PreOffsetStrategy::Padded | PreOffsetStrategy::Relative => {
                if self.offset < 0 {
                    return Err(err("negative pre-offset requires buffer seeking"));
                }
                out.extend(std::iter::repeat_n(0u8, self.offset as usize));
                self.inner.encode(value, out)
            }
            PreOffsetStrategy::Absolute => Err(err("absolute pre-offset requires buffer seeking")),
        }
    }

    fn decode(&self, input: &mut &[u8]) -> CodamaResult<Value> {
        match self.strategy {
            PreOffsetStrategy::Padded | PreOffsetStrategy::Relative => {
                if self.offset < 0 {
                    return Err(err("negative pre-offset requires buffer seeking"));
                }
                take(input, self.offset as usize)?;
                self.inner.decode(input)
            }
            PreOffsetStrategy::Absolute => Err(err("absolute pre-offset requires buffer seeking")),
        }
    }

    fn fixed_size(&self) -> Option<usize> {
        match self.strategy {
            PreOffsetStrategy::Padded | PreOffsetStrategy::Relative if self.offset >= 0 => {
                self.inner.fixed_size().map(|s| s + self.offset as usize)
            }
            _ => None,
        }
    }
}

pub struct PostOffsetCodec {
    pub inner: Box<dyn Codec>,
    pub offset: i32,
    pub strategy: PostOffsetStrategy,
}

impl Codec for PostOffsetCodec {
    fn encode(&self, value: &Value, out: &mut Vec<u8>) -> CodamaResult<()> {
        match self.strategy {
            PostOffsetStrategy::Padded | PostOffsetStrategy::Relative => {
                self.inner.encode(value, out)?;
                if self.offset > 0 {
                    out.extend(std::iter::repeat_n(0u8, self.offset as usize));
                }
                Ok(())
            }
            PostOffsetStrategy::PreOffset => {
                let start = out.len();
                self.inner.encode(value, out)?;
                let inner_written = out.len() - start;
                let target = self.offset.max(0) as usize;
                if inner_written < target {
                    out.extend(std::iter::repeat_n(0u8, target - inner_written));
                }
                Ok(())
            }
            PostOffsetStrategy::Absolute => {
                Err(err("absolute post-offset requires buffer seeking"))
            }
        }
    }

    fn decode(&self, input: &mut &[u8]) -> CodamaResult<Value> {
        match self.strategy {
            PostOffsetStrategy::Padded | PostOffsetStrategy::Relative => {
                let value = self.inner.decode(input)?;
                if self.offset > 0 {
                    take(input, self.offset as usize)?;
                }
                Ok(value)
            }
            PostOffsetStrategy::PreOffset => {
                let inner_size = self.inner.fixed_size().ok_or_else(|| {
                    err("post-offset preOffset strategy requires a fixed-size inner codec")
                })?;
                let value = self.inner.decode(input)?;
                let target = self.offset.max(0) as usize;
                if inner_size < target {
                    take(input, target - inner_size)?;
                }
                Ok(value)
            }
            PostOffsetStrategy::Absolute => {
                Err(err("absolute post-offset requires buffer seeking"))
            }
        }
    }

    fn fixed_size(&self) -> Option<usize> {
        match self.strategy {
            PostOffsetStrategy::Padded | PostOffsetStrategy::Relative if self.offset >= 0 => {
                self.inner.fixed_size().map(|s| s + self.offset as usize)
            }
            PostOffsetStrategy::PreOffset => Some(self.offset.max(0) as usize),
            _ => None,
        }
    }
}

// --------------------------------------------------------------------------
// Zeroable option (None is represented by zero / sentinel bytes; no prefix)
// --------------------------------------------------------------------------

pub struct ZeroableOptionCodec {
    pub item: Box<dyn Codec>,
    /// Bytes representing `None`. If absent, all-zero bytes of item's fixed size.
    pub none_value: Option<Vec<u8>>,
}

impl Codec for ZeroableOptionCodec {
    fn encode(&self, value: &Value, out: &mut Vec<u8>) -> CodamaResult<()> {
        let inner = match value {
            Value::Option(inner) => inner,
            other => return Err(err(format!("expected an option, got {other:?}"))),
        };
        match inner {
            Some(v) => self.item.encode(v, out),
            None => {
                let size = self
                    .item
                    .fixed_size()
                    .ok_or_else(|| err("zeroable option requires a fixed-size item codec"))?;
                match &self.none_value {
                    Some(bytes) => out.extend_from_slice(bytes),
                    None => out.extend(std::iter::repeat_n(0u8, size)),
                }
                Ok(())
            }
        }
    }

    fn decode(&self, input: &mut &[u8]) -> CodamaResult<Value> {
        let size = self
            .item
            .fixed_size()
            .ok_or_else(|| err("zeroable option requires a fixed-size item codec"))?;
        let bytes = take(input, size)?;
        let is_none = match &self.none_value {
            Some(none_bytes) => bytes == none_bytes.as_slice(),
            None => bytes.iter().all(|&b| b == 0),
        };
        if is_none {
            return Ok(Value::Option(None));
        }
        let mut sub = bytes;
        Ok(Value::Option(Some(Box::new(self.item.decode(&mut sub)?))))
    }

    fn fixed_size(&self) -> Option<usize> {
        self.item.fixed_size()
    }
}

// --------------------------------------------------------------------------
// Remainder option (None = empty bytes; Some = encoded item; no prefix)
// --------------------------------------------------------------------------

pub struct RemainderOptionCodec {
    pub item: Box<dyn Codec>,
}

impl Codec for RemainderOptionCodec {
    fn encode(&self, value: &Value, out: &mut Vec<u8>) -> CodamaResult<()> {
        match value {
            Value::Option(Some(v)) => self.item.encode(v, out),
            Value::Option(None) => Ok(()),
            other => Err(err(format!("expected an option, got {other:?}"))),
        }
    }

    fn decode(&self, input: &mut &[u8]) -> CodamaResult<Value> {
        if input.is_empty() {
            Ok(Value::Option(None))
        } else {
            Ok(Value::Option(Some(Box::new(self.item.decode(input)?))))
        }
    }
}

// --------------------------------------------------------------------------
// Map (key-value pairs)
// --------------------------------------------------------------------------

pub struct MapCodec {
    pub key: Box<dyn Codec>,
    pub value: Box<dyn Codec>,
    pub count: CodecCount,
}

impl Codec for MapCodec {
    fn encode(&self, value: &Value, out: &mut Vec<u8>) -> CodamaResult<()> {
        let pairs = match value {
            Value::Map(pairs) => pairs,
            other => return Err(err(format!("expected a map, got {other:?}"))),
        };
        match &self.count {
            CodecCount::Fixed(n) => {
                if pairs.len() != *n {
                    return Err(err(format!(
                        "fixed map expected {n} entries, got {}",
                        pairs.len()
                    )));
                }
            }
            CodecCount::Prefixed(prefix) => {
                prefix.encode(
                    &Value::Number(NumberValue::Unsigned(pairs.len() as u128)),
                    out,
                )?;
            }
            CodecCount::Remainder => {}
        }
        for (k, v) in pairs {
            self.key.encode(k, out)?;
            self.value.encode(v, out)?;
        }
        Ok(())
    }

    fn decode(&self, input: &mut &[u8]) -> CodamaResult<Value> {
        let decode_pair = |input: &mut &[u8]| -> CodamaResult<(Value, Value)> {
            Ok((self.key.decode(input)?, self.value.decode(input)?))
        };
        let mut pairs = Vec::new();
        match &self.count {
            CodecCount::Fixed(n) => {
                for _ in 0..*n {
                    pairs.push(decode_pair(input)?);
                }
            }
            CodecCount::Prefixed(prefix) => {
                let len = match prefix.decode(input)? {
                    Value::Number(n) => n.as_u128() as usize,
                    _ => return Err(err("map prefix did not decode to a number")),
                };
                for _ in 0..len {
                    pairs.push(decode_pair(input)?);
                }
            }
            CodecCount::Remainder => {
                while !input.is_empty() {
                    pairs.push(decode_pair(input)?);
                }
            }
        }
        Ok(Value::Map(pairs))
    }

    fn fixed_size(&self) -> Option<usize> {
        match &self.count {
            CodecCount::Fixed(n) => {
                let entry_size = self.key.fixed_size()? + self.value.fixed_size()?;
                Some(entry_size * n)
            }
            _ => None,
        }
    }
}

// --------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use codama_nodes::{BytesEncoding, NumberFormat::*};

    fn num(format: NumberFormat, endian: Endianness) -> Box<dyn Codec> {
        Box::new(NumberCodec { format, endian })
    }
    fn u8c() -> Box<dyn Codec> {
        num(U8, Endianness::Le)
    }
    fn u32le() -> Box<dyn Codec> {
        num(U32, Endianness::Le)
    }

    /// Asserts the value encodes to `bytes` and decodes back to `decoded`.
    fn check(codec: &dyn Codec, value: Value, bytes: &[u8], decoded: Value) {
        let encoded = codec.encode_to_vec(&value).unwrap();
        assert_eq!(encoded, bytes, "encode mismatch");
        let mut slice = bytes;
        let result = codec.decode(&mut slice).unwrap();
        assert_eq!(result, decoded, "decode mismatch");
        assert!(
            slice.is_empty(),
            "decode left {} trailing bytes",
            slice.len()
        );
    }

    #[test]
    fn numbers() {
        check(&*u8c(), 42u8.into(), &[42], 42u8.into());
        check(
            &*num(U16, Endianness::Le),
            513u16.into(),
            &[1, 2],
            513u16.into(),
        );
        check(
            &*num(U16, Endianness::Be),
            513u16.into(),
            &[2, 1],
            513u16.into(),
        );
        check(&*u32le(), 1u32.into(), &[1, 0, 0, 0], 1u32.into());
        check(
            &*num(U64, Endianness::Le),
            1u64.into(),
            &[1, 0, 0, 0, 0, 0, 0, 0],
            1u64.into(),
        );
        check(
            &*num(I8, Endianness::Le),
            (-1i8).into(),
            &[255],
            (-1i8).into(),
        );
        check(
            &*num(I16, Endianness::Le),
            (-1i16).into(),
            &[255, 255],
            (-1i16).into(),
        );
        check(
            &*num(F32, Endianness::Le),
            1.0f32.into(),
            &[0, 0, 128, 63],
            1.0f32.into(),
        );
    }

    #[test]
    fn short_u16() {
        let c = num(ShortU16, Endianness::Le);
        check(&*c, 0u16.into(), &[0], 0u16.into());
        check(&*c, 127u16.into(), &[127], 127u16.into());
        check(&*c, 128u16.into(), &[0x80, 0x01], 128u16.into());
        check(&*c, 16384u16.into(), &[0x80, 0x80, 0x01], 16384u16.into());
    }

    #[test]
    fn boolean_and_public_key_and_bytes_and_string() {
        let b = BoolCodec { size: u8c() };
        check(&b, true.into(), &[1], true.into());
        check(&b, false.into(), &[0], false.into());

        let key = [7u8; 32];
        check(&PublicKeyCodec, key.into(), &key, key.into());

        check(
            &BytesCodec,
            Value::Bytes(vec![1, 2, 3]),
            &[1, 2, 3],
            Value::Bytes(vec![1, 2, 3]),
        );

        check(
            &StringCodec {
                encoding: BytesEncoding::Utf8,
            },
            "abc".into(),
            &[97, 98, 99],
            "abc".into(),
        );
    }

    #[test]
    fn structs_and_tuples() {
        let s = StructCodec {
            fields: vec![("a".into(), u8c()), ("b".into(), num(U16, Endianness::Le))],
        };
        let value = Value::struct_from([("a".into(), 1u8.into()), ("b".into(), 2u16.into())]);
        check(&s, value.clone(), &[1, 2, 0], value);

        let t = TupleCodec {
            items: vec![u8c(), num(U16, Endianness::Le)],
        };
        let value = Value::Tuple(vec![1u8.into(), 2u16.into()]);
        check(&t, value.clone(), &[1, 2, 0], value);
    }

    #[test]
    fn arrays_with_each_count() {
        let fixed = ArrayCodec {
            item: u8c(),
            count: CodecCount::Fixed(3),
            as_set: false,
        };
        let v = Value::Array(vec![1u8.into(), 2u8.into(), 3u8.into()]);
        check(&fixed, v.clone(), &[1, 2, 3], v);

        let prefixed = ArrayCodec {
            item: u8c(),
            count: CodecCount::Prefixed(u32le()),
            as_set: false,
        };
        let v = Value::Array(vec![1u8.into(), 2u8.into()]);
        check(&prefixed, v.clone(), &[2, 0, 0, 0, 1, 2], v);

        let remainder = ArrayCodec {
            item: u8c(),
            count: CodecCount::Remainder,
            as_set: false,
        };
        let v = Value::Array(vec![1u8.into(), 2u8.into(), 3u8.into()]);
        check(&remainder, v.clone(), &[1, 2, 3], v);
    }

    #[test]
    fn options() {
        let opt = OptionCodec {
            prefix: u8c(),
            item: u8c(),
            fixed: false,
        };
        check(
            &opt,
            Value::Option(Some(Box::new(5u8.into()))),
            &[1, 5],
            Value::Option(Some(Box::new(5u8.into()))),
        );
        check(&opt, Value::Option(None), &[0], Value::Option(None));

        let fixed = OptionCodec {
            prefix: u8c(),
            item: u8c(),
            fixed: true,
        };
        check(&fixed, Value::Option(None), &[0, 0], Value::Option(None));
        check(
            &fixed,
            Value::Option(Some(Box::new(5u8.into()))),
            &[1, 5],
            Value::Option(Some(Box::new(5u8.into()))),
        );
    }

    #[test]
    fn fixed_size_and_size_prefix() {
        let fixed = FixedSizeCodec {
            inner: u8c(),
            size: 4,
        };
        check(&fixed, 7u8.into(), &[7, 0, 0, 0], 7u8.into());

        let padded = FixedSizeCodec {
            inner: Box::new(StringCodec {
                encoding: BytesEncoding::Utf8,
            }),
            size: 5,
        };
        check(&padded, "idl".into(), &[105, 100, 108, 0, 0], "idl".into());

        let prefixed = SizePrefixCodec {
            inner: Box::new(StringCodec {
                encoding: BytesEncoding::Utf8,
            }),
            prefix: u32le(),
        };
        check(&prefixed, "ab".into(), &[2, 0, 0, 0, 97, 98], "ab".into());
    }

    #[test]
    fn scalar_enum() {
        let c = ScalarEnumCodec {
            size: u8c(),
            variants: vec!["A".into(), "B".into(), "C".into()],
        };
        check(&c, Value::unit_enum("B"), &[1], Value::unit_enum("B"));
    }

    #[test]
    fn data_enum() {
        let c = DataEnumCodec {
            size: u8c(),
            variants: vec![
                (
                    "X".into(),
                    Box::new(StructCodec { fields: vec![] }) as Box<dyn Codec>,
                ),
                (
                    "Y".into(),
                    Box::new(StructCodec {
                        fields: vec![("v".into(), u8c())],
                    }),
                ),
            ],
        };
        let y = Value::Enum {
            variant: "Y".into(),
            fields: Box::new(Value::struct_from([("v".into(), 9u8.into())])),
        };
        check(&c, y.clone(), &[1, 9], y);

        let x = Value::Enum {
            variant: "X".into(),
            fields: Box::new(Value::Struct(IndexMap::new())),
        };
        check(&c, x.clone(), &[0], x);
    }

    #[test]
    fn sentinel() {
        let c = SentinelCodec {
            inner: Box::new(BytesCodec),
            sentinel: vec![0xff, 0xfe],
        };
        // encode appends sentinel bytes
        let encoded = c.encode_to_vec(&Value::Bytes(vec![1, 2, 3])).unwrap();
        assert_eq!(encoded, vec![1, 2, 3, 0xff, 0xfe]);
        // decode finds sentinel and strips it
        let mut buf: &[u8] = &[1, 2, 3, 0xff, 0xfe];
        let decoded = c.decode(&mut buf).unwrap();
        assert_eq!(decoded, Value::Bytes(vec![1, 2, 3]));
        assert!(buf.is_empty());
    }

    #[test]
    fn hidden_prefix_and_suffix() {
        let pre = HiddenPrefixCodec {
            inner: u8c(),
            constants: vec![vec![0xAA, 0xBB]],
        };
        // encode prepends constants; decode skips them
        check(&pre, 7u8.into(), &[0xAA, 0xBB, 7], 7u8.into());
        assert_eq!(pre.fixed_size(), Some(3));

        let suf = HiddenSuffixCodec {
            inner: u8c(),
            constants: vec![vec![0xCC]],
        };
        check(&suf, 7u8.into(), &[7, 0xCC], 7u8.into());
        assert_eq!(suf.fixed_size(), Some(2));
    }

    #[test]
    fn pre_post_offset_padded() {
        let pre = PreOffsetCodec {
            inner: u8c(),
            offset: 2,
            strategy: PreOffsetStrategy::Padded,
        };
        check(&pre, 5u8.into(), &[0, 0, 5], 5u8.into());
        assert_eq!(pre.fixed_size(), Some(3));

        let post = PostOffsetCodec {
            inner: u8c(),
            offset: 2,
            strategy: PostOffsetStrategy::Padded,
        };
        check(&post, 5u8.into(), &[5, 0, 0], 5u8.into());
        assert_eq!(post.fixed_size(), Some(3));
    }

    #[test]
    fn zeroable_option() {
        let c = ZeroableOptionCodec {
            item: u8c(),
            none_value: None,
        };
        // Some: encodes item directly
        check(
            &c,
            Value::Option(Some(Box::new(42u8.into()))),
            &[42],
            Value::Option(Some(Box::new(42u8.into()))),
        );
        // None: encodes as zero byte
        check(&c, Value::Option(None), &[0], Value::Option(None));
        assert_eq!(c.fixed_size(), Some(1));
    }

    #[test]
    fn remainder_option() {
        let c = RemainderOptionCodec { item: u8c() };
        // Some: encodes item
        check(
            &c,
            Value::Option(Some(Box::new(7u8.into()))),
            &[7],
            Value::Option(Some(Box::new(7u8.into()))),
        );
        // None: encodes as empty
        let encoded = c.encode_to_vec(&Value::Option(None)).unwrap();
        assert_eq!(encoded, Vec::<u8>::new());
        let mut empty: &[u8] = &[];
        assert_eq!(c.decode(&mut empty).unwrap(), Value::Option(None));
    }

    #[test]
    fn map_codec() {
        let c = MapCodec {
            key: u8c(),
            value: num(U16, Endianness::Le),
            count: CodecCount::Prefixed(u8c()),
        };
        let v = Value::Map(vec![
            (1u8.into(), 256u16.into()),
            (2u8.into(), 512u16.into()),
        ]);
        // prefix=2, then [1, 0,1] then [2, 0,2]
        let bytes = c.encode_to_vec(&v).unwrap();
        assert_eq!(bytes, vec![2, 1, 0, 1, 2, 0, 2]);
        let mut sl: &[u8] = &bytes;
        assert_eq!(c.decode(&mut sl).unwrap(), v);
        assert!(sl.is_empty());
    }
}
