//! Chrome native-messaging frames: a 32-bit little-endian length, then JSON bytes.
//!
//! Messages written back to the browser must stay within 1 MiB. The host also
//! refuses larger incoming messages so a broken peer cannot force a huge allocation.

use std::io::{self, Read, Write};

pub const MAX_MESSAGE: usize = 1024 * 1024;

pub fn read_frame(input: &mut impl Read) -> io::Result<Option<Vec<u8>>> {
    let mut len_buf = [0u8; 4];
    match input.read_exact(&mut len_buf) {
        Ok(()) => {}
        Err(err) if err.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(err) => return Err(err),
    }
    let len = u32::from_le_bytes(len_buf) as usize;
    if len == 0 || len > MAX_MESSAGE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("native message length {len} is outside 1..={MAX_MESSAGE}"),
        ));
    }
    let mut body = vec![0u8; len];
    input.read_exact(&mut body)?;
    Ok(Some(body))
}

pub fn write_frame(output: &mut impl Write, body: &[u8]) -> io::Result<()> {
    if body.len() > MAX_MESSAGE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "native message is {} bytes; max is {MAX_MESSAGE}",
                body.len()
            ),
        ));
    }
    let len = u32::try_from(body.len()).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "native message length does not fit",
        )
    })?;
    output.write_all(&len.to_le_bytes())?;
    output.write_all(body)?;
    output.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let mut buf = Vec::new();
        write_frame(&mut buf, br#"{"v":1}"#).unwrap();
        let mut cursor = &buf[..];
        assert_eq!(read_frame(&mut cursor).unwrap().unwrap(), br#"{"v":1}"#);
        assert!(read_frame(&mut cursor).unwrap().is_none());
    }

    #[test]
    fn rejects_an_empty_frame() {
        let mut cursor = &0u32.to_le_bytes()[..];
        assert!(read_frame(&mut cursor).is_err());
    }
}
