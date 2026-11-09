//! An account as the inspector shows it: what the node says of it, then its
//! data read with its owner's IDL, or the bytes as they are when no IDL reads
//! them.

use super::catalog::IdlCatalog;
use crate::chain::RawAccount;
use base64::Engine as _;
use codama_dynamic_client::{NumberValue, Value};
use solana_pubkey::Pubkey;

/// What every account has, whatever its data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccountHeader {
    pub lamports: u64,
    pub owner: Pubkey,
    pub executable: bool,
    pub rent_epoch: u64,
    pub data_len: usize,
}

/// An account read for the inspector.
#[derive(Clone, Debug, PartialEq)]
pub struct InspectedAccount {
    pub header: AccountHeader,
    pub data: Vec<u8>,
    pub reading: AccountReading,
}

/// What came of reading the data with the owner's IDL.
#[derive(Clone, Debug, PartialEq)]
pub enum AccountReading {
    /// The account holds no data.
    Empty,
    /// Read as one of the IDL's accounts.
    Decoded {
        /// The IDL's name for the program.
        program: String,
        /// The IDL's name for the account.
        account: String,
        fields: Vec<ValueNode>,
    },
    /// Not read: why, in a sentence.
    Raw(String),
}

/// One value of a decoded account, with what it holds under it.
#[derive(Clone, Debug, PartialEq)]
pub struct ValueNode {
    pub label: String,
    pub leaf: Leaf,
    pub children: Vec<ValueNode>,
}

/// How a value reads on its own line.
#[derive(Clone, Debug, PartialEq)]
pub enum Leaf {
    Text(String),
    Address(Pubkey),
    /// Bytes, shown as hex.
    Bytes(Vec<u8>),
    /// A struct, a list or an enum variant: a word for what is under it.
    Group(String),
}

/// Read `account` with the IDL its owner has in `catalog`, if any.
pub fn inspect_account(catalog: &dyn IdlCatalog, account: RawAccount) -> InspectedAccount {
    let header = AccountHeader {
        lamports: account.lamports,
        owner: account.owner,
        executable: account.executable,
        rent_epoch: account.rent_epoch,
        data_len: account.data.len(),
    };
    let reading = read_data(catalog, &account.owner, &account.data);
    InspectedAccount {
        header,
        data: account.data,
        reading,
    }
}

fn read_data(catalog: &dyn IdlCatalog, owner: &Pubkey, data: &[u8]) -> AccountReading {
    if data.is_empty() {
        return AccountReading::Empty;
    }
    let Some(record) = catalog
        .records()
        .iter()
        .find(|record| record.program_id == *owner)
    else {
        return AccountReading::Raw("No IDL of its owner is on this device.".into());
    };
    let Some(client) = catalog.program_client(owner) else {
        return AccountReading::Raw(format!("The {} IDL could not be read.", record.name));
    };
    let parsed = match client.parse_account_data(data) {
        Ok(Some(parsed)) => Some(parsed),
        Ok(None) => None,
        Err(err) => {
            return AccountReading::Raw(format!(
                "The {} IDL names this account, but its data does not read as it says: {err}",
                record.name
            ))
        }
    };
    // Token-2022 accounts with extensions are longer than their size says.
    // The byte after the base token account names which one it is.
    let parsed = parsed.or_else(|| {
        let name = match data.get(TOKEN_ACCOUNT_LEN) {
            Some(1) => "mint",
            Some(2) => "token",
            _ => return None,
        };
        client.parse_account_data_as(name, data).ok().flatten()
    });
    // An IDL with a single account and nothing to tell accounts apart by,
    // such as the system program's nonce.
    let parsed = parsed.or_else(|| {
        let accounts = &client.root().program.accounts;
        match accounts.as_slice() {
            [only] if only.discriminators.is_empty() => client
                .parse_account_data_as(only.name.as_ref(), data)
                .ok()
                .flatten(),
            _ => None,
        }
    });
    let Some(parsed) = parsed else {
        return AccountReading::Raw(format!(
            "None of the accounts of the {} IDL matches this data.",
            record.name
        ));
    };
    let account = match parsed.path.last() {
        Some(codama_nodes::Node::Account(account)) => account.name.to_string(),
        _ => "account".into(),
    };
    let fields = match &parsed.data {
        Value::Struct(fields) => fields
            .iter()
            .map(|(name, value)| value_node(name.clone(), value))
            .collect(),
        other => vec![value_node("data".into(), other)],
    };
    AccountReading::Decoded {
        program: record.name.clone(),
        account,
        fields,
    }
}

/// The length of an SPL token account, after which Token-2022 puts the
/// account type and the extensions.
const TOKEN_ACCOUNT_LEN: usize = 165;

/// Byte arrays at least this long show as hex rather than a list of numbers.
const BYTES_FROM: usize = 8;

fn value_node(label: String, value: &Value) -> ValueNode {
    let leaf = |leaf| ValueNode {
        label: label.clone(),
        leaf,
        children: Vec::new(),
    };
    match value {
        Value::Number(number) => leaf(Leaf::Text(number_text(number))),
        Value::Bool(flag) => leaf(Leaf::Text(flag.to_string())),
        Value::String(text) => leaf(Leaf::Text(format!("\"{text}\""))),
        Value::Bytes(bytes) => leaf(Leaf::Bytes(bytes.clone())),
        Value::PublicKey(bytes) => leaf(Leaf::Address(Pubkey::new_from_array(*bytes))),
        Value::Option(None) => leaf(Leaf::Text("none".into())),
        Value::Option(Some(inner)) => value_node(label, inner),
        Value::Array(items) | Value::Set(items) | Value::Tuple(items) => {
            if let Some(bytes) = as_bytes(items) {
                return leaf(Leaf::Bytes(bytes));
            }
            ValueNode {
                label: label.clone(),
                leaf: Leaf::Group(count(items.len(), "item")),
                children: items
                    .iter()
                    .enumerate()
                    .map(|(i, item)| value_node(i.to_string(), item))
                    .collect(),
            }
        }
        Value::Map(entries) => ValueNode {
            label: label.clone(),
            leaf: Leaf::Group(count(entries.len(), "entry")),
            children: entries
                .iter()
                .map(|(key, value)| value_node(key_text(key), value))
                .collect(),
        },
        Value::Struct(fields) => ValueNode {
            label: label.clone(),
            leaf: Leaf::Group(String::new()),
            children: fields
                .iter()
                .map(|(name, value)| value_node(name.clone(), value))
                .collect(),
        },
        Value::Enum { variant, fields } => {
            let children: Vec<ValueNode> = match &**fields {
                Value::Struct(fields) => fields
                    .iter()
                    .flat_map(|(name, value)| match value {
                        // A tuple variant's fields, by position.
                        Value::Tuple(items) if name == "fields" => items
                            .iter()
                            .enumerate()
                            .map(|(i, item)| value_node(i.to_string(), item))
                            .collect(),
                        _ => vec![value_node(name.clone(), value)],
                    })
                    .collect(),
                other => vec![value_node("value".into(), other)],
            };
            if children.is_empty() {
                leaf(Leaf::Text(variant.clone()))
            } else {
                ValueNode {
                    label: label.clone(),
                    leaf: Leaf::Group(variant.clone()),
                    children,
                }
            }
        }
    }
}

fn number_text(number: &NumberValue) -> String {
    match number {
        NumberValue::Unsigned(n) => n.to_string(),
        NumberValue::Signed(n) => n.to_string(),
        NumberValue::Float(n) => n.to_string(),
    }
}

fn key_text(key: &Value) -> String {
    match key {
        Value::Number(number) => number_text(number),
        Value::String(text) => text.clone(),
        Value::PublicKey(bytes) => Pubkey::new_from_array(*bytes).to_string(),
        Value::Bool(flag) => flag.to_string(),
        Value::Enum { variant, .. } => variant.clone(),
        other => format!("{other:?}"),
    }
}

/// A list of numbers that are all bytes, long enough to be a byte array
/// rather than a list someone reads one by one.
fn as_bytes(items: &[Value]) -> Option<Vec<u8>> {
    if items.len() < BYTES_FROM {
        return None;
    }
    items
        .iter()
        .map(|item| match item {
            Value::Number(NumberValue::Unsigned(n)) => u8::try_from(*n).ok(),
            _ => None,
        })
        .collect()
}

fn count(n: usize, what: &str) -> String {
    match (n, what) {
        (1, _) => format!("1 {what}"),
        (_, "entry") => format!("{n} entries"),
        _ => format!("{n} {what}s"),
    }
}

/// The most bytes [`hex_dump`] and [`base64_text`] show, so a large account
/// does not stall the window.
pub const SHOWN_BYTES: usize = 64 * 1024;

/// `bytes` as lines of 16: the offset, the bytes in hex, and the printable
/// ones as text.
pub fn hex_dump(bytes: &[u8]) -> String {
    let shown = &bytes[..bytes.len().min(SHOWN_BYTES)];
    let mut out = String::with_capacity(shown.len() * 4 + shown.len() / 16 * 12);
    for (row, chunk) in shown.chunks(16).enumerate() {
        out.push_str(&format!("{:08x}  ", row * 16));
        for i in 0..16 {
            match chunk.get(i) {
                Some(byte) => out.push_str(&format!("{byte:02x} ")),
                None => out.push_str("   "),
            }
            if i == 7 {
                out.push(' ');
            }
        }
        out.push(' ');
        out.extend(chunk.iter().map(|&byte| {
            if byte.is_ascii_graphic() || byte == b' ' {
                byte as char
            } else {
                '.'
            }
        }));
        out.push('\n');
    }
    out.pop();
    out
}

/// `bytes` in base64, wrapped at 76 characters.
pub fn base64_text(bytes: &[u8]) -> String {
    let shown = &bytes[..bytes.len().min(SHOWN_BYTES)];
    let text = base64::engine::general_purpose::STANDARD.encode(shown);
    text.as_bytes()
        .chunks(76)
        .map(|line| std::str::from_utf8(line).unwrap_or_default())
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::idl::catalog::{IdlStore, MemoryBodies};
    use crate::ids::{SYSTEM_PROGRAM_ID, TOKEN_PROGRAM_ID};

    fn catalog() -> IdlStore<wallet_vault::store::MemoryStore, MemoryBodies> {
        IdlStore::open(wallet_vault::store::MemoryStore::new(), MemoryBodies::new()).with_builtins()
    }

    fn token_account(mint: Pubkey, owner: Pubkey, amount: u64) -> Vec<u8> {
        let mut data = Vec::with_capacity(165);
        data.extend_from_slice(mint.as_ref());
        data.extend_from_slice(owner.as_ref());
        data.extend_from_slice(&amount.to_le_bytes());
        data.extend_from_slice(&[0; 36]); // no delegate
        data.push(1); // initialized
        data.extend_from_slice(&[0; 12]); // not native
        data.extend_from_slice(&0u64.to_le_bytes());
        data.extend_from_slice(&[0; 36]); // no close authority
        assert_eq!(data.len(), 165);
        data
    }

    fn field<'a>(fields: &'a [ValueNode], label: &str) -> &'a ValueNode {
        fields
            .iter()
            .find(|node| node.label == label)
            .unwrap_or_else(|| panic!("no {label} in {fields:?}"))
    }

    #[test]
    fn a_token_account_reads_with_the_token_idl() {
        let mint = Pubkey::new_unique();
        let owner = Pubkey::new_unique();
        let read = inspect_account(
            &catalog(),
            RawAccount {
                lamports: 2_039_280,
                data: token_account(mint, owner, 42),
                owner: TOKEN_PROGRAM_ID,
                executable: false,
                rent_epoch: u64::MAX,
            },
        );
        assert_eq!(read.header.data_len, 165);
        let AccountReading::Decoded {
            account, fields, ..
        } = read.reading
        else {
            panic!("not decoded: {:?}", read.reading);
        };
        assert_eq!(account, "token");
        assert_eq!(field(&fields, "mint").leaf, Leaf::Address(mint));
        assert_eq!(field(&fields, "owner").leaf, Leaf::Address(owner));
        assert_eq!(field(&fields, "amount").leaf, Leaf::Text("42".into()));
        assert_eq!(field(&fields, "delegate").leaf, Leaf::Text("none".into()));
    }

    #[test]
    fn a_nonce_reads_with_the_system_idl_though_nothing_tells_it_apart() {
        let authority = Pubkey::new_unique();
        let mut data = Vec::new();
        data.extend_from_slice(&1u32.to_le_bytes());
        data.extend_from_slice(&1u32.to_le_bytes());
        data.extend_from_slice(authority.as_ref());
        data.extend_from_slice(&[7; 32]);
        data.extend_from_slice(&5000u64.to_le_bytes());
        let read = inspect_account(
            &catalog(),
            RawAccount {
                lamports: 1_447_680,
                data,
                owner: SYSTEM_PROGRAM_ID,
                executable: false,
                rent_epoch: 0,
            },
        );
        let AccountReading::Decoded {
            account, fields, ..
        } = read.reading
        else {
            panic!("not decoded: {:?}", read.reading);
        };
        assert_eq!(account, "nonce");
        assert_eq!(field(&fields, "authority").leaf, Leaf::Address(authority));
    }

    #[test]
    fn data_no_idl_reads_stays_raw() {
        let read = inspect_account(
            &catalog(),
            RawAccount {
                lamports: 1,
                data: vec![1, 2, 3],
                owner: Pubkey::new_unique(),
                executable: false,
                rent_epoch: 0,
            },
        );
        assert!(matches!(read.reading, AccountReading::Raw(_)));
    }

    #[test]
    fn a_wallet_has_no_data() {
        let read = inspect_account(
            &catalog(),
            RawAccount {
                lamports: 1,
                data: Vec::new(),
                owner: SYSTEM_PROGRAM_ID,
                executable: false,
                rent_epoch: 0,
            },
        );
        assert_eq!(read.reading, AccountReading::Empty);
    }

    #[test]
    fn hex_dump_shows_offsets_hex_and_text() {
        let dump = hex_dump(b"Airlock reads accounts!");
        let lines: Vec<&str> = dump.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].starts_with("00000000  41 69 72 6c 6f 63 6b 20  72 65 61 64"));
        assert!(lines[0].ends_with("Airlock reads ac"));
        assert!(lines[1].starts_with("00000010  "));
        assert!(lines[1].ends_with("counts!"));
    }

    #[test]
    fn base64_wraps_long_data() {
        let text = base64_text(&[0u8; 120]);
        assert!(text.lines().all(|line| line.len() <= 76));
        assert_eq!(text.replace('\n', "").len(), 160);
    }
}
