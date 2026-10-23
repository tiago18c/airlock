use indexmap::IndexMap;

/// A dynamic value: the currency that codecs encode from and decode into.
///
/// This is the Rust counterpart of the plain JavaScript values the TypeScript
/// dynamic codecs operate on. A custom enum (rather than `serde_json::Value`)
/// is used so that integer widths, public keys and byte buffers are represented
/// losslessly and struct field order is preserved.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// An integer or floating-point number, tagged by signedness/precision.
    Number(NumberValue),
    Bool(bool),
    String(String),
    /// A raw byte buffer.
    Bytes(Vec<u8>),
    /// A 32-byte public key.
    PublicKey([u8; 32]),
    Array(Vec<Value>),
    Set(Vec<Value>),
    Tuple(Vec<Value>),
    Map(Vec<(Value, Value)>),
    /// An ordered struct. Field order matters for encoding.
    Struct(IndexMap<String, Value>),
    /// An enum variant. `fields` is `Struct` for empty/struct variants and
    /// `Struct({"fields": Tuple([...])})` for tuple variants, mirroring how the
    /// TypeScript codecs treat data enums (`{ __kind, ...fields }`).
    Enum {
        variant: String,
        fields: Box<Value>,
    },
    /// An optional value (`OptionTypeNode`, `RemainderOptionTypeNode`, ...).
    Option(Option<Box<Value>>),
}

/// A numeric value, preserving signedness and precision so it can be encoded
/// against any [`codama_nodes::NumberFormat`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum NumberValue {
    Unsigned(u128),
    Signed(i128),
    Float(f64),
}

impl Value {
    /// Convenience constructor for an empty enum variant.
    pub fn unit_enum(variant: impl Into<String>) -> Self {
        Value::Enum {
            variant: variant.into(),
            fields: Box::new(Value::Struct(IndexMap::new())),
        }
    }

    /// Builds a struct value from name/value pairs, preserving order.
    pub fn struct_from(fields: impl IntoIterator<Item = (String, Value)>) -> Self {
        Value::Struct(fields.into_iter().collect())
    }
}

macro_rules! impl_from_unsigned {
    ($($t:ty),*) => {$(
        impl From<$t> for Value {
            fn from(n: $t) -> Self { Value::Number(NumberValue::Unsigned(n as u128)) }
        }
    )*};
}
macro_rules! impl_from_signed {
    ($($t:ty),*) => {$(
        impl From<$t> for Value {
            fn from(n: $t) -> Self { Value::Number(NumberValue::Signed(n as i128)) }
        }
    )*};
}
impl_from_unsigned!(u8, u16, u32, u64, u128);
impl_from_signed!(i8, i16, i32, i64, i128);

impl From<f32> for Value {
    fn from(n: f32) -> Self {
        Value::Number(NumberValue::Float(n as f64))
    }
}
impl From<f64> for Value {
    fn from(n: f64) -> Self {
        Value::Number(NumberValue::Float(n))
    }
}
impl From<bool> for Value {
    fn from(b: bool) -> Self {
        Value::Bool(b)
    }
}
impl From<String> for Value {
    fn from(s: String) -> Self {
        Value::String(s)
    }
}
impl From<&str> for Value {
    fn from(s: &str) -> Self {
        Value::String(s.to_string())
    }
}
impl From<Vec<u8>> for Value {
    fn from(b: Vec<u8>) -> Self {
        Value::Bytes(b)
    }
}
impl From<[u8; 32]> for Value {
    fn from(k: [u8; 32]) -> Self {
        Value::PublicKey(k)
    }
}

impl NumberValue {
    /// Reinterprets the number as `u128` for unsigned-integer encoding.
    pub fn as_u128(&self) -> u128 {
        match self {
            NumberValue::Unsigned(n) => *n,
            NumberValue::Signed(n) => *n as u128,
            NumberValue::Float(n) => *n as u128,
        }
    }

    /// Reinterprets the number as `i128` for signed-integer encoding.
    pub fn as_i128(&self) -> i128 {
        match self {
            NumberValue::Unsigned(n) => *n as i128,
            NumberValue::Signed(n) => *n,
            NumberValue::Float(n) => *n as i128,
        }
    }

    /// Reinterprets the number as `f64` for float encoding.
    pub fn as_f64(&self) -> f64 {
        match self {
            NumberValue::Unsigned(n) => *n as f64,
            NumberValue::Signed(n) => *n as f64,
            NumberValue::Float(n) => *n,
        }
    }
}
