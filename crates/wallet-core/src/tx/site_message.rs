//! What a site may ask a key to sign that is not a transaction: a message, or
//! a Sign In With Solana message this device writes itself.
//!
//! A key signs bytes, and cannot tell what they are for. So bytes that would
//! also be read as a transaction, an offchain message or a sign-in to another
//! site are refused, whatever the site says they are.

use serde::Deserialize;
use solana_transaction::versioned::VersionedTransaction;

use super::decode::parse_wire;

const SIGNATURE_BYTES: usize = 64;
const DIGEST_BYTES: usize = 32;
/// How a message in Solana's offchain format starts.
const OFFCHAIN_DOMAIN: &[u8] = b"\xffsolana offchain";
const SIGN_IN_LINE: &str = " wants you to sign in with your Solana account:\n";

/// Why a message or a sign-in is refused.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SiteMessageError {
    #[error("The message is empty.")]
    Empty,
    #[error(
        "These bytes could be read as a Solana transaction, so signing them could move funds."
    )]
    Transaction,
    #[error("These 32 bytes could be a digest that authorizes a Vector account, so they are not signed as a message.")]
    Digest,
    #[error("These bytes are an offchain message, which Airlock does not sign yet.")]
    Offchain,
    #[error("This message signs you in to {0}, not to the site asking.")]
    OtherSite(String),
    #[error("The site asked to sign in to {asked}, but it is {origin}.")]
    WrongDomain { asked: String, origin: String },
    #[error("The {0} of a sign-in cannot hold a line break.")]
    LineBreak(&'static str),
    #[error("The sign-in has no site address to name.")]
    NoDomain,
}

/// Whether `bytes` may be signed as a message for the site at `origin`.
pub fn check_message(bytes: &[u8], origin: &str) -> Result<(), SiteMessageError> {
    if bytes.is_empty() {
        return Err(SiteMessageError::Empty);
    }
    if bytes.starts_with(OFFCHAIN_DOMAIN) {
        return Err(SiteMessageError::Offchain);
    }
    if reads_as_transaction(bytes) {
        return Err(SiteMessageError::Transaction);
    }
    // A Vector authority signs a bare digest, which looks like nothing else.
    if bytes.len() == DIGEST_BYTES && message_text(bytes).is_none() {
        return Err(SiteMessageError::Digest);
    }
    // A site must not get a sign-in to another site through a plain message.
    if let Some(domain) = std::str::from_utf8(bytes)
        .ok()
        .and_then(|text| text.split_once(SIGN_IN_LINE))
        .map(|(domain, _)| domain)
    {
        if !same_host(domain, host_of(origin)) {
            return Err(SiteMessageError::OtherSite(domain.to_owned()));
        }
    }
    Ok(())
}

/// Whether `bytes` could be the message of a transaction, or a whole one.
/// Bytes after a message that reads are ignored, to be on the safe side.
fn reads_as_transaction(bytes: &[u8]) -> bool {
    if parse_wire(bytes).is_ok() {
        return true;
    }
    let required = match bytes {
        [0x80, count, ..] | [0x81, count, ..] => usize::from(*count),
        [count, ..] if *count < 0x80 => usize::from(*count),
        _ => return false,
    };
    let signatures = vec![0u8; required * SIGNATURE_BYTES];
    // Signatures come after a v1 message, and before any other.
    let wire = if bytes[0] == 0x81 {
        [bytes, &signatures].concat()
    } else {
        let Ok(count) = u8::try_from(required).map(|count| count & 0x7f) else {
            return false;
        };
        [&[count][..], &signatures, bytes].concat()
    };
    wincode::deserialize::<VersionedTransaction>(&wire).is_ok()
}

/// How a message is shown: as text when it reads as text, and bytes otherwise.
/// Control characters could hide what the text says.
pub fn message_text(bytes: &[u8]) -> Option<&str> {
    let text = std::str::from_utf8(bytes).ok()?;
    text.chars()
        .all(|c| !c.is_control() || matches!(c, '\n' | '\r' | '\t'))
        .then_some(text)
}

/// The host of an origin, with its port: `https://a.test:8080` is `a.test:8080`.
pub fn host_of(origin: &str) -> &str {
    let host = origin.split_once("://").map_or(origin, |(_, rest)| rest);
    host.split('/').next().unwrap_or(host)
}

fn same_host(left: &str, right: &str) -> bool {
    left.eq_ignore_ascii_case(right)
}

/// What a site asks to sign in with. The names are the Wallet Standard's.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SignIn {
    pub domain: Option<String>,
    pub address: Option<String>,
    pub statement: Option<String>,
    pub uri: Option<String>,
    pub version: Option<String>,
    pub chain_id: Option<String>,
    pub nonce: Option<String>,
    pub issued_at: Option<String>,
    pub expiration_time: Option<String>,
    pub not_before: Option<String>,
    pub request_id: Option<String>,
    pub resources: Option<Vec<String>>,
}

impl SignIn {
    /// Refuse a sign-in that names another site, or whose fields could write
    /// lines of their own into the message.
    pub fn check(&self, origin: &str) -> Result<(), SiteMessageError> {
        let host = host_of(origin);
        if host.is_empty() {
            return Err(SiteMessageError::NoDomain);
        }
        if let Some(asked) = &self.domain {
            if !same_host(asked, host) {
                return Err(SiteMessageError::WrongDomain {
                    asked: asked.clone(),
                    origin: host.to_owned(),
                });
            }
        }
        let lines = [
            ("domain", &self.domain),
            ("address", &self.address),
            ("statement", &self.statement),
            ("URI", &self.uri),
            ("version", &self.version),
            ("chain ID", &self.chain_id),
            ("nonce", &self.nonce),
            ("issued time", &self.issued_at),
            ("expiration time", &self.expiration_time),
            ("not-before time", &self.not_before),
            ("request ID", &self.request_id),
        ];
        for (name, value) in lines {
            if value.as_deref().is_some_and(has_line_break) {
                return Err(SiteMessageError::LineBreak(name));
            }
        }
        if self
            .resources
            .iter()
            .flatten()
            .any(|resource| has_line_break(resource))
        {
            return Err(SiteMessageError::LineBreak("resources"));
        }
        Ok(())
    }

    /// Whether its URI points somewhere other than the site asking. The
    /// standard allows that, so it is said, not refused.
    pub fn uri_elsewhere(&self, origin: &str) -> bool {
        self.uri
            .as_deref()
            .is_some_and(|uri| !same_host(host_of(uri), host_of(origin)))
    }

    /// The message, as `@solana/wallet-standard-util`'s `createSignInMessageText`
    /// writes it, so a site's own check of it passes.
    pub fn message(&self, origin: &str, address: &str) -> String {
        let domain = self.domain.as_deref().unwrap_or_else(|| host_of(origin));
        let mut message = format!("{domain}{SIGN_IN_LINE}{address}");
        if let Some(statement) = filled(&self.statement) {
            message.push_str("\n\n");
            message.push_str(statement);
        }
        let mut fields: Vec<String> = [
            ("URI", &self.uri),
            ("Version", &self.version),
            ("Chain ID", &self.chain_id),
            ("Nonce", &self.nonce),
            ("Issued At", &self.issued_at),
            ("Expiration Time", &self.expiration_time),
            ("Not Before", &self.not_before),
            ("Request ID", &self.request_id),
        ]
        .into_iter()
        .filter_map(|(name, value)| filled(value).map(|value| format!("{name}: {value}")))
        .collect();
        if let Some(resources) = &self.resources {
            fields.push("Resources:".to_owned());
            fields.extend(resources.iter().map(|resource| format!("- {resource}")));
        }
        if !fields.is_empty() {
            message.push_str("\n\n");
            message.push_str(&fields.join("\n"));
        }
        message
    }
}

fn has_line_break(text: &str) -> bool {
    text.contains(['\n', '\r'])
}

fn filled(value: &Option<String>) -> Option<&str> {
    value.as_deref().filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use solana_message::{Message, VersionedMessage};
    use solana_pubkey::Pubkey;
    use solana_signature::Signature;

    const ORIGIN: &str = "https://app.example:8080";

    fn legacy_message() -> Message {
        let payer = Pubkey::new_from_array([1; 32]);
        let mut message = Message::new(
            &[solana_system_interface::instruction::transfer(
                &payer,
                &Pubkey::new_from_array([2; 32]),
                5,
            )],
            Some(&payer),
        );
        message.recent_blockhash = solana_hash::Hash::new_from_array([3; 32]);
        message
    }

    #[test]
    fn a_legacy_transaction_message_is_refused() {
        let bytes = VersionedMessage::Legacy(legacy_message()).serialize();
        assert_eq!(
            check_message(&bytes, ORIGIN),
            Err(SiteMessageError::Transaction)
        );
    }

    #[test]
    fn a_v0_transaction_message_is_refused() {
        let legacy = legacy_message();
        let v0 = solana_message::v0::Message {
            header: legacy.header,
            account_keys: legacy.account_keys.clone(),
            recent_blockhash: legacy.recent_blockhash,
            instructions: legacy.instructions.clone(),
            address_table_lookups: vec![],
        };
        let bytes = VersionedMessage::V0(v0).serialize();
        assert_eq!(bytes[0], 0x80);
        assert_eq!(
            check_message(&bytes, ORIGIN),
            Err(SiteMessageError::Transaction)
        );
    }

    #[test]
    fn a_v1_transaction_message_is_refused() {
        let payer = Pubkey::new_from_array([1; 32]);
        let wire = crate::compile_v1(
            payer,
            vec![solana_system_interface::instruction::transfer(
                &payer,
                &Pubkey::new_from_array([2; 32]),
                5,
            )],
            None,
            Some(solana_hash::Hash::new_from_array([3; 32])),
        )
        .unwrap()
        .wire;
        // The message is the wire without the signature after it.
        let message = &wire[..wire.len() - SIGNATURE_BYTES];
        assert_eq!(
            check_message(message, ORIGIN),
            Err(SiteMessageError::Transaction)
        );
        assert_eq!(
            check_message(&wire, ORIGIN),
            Err(SiteMessageError::Transaction)
        );
    }

    #[test]
    fn a_whole_legacy_transaction_is_refused() {
        let tx = VersionedTransaction {
            signatures: vec![Signature::default()],
            message: VersionedMessage::Legacy(legacy_message()),
        };
        let wire = wincode::serialize(&tx).unwrap();
        assert_eq!(
            check_message(&wire, ORIGIN),
            Err(SiteMessageError::Transaction)
        );
    }

    #[test]
    fn a_transaction_message_with_bytes_after_it_is_refused() {
        let mut bytes = VersionedMessage::Legacy(legacy_message()).serialize();
        bytes.extend_from_slice(b"trailing");
        assert_eq!(
            check_message(&bytes, ORIGIN),
            Err(SiteMessageError::Transaction)
        );
    }

    #[test]
    fn plain_text_and_bytes_may_be_signed() {
        assert_eq!(check_message(b"Hello from a site", ORIGIN), Ok(()));
        assert_eq!(check_message(&[0x00, 0x01, 0x02, 0xfe], ORIGIN), Ok(()));
        assert_eq!(check_message(&[0x80], ORIGIN), Ok(()));
        assert_eq!(check_message(&[], ORIGIN), Err(SiteMessageError::Empty));
    }

    #[test]
    fn a_bare_digest_is_refused_and_its_text_form_is_not() {
        assert_eq!(
            check_message(&[0xfe; 32], ORIGIN),
            Err(SiteMessageError::Digest)
        );
        assert_eq!(check_message(&[0xfe; 31], ORIGIN), Ok(()));
        assert_eq!(check_message(&[b'a'; 32], ORIGIN), Ok(()));
    }

    #[test]
    fn an_offchain_message_is_refused() {
        let mut bytes = b"\xffsolana offchain".to_vec();
        bytes.extend_from_slice(&[0, 0, 0]);
        assert_eq!(
            check_message(&bytes, ORIGIN),
            Err(SiteMessageError::Offchain)
        );
    }

    #[test]
    fn a_message_cannot_sign_in_to_another_site() {
        let other = format!("bank.example{SIGN_IN_LINE}abc");
        assert_eq!(
            check_message(other.as_bytes(), ORIGIN),
            Err(SiteMessageError::OtherSite("bank.example".into()))
        );
        let own = format!("app.example:8080{SIGN_IN_LINE}abc");
        assert_eq!(check_message(own.as_bytes(), ORIGIN), Ok(()));
    }

    #[test]
    fn control_characters_show_as_bytes() {
        assert_eq!(
            message_text(b"line one\nline two"),
            Some("line one\nline two")
        );
        assert_eq!(message_text(b"hidden\x1b[2Jtext"), None);
        assert_eq!(message_text(&[0xff, 0xfe]), None);
    }

    #[test]
    fn the_host_keeps_its_port() {
        assert_eq!(host_of("https://a.test:8080"), "a.test:8080");
        assert_eq!(host_of("https://a.test/path"), "a.test");
        assert_eq!(host_of("a.test"), "a.test");
    }

    #[test]
    fn a_sign_in_to_another_domain_is_refused() {
        let asked = SignIn {
            domain: Some("bank.example".into()),
            ..SignIn::default()
        };
        assert!(matches!(
            asked.check(ORIGIN),
            Err(SiteMessageError::WrongDomain { .. })
        ));
        let own = SignIn {
            domain: Some("APP.example:8080".into()),
            ..SignIn::default()
        };
        assert_eq!(own.check(ORIGIN), Ok(()));
        assert_eq!(SignIn::default().check(ORIGIN), Ok(()));
    }

    #[test]
    fn a_field_cannot_write_lines_of_its_own() {
        let statement = SignIn {
            statement: Some("Hello\nURI: https://bank.example".into()),
            ..SignIn::default()
        };
        assert_eq!(
            statement.check(ORIGIN),
            Err(SiteMessageError::LineBreak("statement"))
        );
        let resource = SignIn {
            resources: Some(vec!["a\n- b".into()]),
            ..SignIn::default()
        };
        assert_eq!(
            resource.check(ORIGIN),
            Err(SiteMessageError::LineBreak("resources"))
        );
    }

    #[test]
    fn the_message_is_written_as_the_standard_does() {
        let full = SignIn {
            domain: Some("app.example:8080".into()),
            statement: Some("Welcome".into()),
            uri: Some("https://app.example:8080/login".into()),
            version: Some("1".into()),
            chain_id: Some("mainnet".into()),
            nonce: Some("abc123".into()),
            issued_at: Some("2026-10-30T00:00:00Z".into()),
            expiration_time: Some("2026-10-31T00:00:00Z".into()),
            not_before: Some("2026-10-30T00:00:00Z".into()),
            request_id: Some("r1".into()),
            resources: Some(vec!["https://a".into(), "https://b".into()]),
            ..SignIn::default()
        };
        assert_eq!(
            full.message(ORIGIN, "ADDR"),
            "app.example:8080 wants you to sign in with your Solana account:\nADDR\n\nWelcome\n\nURI: https://app.example:8080/login\nVersion: 1\nChain ID: mainnet\nNonce: abc123\nIssued At: 2026-10-30T00:00:00Z\nExpiration Time: 2026-10-31T00:00:00Z\nNot Before: 2026-10-30T00:00:00Z\nRequest ID: r1\nResources:\n- https://a\n- https://b"
        );
    }

    #[test]
    fn a_bare_sign_in_names_the_site_and_the_account() {
        assert_eq!(
            SignIn::default().message(ORIGIN, "ADDR"),
            "app.example:8080 wants you to sign in with your Solana account:\nADDR"
        );
    }

    #[test]
    fn a_uri_elsewhere_is_noticed() {
        let elsewhere = SignIn {
            uri: Some("https://other.example/login".into()),
            ..SignIn::default()
        };
        assert!(elsewhere.uri_elsewhere(ORIGIN));
        let own = SignIn {
            uri: Some("https://app.example:8080/login".into()),
            ..SignIn::default()
        };
        assert!(!own.uri_elsewhere(ORIGIN));
    }

    #[test]
    fn unknown_fields_of_a_sign_in_are_ignored() {
        let parsed: SignIn =
            serde_json::from_str(r#"{"chainId":"mainnet","issuedAt":"t","other":1}"#).unwrap();
        assert_eq!(parsed.chain_id.as_deref(), Some("mainnet"));
        assert_eq!(parsed.issued_at.as_deref(), Some("t"));
    }
}
