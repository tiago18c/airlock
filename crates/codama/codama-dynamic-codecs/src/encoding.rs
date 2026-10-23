use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use codama_nodes::BytesEncoding;
use codama_visitors_core::{DynamicError as CodamaError, DynamicResult as CodamaResult};

/// Decodes a string into bytes according to a [`BytesEncoding`].
pub fn decode_bytes(encoding: BytesEncoding, data: &str) -> CodamaResult<Vec<u8>> {
    match encoding {
        BytesEncoding::Utf8 => Ok(data.as_bytes().to_vec()),
        BytesEncoding::Base16 => {
            hex::decode(data).map_err(|e| CodamaError::InvalidBytesEncoding(e.to_string()))
        }
        BytesEncoding::Base58 => bs58::decode(data)
            .into_vec()
            .map_err(|e| CodamaError::InvalidBytesEncoding(e.to_string())),
        BytesEncoding::Base64 => BASE64
            .decode(data)
            .map_err(|e| CodamaError::InvalidBytesEncoding(e.to_string())),
    }
}

/// Encodes bytes into a string according to a [`BytesEncoding`].
pub fn encode_bytes(encoding: BytesEncoding, bytes: &[u8]) -> CodamaResult<String> {
    match encoding {
        BytesEncoding::Utf8 => String::from_utf8(bytes.to_vec())
            .map_err(|e| CodamaError::InvalidBytesEncoding(e.to_string())),
        BytesEncoding::Base16 => Ok(hex::encode(bytes)),
        BytesEncoding::Base58 => Ok(bs58::encode(bytes).into_string()),
        BytesEncoding::Base64 => Ok(BASE64.encode(bytes)),
    }
}
