//! What a site may ask a key to sign that is not a transaction: a message, or
//! a Sign In With Solana message this device writes itself.
//!
//! A key signs bytes, and cannot tell what they are for. So bytes that would
//! also be read as a transaction, an offchain message or a sign-in to another
//! site are refused, whatever the site says they are.

use serde::Deserialize;
use solana_pubkey::Pubkey;
use solana_transaction::versioned::VersionedTransaction;

use super::decode::parse_wire;

const SIGNATURE_BYTES: usize = 64;
const DIGEST_BYTES: usize = 32;
/// How a message in Solana's offchain format starts.
const OFFCHAIN_DOMAIN: &[u8] = b"\xffsolana offchain";
/// The version of the offchain format a Ledger and a Trezor sign.
const OFFCHAIN_V1: u8 = 1;
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
    #[error("These 32 bytes could be a digest that authorizes a Vector account, so they are not signed as a message. A nonce of this length can be signed as 32 hex digits.")]
    Digest,
    #[error("These bytes are an offchain message, which Airlock does not sign yet.")]
    Offchain,
    #[error("This message signs you in to {0}, not to the site asking.")]
    OtherSite(String),
    #[error("The site asked to sign in to {asked}, but it is {origin}.")]
    WrongDomain { asked: String, origin: String },
    #[error("The {0} of a sign-in cannot hold a line break.")]
    LineBreak(&'static str),
    #[error("The {0} of a sign-in holds characters that would hide what it says.")]
    HiddenText(&'static str),
    #[error("The sign-in has no site address to name.")]
    NoDomain,
    #[error("A {0} signs only text messages, and this message is not text.")]
    NotText(&'static str),
    #[error(
        "This offchain message is not version 1 for this account alone, so a {0} does not sign it."
    )]
    OtherOffchain(&'static str),
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
    // Reading as text proves nothing: whoever builds the transaction can vary
    // it until its digest does, in about 2^36 tries. A digest that is all hex
    // digits takes about 2^113, and that is how a nonce is often written.
    if bytes.len() == DIGEST_BYTES && !bytes.iter().all(u8::is_ascii_hexdigit) {
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
pub fn message_text(bytes: &[u8]) -> Option<&str> {
    let text = std::str::from_utf8(bytes).ok()?;
    (!text.chars().any(hides_text)).then_some(text)
}

/// Whether `c` could hide what text says, or put it in another order than it
/// is signed in: a control character, or a format character that draws
/// nothing. A line break and a tab are plain to see, and the joiner inside an
/// emoji changes no meaning.
fn hides_text(c: char) -> bool {
    if matches!(c, '\n' | '\r' | '\t') {
        return false;
    }
    c.is_control()
        || matches!(
            c,
            '\u{ad}'
                | '\u{34f}'
                | '\u{61c}'
                | '\u{115f}'..='\u{1160}'
                | '\u{17b4}'..='\u{17b5}'
                | '\u{180b}'..='\u{180f}'
                | '\u{200b}'..='\u{200c}'
                | '\u{200e}'..='\u{200f}'
                | '\u{2028}'..='\u{202e}'
                | '\u{2060}'..='\u{206f}'
                | '\u{3164}'
                | '\u{feff}'
                | '\u{ffa0}'
                | '\u{fff9}'..='\u{fffb}'
                | '\u{e0000}'..='\u{e0fff}'
        )
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
            if let Some(value) = value {
                check_field(name, value)?;
            }
        }
        for resource in self.resources.iter().flatten() {
            check_field("resources", resource)?;
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

/// Refuse a field that could write lines of its own into the message, or hide
/// what it says. The line separators of Unicode draw a line break too.
fn check_field(name: &'static str, text: &str) -> Result<(), SiteMessageError> {
    if text.contains(['\n', '\r', '\u{85}', '\u{2028}', '\u{2029}']) {
        return Err(SiteMessageError::LineBreak(name));
    }
    if text.chars().any(hides_text) {
        return Err(SiteMessageError::HiddenText(name));
    }
    Ok(())
}

fn filled(value: &Option<String>) -> Option<&str> {
    value.as_deref().filter(|value| !value.is_empty())
}

/// What a Ledger or a Trezor signs for a site's message, `bytes`: Solana's
/// offchain message, version 1, naming `account` as its one signer, with the
/// site's text after it. The site gets these bytes back as `signedMessage`,
/// as the Wallet Standard allows. `device` names the vendor in a refusal.
///
/// A site that sends a version 1 offchain message for this account alone
/// gets it signed as it is. Whatever [`check_message`] refuses is refused
/// here too, and so is a message that is not text: those devices sign only
/// text.
pub fn device_message(
    bytes: &[u8],
    origin: &str,
    account: &Pubkey,
    device: &'static str,
) -> Result<Vec<u8>, SiteMessageError> {
    if bytes.starts_with(OFFCHAIN_DOMAIN) {
        let (signers, text) =
            read_offchain_v1(bytes).ok_or(SiteMessageError::OtherOffchain(device))?;
        if signers != [*account] {
            return Err(SiteMessageError::OtherOffchain(device));
        }
        check_device_text(text, origin, device)?;
        return Ok(bytes.to_vec());
    }
    check_device_text(bytes, origin, device)?;
    Ok(offchain_v1(&[*account], bytes))
}

fn check_device_text(
    text: &[u8],
    origin: &str,
    device: &'static str,
) -> Result<(), SiteMessageError> {
    check_message(text, origin)?;
    if message_text(text).is_none() {
        return Err(SiteMessageError::NotText(device));
    }
    Ok(())
}

/// An offchain message, version 1: the domain, the version, the signers in
/// ascending order with no repeats, then `text` with no length before it.
pub fn offchain_v1(signers: &[Pubkey], text: &[u8]) -> Vec<u8> {
    let mut signers = signers.to_vec();
    signers.sort();
    signers.dedup();
    let mut out = OFFCHAIN_DOMAIN.to_vec();
    out.push(OFFCHAIN_V1);
    out.push(u8::try_from(signers.len()).unwrap_or(u8::MAX));
    for signer in signers.iter().take(usize::from(u8::MAX)) {
        out.extend_from_slice(signer.as_ref());
    }
    out.extend_from_slice(text);
    out
}

/// The signers and the text of a version 1 offchain message, or `None`
/// when `bytes` is not one: another version, no signer, signers out of
/// order or repeated, or no text.
pub fn read_offchain_v1(bytes: &[u8]) -> Option<(Vec<Pubkey>, &[u8])> {
    let rest = bytes.strip_prefix(OFFCHAIN_DOMAIN)?;
    let (&[version, count], rest) = rest.split_first_chunk::<2>()?;
    if version != OFFCHAIN_V1 || count == 0 {
        return None;
    }
    let keys_len = usize::from(count) * 32;
    if rest.len() <= keys_len {
        return None;
    }
    let (keys, text) = rest.split_at(keys_len);
    let signers: Vec<Pubkey> = keys
        .as_chunks::<32>()
        .0
        .iter()
        .map(|key| Pubkey::new_from_array(*key))
        .collect();
    if !signers.windows(2).all(|pair| pair[0] < pair[1]) {
        return None;
    }
    Some((signers, text))
}

#[cfg(test)]
mod tests {
    use super::*;
    use solana_message::{Message, VersionedMessage};
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
    fn a_bare_digest_is_refused_even_if_it_reads_as_text() {
        assert_eq!(
            check_message(&[0xfe; 32], ORIGIN),
            Err(SiteMessageError::Digest)
        );
        // About one digest in 2^36 reads as text, and a site can grind for one.
        let reads = b"k7$Fz!mQ2_xR9@vL4#tN8&pW1*sY6%uC";
        assert_eq!(reads.len(), 32);
        assert!(message_text(reads).is_some());
        assert_eq!(check_message(reads, ORIGIN), Err(SiteMessageError::Digest));

        assert_eq!(check_message(&[0xfe; 31], ORIGIN), Ok(()));
        assert_eq!(check_message(&[0xfe; 33], ORIGIN), Ok(()));
    }

    #[test]
    fn a_nonce_of_32_hex_digits_may_be_signed() {
        let nonce = b"0123456789abcdefABCDEF0123456789";
        assert_eq!(nonce.len(), 32);
        assert_eq!(check_message(nonce, ORIGIN), Ok(()));
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
    fn characters_that_hide_or_reorder_text_show_as_bytes() {
        for hiding in [
            "pay \u{202e}000,1 ot",
            "isolate \u{2066}this\u{2069}",
            "zero\u{200b}width",
            "soft\u{ad}hyphen",
            "line\u{2028}separator",
            "\u{feff}bom",
            "tag\u{e0041}",
        ] {
            assert_eq!(message_text(hiding.as_bytes()), None, "{hiding:?}");
        }
        for plain in [
            "caf\u{e9}",
            "\u{65e5}\u{672c}\u{8a9e}",
            "family \u{1f468}\u{200d}\u{1f469}",
        ] {
            assert_eq!(message_text(plain.as_bytes()), Some(plain));
        }
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
        let separator = SignIn {
            statement: Some("Hello\u{2028}URI: https://bank.example".into()),
            ..SignIn::default()
        };
        assert_eq!(
            separator.check(ORIGIN),
            Err(SiteMessageError::LineBreak("statement"))
        );
    }

    #[test]
    fn a_field_cannot_hide_what_it_says() {
        let statement = SignIn {
            statement: Some("Sign in to \u{202e}moc.knab".into()),
            ..SignIn::default()
        };
        assert_eq!(
            statement.check(ORIGIN),
            Err(SiteMessageError::HiddenText("statement"))
        );
        let resource = SignIn {
            resources: Some(vec!["https://a\u{200b}".into()]),
            ..SignIn::default()
        };
        assert_eq!(
            resource.check(ORIGIN),
            Err(SiteMessageError::HiddenText("resources"))
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

    #[test]
    fn a_device_signs_the_text_inside_an_offchain_message_for_the_account() {
        let account = Pubkey::new_from_array([7; 32]);
        let signed = device_message(b"Hello", ORIGIN, &account, "Ledger").unwrap();
        assert_eq!(&signed[..16], OFFCHAIN_DOMAIN);
        assert_eq!(signed[16..18], [1, 1]);
        assert_eq!(&signed[18..50], account.as_ref());
        assert_eq!(&signed[50..], b"Hello");
        assert_eq!(
            read_offchain_v1(&signed),
            Some((vec![account], &b"Hello"[..]))
        );
        // A site that sent it already in this shape gets it signed as it is.
        assert_eq!(
            device_message(&signed, ORIGIN, &account, "Ledger"),
            Ok(signed.clone())
        );
    }

    #[test]
    fn a_device_refuses_what_it_cannot_show_or_another_accounts_message() {
        let account = Pubkey::new_from_array([7; 32]);
        let other = Pubkey::new_from_array([8; 32]);
        assert_eq!(
            device_message(&[0x00, 0x01, 0xfe], ORIGIN, &account, "Ledger"),
            Err(SiteMessageError::NotText("Ledger"))
        );
        let theirs = offchain_v1(&[other], b"Hello");
        assert_eq!(
            device_message(&theirs, ORIGIN, &account, "Ledger"),
            Err(SiteMessageError::OtherOffchain("Ledger"))
        );
        let both = offchain_v1(&[other, account], b"Hello");
        assert_eq!(
            device_message(&both, ORIGIN, &account, "Ledger"),
            Err(SiteMessageError::OtherOffchain("Ledger"))
        );
        let tx = VersionedMessage::Legacy(legacy_message()).serialize();
        assert_eq!(
            device_message(&tx, ORIGIN, &account, "Ledger"),
            Err(SiteMessageError::Transaction)
        );
        let inside = offchain_v1(&[account], &tx);
        assert_eq!(
            device_message(&inside, ORIGIN, &account, "Ledger"),
            Err(SiteMessageError::Transaction)
        );
    }

    #[test]
    fn signers_go_in_ascending_order_once_each() {
        let low = Pubkey::new_from_array([1; 32]);
        let high = Pubkey::new_from_array([9; 32]);
        let message = offchain_v1(&[high, low, high], b"x");
        assert_eq!(message[17], 2);
        assert_eq!(
            read_offchain_v1(&message),
            Some((vec![low, high], &b"x"[..]))
        );
        let mut reversed = message.clone();
        reversed[18..50].copy_from_slice(high.as_ref());
        reversed[50..82].copy_from_slice(low.as_ref());
        assert_eq!(read_offchain_v1(&reversed), None);
        // No text, or another version, is not a version 1 message.
        assert_eq!(read_offchain_v1(&offchain_v1(&[low], b"")), None);
        let mut v0 = message;
        v0[16] = 0;
        assert_eq!(read_offchain_v1(&v0), None);
    }
}
