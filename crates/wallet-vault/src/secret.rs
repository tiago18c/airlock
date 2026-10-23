//! Secrets as they are held in memory: wiped when dropped, and never printed.

use base64::{engine::general_purpose::STANDARD, Engine};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

/// Secret bytes. Written as base64, and the text is wiped once read.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct SecretBytes(pub Vec<u8>);

impl Serialize for SecretBytes {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let text = Zeroizing::new(STANDARD.encode(&self.0));
        serializer.serialize_str(&text)
    }
}

impl<'de> Deserialize<'de> for SecretBytes {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = Zeroizing::new(String::deserialize(deserializer)?);
        STANDARD
            .decode(text.as_bytes())
            .map(Self)
            .map_err(|_| serde::de::Error::custom("secret bytes are not base64"))
    }
}

impl PartialEq for SecretBytes {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}
impl Eq for SecretBytes {}

impl std::fmt::Debug for SecretBytes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SecretBytes([redacted; {} bytes])", self.0.len())
    }
}

#[derive(Clone, Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
pub struct SecretString(pub String);

impl PartialEq for SecretString {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}
impl Eq for SecretString {}

impl std::fmt::Debug for SecretString {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SecretString([redacted])")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_does_not_print_secrets() {
        let bytes = SecretBytes(vec![0x11, 0x22, 0xab]);
        let shown = format!("{bytes:?}");
        assert!(shown.contains("redacted"));
        assert!(!shown.contains("11"));
        assert!(!shown.contains("ab"));

        let text = SecretString("correct horse".into());
        let shown = format!("{text:?}");
        assert_eq!(shown, "SecretString([redacted])");
        assert!(!shown.contains("horse"));
    }

    #[test]
    fn secret_bytes_are_written_as_base64() {
        let bytes = SecretBytes(vec![0x11, 0x22, 0xab]);
        let json = serde_json::to_string(&bytes).unwrap();
        assert_eq!(json, "\"ESKr\"");
        assert_eq!(serde_json::from_str::<SecretBytes>(&json).unwrap(), bytes);
        assert!(serde_json::from_str::<SecretBytes>("[17,34,171]").is_err());
    }
}
