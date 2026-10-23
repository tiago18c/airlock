//! Chrome extension ids are the first 16 bytes of SHA-256(SPKI DER), with each
//! nibble mapped to `a`–`p`. The public key lives in the extension manifest.

use base64::Engine;
use sha2::{Digest, Sha256};

const MANIFEST: &str = include_str!("../../../extension/manifest.json");

pub const HOST_NAME: &str = "dev.airlock.bridge";
pub const GECKO_ID: &str = "bridge@airlock.dev";

pub fn chrome_extension_id(spki_der: &[u8]) -> String {
    let digest = Sha256::digest(spki_der);
    let mut id = String::with_capacity(32);
    for byte in digest.iter().take(16) {
        id.push(nibble(byte >> 4));
        id.push(nibble(byte & 0x0f));
    }
    id
}

fn nibble(value: u8) -> char {
    char::from(b'a' + value)
}

pub fn extension_manifest() -> serde_json::Value {
    serde_json::from_str(MANIFEST).expect("extension/manifest.json is valid JSON")
}

pub fn pinned_chrome_extension_id() -> String {
    let key = extension_manifest()
        .get("key")
        .and_then(|value| value.as_str())
        .expect("extension manifest has a key")
        .to_owned();
    let der = base64::engine::general_purpose::STANDARD
        .decode(key.trim())
        .expect("extension key is standard base64");
    chrome_extension_id(&der)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn development_extension_id_is_stable() {
        assert_eq!(
            pinned_chrome_extension_id(),
            "mmbfhgibjpbgmhjfhfhlejdpopgapphp"
        );
        assert_eq!(
            extension_manifest()["browser_specific_settings"]["gecko"]["id"],
            GECKO_ID
        );
    }
}
