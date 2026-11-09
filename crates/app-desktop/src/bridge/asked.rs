//! What a frame asks, read from its params once.
//!
//! A site writes the params, so nothing in them is taken as given: what is
//! missing or of another type reads as absent, and what the bridge then makes
//! of that is its business. Only what is cheap is read here. A transaction
//! stays the text the site sent until a request is let through, so one that is
//! refused costs no decoding.

use std::collections::BTreeSet;

use app_core::bridge::{SendPlan, SiteCall, SiteTransaction, SiteWire, SEND, SEND_ALL};
use base64::Engine;
use bridge_protocol::request::is_site_origin;
use bridge_protocol::MAX_TRANSACTIONS;
use serde_json::Value;
use solana_pubkey::Pubkey;
use wallet_core::ConfirmationLevel;

/// One frame's method, with what its params hold for it.
#[derive(Debug, PartialEq)]
pub(super) enum Asked {
    Status,
    /// A second launch knocking.
    Show,
    /// The origins with a page open in a browser, those that are web origins.
    Pages(BTreeSet<String>),
    /// The id of a request nobody waits on any more.
    Cancel(Option<String>),
    Disconnect,
    Connect {
        silent: bool,
    },
    SignMessage {
        /// The account named, when it reads as an address.
        account: Option<Pubkey>,
        /// The message as sent, in base64.
        message: Option<String>,
    },
    /// The Wallet Standard's sign-in input, which `SignIn` reads whole.
    SignIn(Value),
    Transactions(AskedTransactions),
    /// A method the app does not know.
    Other,
}

/// How a request for transactions wants them back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    Sign,
    Send,
    SendAll { serial: bool },
}

/// The transactions of a `signTransaction` or a send.
#[derive(Debug, PartialEq)]
pub(super) struct AskedTransactions {
    pub kind: Kind,
    /// How many the site sent.
    pub count: usize,
    /// Whether every one says which network it is for.
    pub every_chain_named: bool,
    /// Each of them, or none when there are more than the app signs at once:
    /// that request is refused whatever they hold.
    items: Vec<AskedTransaction>,
}

#[derive(Debug, PartialEq)]
struct AskedTransaction {
    /// The account named, when it reads as an address.
    account: Option<Pubkey>,
    /// The transaction as sent, in base64.
    transaction: Option<String>,
    chain: Option<String>,
    commitment: Option<ConfirmationLevel>,
}

impl Asked {
    pub(super) fn read(method: &str, params: Value) -> Self {
        let text = |field: &str| params.get(field).and_then(Value::as_str);
        match method {
            "status" => Self::Status,
            "show" => Self::Show,
            "pages" => Self::Pages(listed_pages(&params)),
            "cancel" => Self::Cancel(text("id").map(str::to_owned)),
            "disconnect" => Self::Disconnect,
            "connect" => Self::Connect {
                silent: params.get("silent").and_then(Value::as_bool) == Some(true),
            },
            "signMessage" => Self::SignMessage {
                account: text("account").and_then(|account| account.parse().ok()),
                message: text("message").map(str::to_owned),
            },
            "signIn" => Self::SignIn(params),
            "signTransaction" => Self::Transactions(AskedTransactions::read(Kind::Sign, &params)),
            SEND => Self::Transactions(AskedTransactions::read(Kind::Send, &params)),
            SEND_ALL => {
                let serial = text("mode") == Some("serial");
                Self::Transactions(AskedTransactions::read(Kind::SendAll { serial }, &params))
            }
            _ => Self::Other,
        }
    }

    /// A connect that may only ask whether the site is connected already.
    pub(super) fn is_silent_connect(&self) -> bool {
        matches!(self, Self::Connect { silent: true })
    }
}

impl AskedTransactions {
    fn read(kind: Kind, params: &Value) -> Self {
        let sent = params
            .get("transactions")
            .and_then(Value::as_array)
            .map_or(&[][..], Vec::as_slice);
        let chain = |item: &Value| item.get("chain").and_then(Value::as_str).map(str::to_owned);
        let items = if sent.len() > MAX_TRANSACTIONS {
            Vec::new()
        } else {
            sent.iter()
                .map(|item| AskedTransaction {
                    account: item
                        .get("account")
                        .and_then(Value::as_str)
                        .and_then(|account| account.parse().ok()),
                    transaction: item
                        .get("transaction")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                    chain: chain(item),
                    commitment: match item
                        .get("options")
                        .and_then(|options| options.get("commitment"))
                        .and_then(Value::as_str)
                    {
                        Some("processed") => Some(ConfirmationLevel::Processed),
                        Some("confirmed") => Some(ConfirmationLevel::Confirmed),
                        Some("finalized") => Some(ConfirmationLevel::Finalized),
                        _ => None,
                    },
                })
                .collect()
        };
        Self {
            kind,
            count: sent.len(),
            every_chain_named: sent
                .iter()
                .all(|item| item.get("chain").is_some_and(Value::is_string)),
            items,
        }
    }

    /// Whether the app sends what this asks to have signed.
    pub(super) fn sends(&self) -> bool {
        self.kind != Kind::Sign
    }

    /// The accounts the transactions name, those that read as addresses.
    pub(super) fn accounts(&self) -> Vec<Pubkey> {
        self.items.iter().filter_map(|item| item.account).collect()
    }

    /// The request these make, each of them having named one of `accounts`.
    /// This is where each transaction is decoded.
    pub(super) fn call(self, accounts: Vec<Pubkey>) -> SiteCall {
        let transactions = self
            .items
            .into_iter()
            .zip(accounts)
            .map(|(item, account)| SiteTransaction {
                account,
                wire: read_wire(item.transaction.as_deref()),
                chain: item.chain,
                commitment: item.commitment,
            })
            .collect();
        let plan = match self.kind {
            Kind::Sign => return SiteCall::SignTransactions(transactions),
            Kind::Send => SendPlan::InOrder,
            Kind::SendAll { serial: true } => SendPlan::Serial,
            Kind::SendAll { serial: false } => SendPlan::Independent,
        };
        SiteCall::Send { transactions, plan }
    }
}

/// The origins a `pages` frame lists. One that is not a web origin is left
/// out, rather than the whole list refused.
pub(super) fn listed_pages(params: &Value) -> BTreeSet<String> {
    params
        .get("origins")
        .and_then(Value::as_array)
        .map(|origins| {
            origins
                .iter()
                .filter_map(Value::as_str)
                .filter(|origin| is_site_origin(origin))
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// A transaction as a site sent it, in base64, read as far as it can be.
pub(super) fn read_wire(transaction: Option<&str>) -> SiteWire {
    let Some(text) = transaction else {
        return SiteWire::Missing;
    };
    let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(text) else {
        return SiteWire::NotBase64;
    };
    match wallet_core::parse_wire(&bytes).map(|view| view.format) {
        Ok(format) => SiteWire::Readable(format, bytes),
        Err(_) => SiteWire::Unreadable(bytes.len()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn key(byte: u8) -> Pubkey {
        Pubkey::new_from_array([byte; 32])
    }

    fn transactions(method: &str, params: Value) -> AskedTransactions {
        match Asked::read(method, params) {
            Asked::Transactions(asked) => asked,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn each_method_reads_what_it_needs() {
        assert_eq!(Asked::read("status", Value::Null), Asked::Status);
        assert_eq!(Asked::read("show", json!({})), Asked::Show);
        assert_eq!(Asked::read("disconnect", json!({})), Asked::Disconnect);
        assert_eq!(Asked::read("signAll", json!({})), Asked::Other);
        assert_eq!(
            Asked::read("cancel", json!({ "id": "abc" })),
            Asked::Cancel(Some("abc".into()))
        );
        assert_eq!(
            Asked::read("cancel", json!({ "id": 7 })),
            Asked::Cancel(None)
        );
        let sign_in = json!({ "domain": "a.example" });
        assert_eq!(
            Asked::read("signIn", sign_in.clone()),
            Asked::SignIn(sign_in)
        );
        assert_eq!(
            Asked::read(
                "signMessage",
                json!({ "account": key(1).to_string(), "message": "aGk=" })
            ),
            Asked::SignMessage {
                account: Some(key(1)),
                message: Some("aGk=".into())
            }
        );
        assert_eq!(
            Asked::read("signMessage", json!({ "account": "nobody", "message": 3 })),
            Asked::SignMessage {
                account: None,
                message: None
            }
        );
    }

    #[test]
    fn a_connect_is_silent_only_when_it_says_so() {
        for (params, silent) in [
            (json!({ "silent": true }), true),
            (json!({ "silent": false }), false),
            (json!({ "silent": "true" }), false),
            (json!({ "silent": 1 }), false),
            (json!({}), false),
            (Value::Null, false),
        ] {
            let asked = Asked::read("connect", params);
            assert_eq!(asked, Asked::Connect { silent });
            assert_eq!(asked.is_silent_connect(), silent);
        }
        // Nothing else is, whatever it carries.
        assert!(!Asked::read("signIn", json!({ "silent": true })).is_silent_connect());
    }

    #[test]
    fn transactions_are_read_as_far_as_each_goes() {
        let asked = transactions(
            SEND_ALL,
            json!({
                "mode": "serial",
                "transactions": [
                    {
                        "account": key(1).to_string(),
                        "transaction": "AQID",
                        "chain": "solana:devnet",
                        "options": { "commitment": "finalized" },
                    },
                    { "account": "nobody", "transaction": 7, "chain": 5 },
                    {},
                ],
            }),
        );
        assert_eq!(asked.kind, Kind::SendAll { serial: true });
        assert_eq!(asked.count, 3);
        assert!(!asked.every_chain_named);
        assert!(asked.sends());
        // Only the first named an account that reads.
        assert_eq!(asked.accounts(), [key(1)]);
        assert_eq!(
            asked.items[0],
            AskedTransaction {
                account: Some(key(1)),
                transaction: Some("AQID".into()),
                chain: Some("solana:devnet".into()),
                commitment: Some(ConfirmationLevel::Finalized),
            }
        );
        assert_eq!(
            asked.items[1],
            AskedTransaction {
                account: None,
                transaction: None,
                chain: None,
                commitment: None,
            }
        );
    }

    #[test]
    fn how_they_go_out_comes_from_the_method_and_its_mode() {
        let one = |method: &str, mode: Value| {
            let params = json!({
                "mode": mode,
                "transactions": [{ "account": key(1).to_string(), "chain": "solana:devnet" }],
            });
            transactions(method, params).call(vec![key(1)])
        };
        assert!(matches!(
            one("signTransaction", json!("serial")),
            SiteCall::SignTransactions(sent) if sent.len() == 1 && sent[0].wire == SiteWire::Missing
        ));
        for (method, mode, plan) in [
            (SEND, json!("serial"), SendPlan::InOrder),
            (SEND_ALL, json!("serial"), SendPlan::Serial),
            (SEND_ALL, json!("parallel"), SendPlan::Independent),
            (SEND_ALL, Value::Null, SendPlan::Independent),
        ] {
            match one(method, mode) {
                SiteCall::Send {
                    transactions,
                    plan: planned,
                } => {
                    assert_eq!(planned, plan);
                    assert_eq!(transactions[0].account, key(1));
                    assert_eq!(transactions[0].chain.as_deref(), Some("solana:devnet"));
                }
                other => panic!("{other:?}"),
            }
        }
    }

    #[test]
    fn no_transactions_and_too_many_are_counted_and_not_read() {
        for params in [
            Value::Null,
            json!({}),
            json!({ "transactions": "x" }),
            json!({ "transactions": [] }),
        ] {
            let asked = transactions("signTransaction", params);
            assert_eq!(asked.count, 0);
            assert!(asked.every_chain_named);
            assert!(!asked.sends());
        }
        let many: Vec<Value> = (0..=MAX_TRANSACTIONS)
            .map(|_| json!({ "account": key(1).to_string(), "transaction": "AQID" }))
            .collect();
        let asked = transactions(SEND, json!({ "transactions": many }));
        assert_eq!(asked.count, MAX_TRANSACTIONS + 1);
        // Still said, since a send that names no network is refused first.
        assert!(!asked.every_chain_named);
        assert!(asked.items.is_empty());
    }
}
