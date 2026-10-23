//! Asking a node what a transaction would do, and reading its answer.
//!
//! Nothing here opens a socket. An RPC client sends the request this module
//! describes and hands back the node's answer, so the reading can be tested
//! on made-up answers.

pub use crate::chain::{CompiledCall, InnerReply, SimulationReply, SimulationRequest};
use crate::tx::decode::DecodedTx;
use serde_json::Value;
use solana_instruction::{AccountMeta, Instruction};
use solana_pubkey::Pubkey;

/// What a node says a transaction would do if it were sent now.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Simulation {
    /// `None` when the transaction would succeed.
    pub error: Option<SimulationError>,
    pub logs: Vec<String>,
    pub units_consumed: Option<u64>,
    /// Bytes of account data the transaction loaded. Older nodes do not say.
    pub loaded_accounts_data_size: Option<u32>,
    /// Lamports of each watched account that would change.
    pub balance_changes: Vec<BalanceChange>,
    /// Calls the transaction's instructions would make into other programs.
    pub inner: Vec<InnerCalls>,
    /// The program that returned data last, and the data.
    pub return_data: Option<(Pubkey, Vec<u8>)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SimulationError {
    /// The instruction that failed, counted from zero. `None` when the
    /// transaction was refused before any instruction ran.
    pub instruction: Option<usize>,
    pub message: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BalanceChange {
    pub pubkey: Pubkey,
    pub before: u64,
    pub after: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InnerCalls {
    /// Which instruction of the transaction made the calls.
    pub instruction: usize,
    pub calls: Vec<InnerCall>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InnerCall {
    /// How deep the call is: 2 is called by the instruction itself.
    pub depth: u8,
    pub instruction: Instruction,
}

/// The accounts whose balance a simulation reports on: every account the
/// transaction may write to.
pub fn watched_accounts(tx: &DecodedTx) -> Vec<Pubkey> {
    tx.accounts
        .iter()
        .filter(|account| account.writable)
        .map(|account| account.pubkey)
        .collect()
}

impl SimulationRequest {
    pub fn new(tx: &DecodedTx, watched: &[Pubkey]) -> Self {
        Self {
            wire: tx.wire.clone(),
            replace_recent_blockhash: !tx.uses_durable_nonce,
            watched: watched.to_vec(),
        }
    }
}

/// Read a simulation. `before` holds the balances of `watched` from just
/// before, in the same order.
pub fn read_simulation(
    reply: SimulationReply,
    tx: &DecodedTx,
    watched: &[Pubkey],
    before: &[u64],
) -> Simulation {
    let error = reply
        .err
        .as_ref()
        .filter(|err| !err.is_null())
        .map(describe_error);

    let mut balance_changes = Vec::new();
    // A failed transaction changes nothing but the fee payer's balance, and
    // the node reports the accounts as they were. Showing that as "no change"
    // would be true and useless, so changes are listed only on success.
    if error.is_none() {
        for (index, pubkey) in watched.iter().enumerate() {
            let before = before.get(index).copied().unwrap_or(0);
            let after = reply.balances.get(index).copied().unwrap_or(0);
            if before != after {
                balance_changes.push(BalanceChange {
                    pubkey: *pubkey,
                    before,
                    after,
                });
            }
        }
    }

    let inner = reply
        .inner
        .iter()
        .map(|group| inner_calls(group, tx))
        .collect();

    Simulation {
        error,
        logs: reply.logs,
        units_consumed: reply.units_consumed,
        loaded_accounts_data_size: reply.loaded_accounts_data_size,
        balance_changes,
        inner,
        return_data: reply.return_data,
    }
}

/// The calls of `group` as instructions. A call that names an account the
/// transaction does not have is left out.
fn inner_calls(group: &InnerReply, tx: &DecodedTx) -> InnerCalls {
    let meta = |index: &u8| {
        let account = tx.accounts.get(usize::from(*index))?;
        Some(AccountMeta {
            pubkey: account.pubkey,
            is_signer: account.signer,
            is_writable: account.writable,
        })
    };
    let calls = group
        .calls
        .iter()
        .filter_map(|call| {
            Some(InnerCall {
                depth: call
                    .stack_height
                    .and_then(|depth| u8::try_from(depth).ok())
                    .unwrap_or(2),
                instruction: Instruction {
                    program_id: meta(&call.program_id_index)?.pubkey,
                    accounts: call.accounts.iter().map(meta).collect::<Option<Vec<_>>>()?,
                    data: call.data.clone(),
                },
            })
        })
        .collect();
    InnerCalls {
        instruction: group.instruction,
        calls,
    }
}

/// A node's `err` value in words. Its shape is the runtime's
/// `TransactionError`: a name, or a name with details.
fn describe_error(err: &Value) -> SimulationError {
    if let Some(parts) = err.get("InstructionError").and_then(Value::as_array) {
        let instruction = parts
            .first()
            .and_then(Value::as_u64)
            .and_then(|index| usize::try_from(index).ok());
        let detail = parts.get(1).unwrap_or(&Value::Null);
        let message = match detail.get("Custom").and_then(Value::as_u64) {
            Some(code) => format!("the program returned error {code} ({code:#x})"),
            None => spaced(&plain(detail)),
        };
        return SimulationError {
            instruction,
            message,
        };
    }
    let name = plain(err);
    let message = match name.as_str() {
        "AccountNotFound" => "the fee payer does not exist on this cluster".to_string(),
        "InsufficientFundsForFee" => "the fee payer cannot afford the fee".to_string(),
        "BlockhashNotFound" => "the blockhash is too old or unknown to this cluster".to_string(),
        "AlreadyProcessed" => "this transaction has already been processed".to_string(),
        _ => spaced(&name),
    };
    SimulationError {
        instruction: None,
        message,
    }
}

fn plain(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// `InvalidAccountData` as "invalid account data". Anything that is not a
/// single CamelCase word is left as it is.
fn spaced(name: &str) -> String {
    if name.is_empty() || !name.chars().all(|ch| ch.is_ascii_alphanumeric()) {
        return name.to_string();
    }
    let mut out = String::with_capacity(name.len() + 8);
    for ch in name.chars() {
        if ch.is_ascii_uppercase() && !out.is_empty() {
            out.push(' ');
        }
        out.push(ch.to_ascii_lowercase());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::{compile, pk, transfer};
    use crate::ids::SYSTEM_PROGRAM_ID;
    use crate::tx::decode::decode_v1_transaction;
    use serde_json::json;

    fn tx() -> DecodedTx {
        decode_v1_transaction(&compile(pk(1), vec![transfer(pk(1), pk(2), 5)]).wire).unwrap()
    }

    #[test]
    fn the_request_watches_what_may_change_and_replaces_the_blockhash() {
        let tx = tx();
        let watched = watched_accounts(&tx);
        assert_eq!(watched, vec![pk(1), pk(2)]);
        let request = SimulationRequest::new(&tx, &watched);
        assert_eq!(request.wire, tx.wire);
        assert!(request.replace_recent_blockhash);
        assert_eq!(request.watched, watched);
    }

    #[test]
    fn a_durable_nonce_transaction_keeps_its_own_lifetime() {
        let on_nonce = crate::tx::builder::compile_v1(
            pk(1),
            vec![transfer(pk(1), pk(2), 5)],
            Some(crate::tx::builder::NonceSpec {
                account: pk(3),
                authority: pk(1),
                hash: crate::fixtures::blockhash(4),
            }),
            None,
        )
        .unwrap();
        let tx = decode_v1_transaction(&on_nonce.wire).unwrap();
        assert!(!SimulationRequest::new(&tx, &[]).replace_recent_blockhash);
    }

    #[test]
    fn a_success_lists_cost_balance_changes_and_inner_calls() {
        let tx = tx();
        let watched = watched_accounts(&tx);
        // Account 2 is the System program in this message.
        let system_index = tx
            .accounts
            .iter()
            .position(|account| account.pubkey == SYSTEM_PROGRAM_ID)
            .unwrap();
        let reply = SimulationReply {
            err: None,
            logs: vec![
                "Program 11111111111111111111111111111111 invoke [1]".into(),
                "Program 11111111111111111111111111111111 success".into(),
            ],
            units_consumed: Some(150),
            loaded_accounts_data_size: Some(142),
            balances: vec![994_995, 5],
            inner: vec![InnerReply {
                instruction: 0,
                calls: vec![
                    CompiledCall {
                        program_id_index: system_index as u8,
                        accounts: vec![0, 1],
                        data: vec![2, 0, 0, 0],
                        stack_height: Some(2),
                    },
                    // Names an account the transaction does not have.
                    CompiledCall {
                        program_id_index: 200,
                        accounts: vec![],
                        data: vec![],
                        stack_height: None,
                    },
                ],
            }],
            return_data: Some((SYSTEM_PROGRAM_ID, vec![7, 8])),
        };
        let simulation = read_simulation(reply, &tx, &watched, &[1_000_000, 0]);
        assert_eq!(simulation.error, None);
        assert_eq!(simulation.units_consumed, Some(150));
        assert_eq!(simulation.loaded_accounts_data_size, Some(142));
        assert_eq!(simulation.logs.len(), 2);
        assert_eq!(
            simulation.balance_changes,
            vec![
                BalanceChange {
                    pubkey: pk(1),
                    before: 1_000_000,
                    after: 994_995
                },
                BalanceChange {
                    pubkey: pk(2),
                    before: 0,
                    after: 5
                },
            ]
        );
        assert_eq!(simulation.inner[0].calls.len(), 1);
        let call = &simulation.inner[0].calls[0];
        assert_eq!((simulation.inner[0].instruction, call.depth), (0, 2));
        assert_eq!(call.instruction.program_id, SYSTEM_PROGRAM_ID);
        assert_eq!(call.instruction.data, vec![2, 0, 0, 0]);
        assert!(call.instruction.accounts[0].is_signer);
        assert_eq!(
            simulation.return_data,
            Some((SYSTEM_PROGRAM_ID, vec![7, 8]))
        );
    }

    #[test]
    fn a_failure_names_the_instruction_and_lists_no_changes() {
        let tx = tx();
        let watched = watched_accounts(&tx);
        let read = |err: Value| {
            let reply = SimulationReply {
                err: Some(err),
                balances: vec![9, 9],
                ..SimulationReply::default()
            };
            read_simulation(reply, &tx, &watched, &[1, 1])
        };

        let custom = read(json!({ "InstructionError": [0, { "Custom": 3 }] }));
        assert_eq!(
            custom.error,
            Some(SimulationError {
                instruction: Some(0),
                message: "the program returned error 3 (0x3)".into(),
            })
        );
        assert!(custom.balance_changes.is_empty());

        let named = read(json!({ "InstructionError": [1, "InvalidAccountData"] }));
        assert_eq!(
            named.error,
            Some(SimulationError {
                instruction: Some(1),
                message: "invalid account data".into(),
            })
        );

        let refused = read(json!("AccountNotFound")).error.unwrap();
        assert_eq!(refused.instruction, None);
        assert!(refused.message.contains("fee payer"));

        let odd = read(json!({ "InsufficientFundsForRent": { "account_index": 2 } }));
        assert!(odd.error.unwrap().message.contains("account_index"));
    }
}
