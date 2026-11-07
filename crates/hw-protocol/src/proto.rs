//! The part of protobuf (proto2) Trezor's messages use: varints and
//! length-delimited fields. Repeated scalars are written unpacked, one
//! field each, as proto2 does by default. Unknown fields are skipped when
//! reading.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProtoError;

#[derive(Default)]
pub(crate) struct Writer {
    out: Vec<u8>,
}

fn varint(out: &mut Vec<u8>, mut value: u64) {
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

impl Writer {
    pub(crate) fn uint(&mut self, field: u32, value: u64) -> &mut Self {
        varint(&mut self.out, u64::from(field) << 3);
        varint(&mut self.out, value);
        self
    }

    pub(crate) fn bool(&mut self, field: u32, value: bool) -> &mut Self {
        self.uint(field, u64::from(value))
    }

    pub(crate) fn bytes(&mut self, field: u32, value: &[u8]) -> &mut Self {
        varint(&mut self.out, (u64::from(field) << 3) | 2);
        varint(&mut self.out, value.len() as u64);
        self.out.extend_from_slice(value);
        self
    }

    pub(crate) fn uints(&mut self, field: u32, values: &[u32]) -> &mut Self {
        for value in values {
            self.uint(field, u64::from(*value));
        }
        self
    }

    pub(crate) fn finish(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.out)
    }
}

/// One field of a message as read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Field<'a> {
    Varint(u64),
    Bytes(&'a [u8]),
}

/// The fields of `message`, in order.
pub(crate) fn fields(message: &[u8]) -> Result<Vec<(u32, Field<'_>)>, ProtoError> {
    let mut at = 0;
    let read_varint = |at: &mut usize| -> Result<u64, ProtoError> {
        let mut value = 0u64;
        for shift in (0..64).step_by(7) {
            let byte = *message.get(*at).ok_or(ProtoError)?;
            *at += 1;
            value |= u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return Ok(value);
            }
        }
        Err(ProtoError)
    };
    let mut out = Vec::new();
    while at < message.len() {
        let key = read_varint(&mut at)?;
        let field = u32::try_from(key >> 3).map_err(|_| ProtoError)?;
        match key & 7 {
            0 => out.push((field, Field::Varint(read_varint(&mut at)?))),
            2 => {
                let len = usize::try_from(read_varint(&mut at)?).map_err(|_| ProtoError)?;
                let end = at.checked_add(len).ok_or(ProtoError)?;
                out.push((field, Field::Bytes(message.get(at..end).ok_or(ProtoError)?)));
                at = end;
            }
            // Fixed-width fields: none of Trezor's messages here use them,
            // but skipping them keeps a newer firmware's reply readable.
            1 => {
                at = at
                    .checked_add(8)
                    .filter(|&end| end <= message.len())
                    .ok_or(ProtoError)?
            }
            5 => {
                at = at
                    .checked_add(4)
                    .filter(|&end| end <= message.len())
                    .ok_or(ProtoError)?
            }
            _ => return Err(ProtoError),
        }
    }
    Ok(out)
}

/// Lookups over the fields of one message.
pub(crate) struct Read<'a>(pub(crate) Vec<(u32, Field<'a>)>);

impl<'a> Read<'a> {
    pub(crate) fn new(message: &'a [u8]) -> Result<Self, ProtoError> {
        fields(message).map(Self)
    }

    pub(crate) fn uint(&self, field: u32) -> Option<u64> {
        self.0.iter().rev().find_map(|(number, value)| match value {
            Field::Varint(n) if *number == field => Some(*n),
            _ => None,
        })
    }

    pub(crate) fn bool(&self, field: u32) -> Option<bool> {
        self.uint(field).map(|n| n != 0)
    }

    pub(crate) fn bytes(&self, field: u32) -> Option<&'a [u8]> {
        self.0.iter().rev().find_map(|(number, value)| match value {
            Field::Bytes(bytes) if *number == field => Some(*bytes),
            _ => None,
        })
    }

    pub(crate) fn text(&self, field: u32) -> Option<String> {
        self.bytes(field)
            .and_then(|bytes| String::from_utf8(bytes.to_vec()).ok())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fields_round_trip() {
        let message = Writer::default()
            .uints(1, &[0x8000_002c, 0x8000_01f5])
            .bool(2, true)
            .bytes(3, b"tx")
            .finish();
        assert_eq!(
            message,
            [
                0x08, 0xac, 0x80, 0x80, 0x80, 0x08, 0x08, 0xf5, 0x83, 0x80, 0x80, 0x08, 0x10, 0x01,
                0x1a, 0x02, b't', b'x'
            ]
        );
        let read = Read::new(&message).unwrap();
        assert_eq!(read.bool(2), Some(true));
        assert_eq!(read.bytes(3), Some(&b"tx"[..]));
        assert_eq!(read.uint(9), None);
    }

    #[test]
    fn a_truncated_message_is_refused() {
        assert_eq!(fields(&[0x1a, 0x05, 1]), Err(ProtoError));
        assert_eq!(fields(&[0x08, 0x80]), Err(ProtoError));
        assert_eq!(fields(&[0x0b]), Err(ProtoError));
    }
}
