//! From the types both clients answer in to the app's own. The clients share
//! Agave's account and transaction-status types, so this is written once.

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use solana_account_decoder_client_types::token::TokenAccountType;
use solana_account_decoder_client_types::{UiAccount, UiAccountData};
use solana_pubkey::Pubkey;
use solana_signature::Signature;
use solana_transaction_status_client_types::{
    EncodedConfirmedTransactionWithStatusMeta, EncodedTransaction, TransactionBinaryEncoding,
    TransactionConfirmationStatus, UiInnerInstructions, UiInstruction, UiTransactionTokenBalance,
};
use wallet_core::simulate::{CompiledCall, InnerReply, SimulationReply};
use wallet_core::stake::StakeReward;
use wallet_core::{
    AddressSignature, ConfirmationLevel, DiscoveredTokenAccount, LandedTx, RpcError,
    SignatureStatus, TokenBalanceChange,
};

/// From either client's signature status into the app's.
pub fn signature_status(
    slot: u64,
    confirmations: Option<u64>,
    err: Option<String>,
    confirmation_status: Option<&TransactionConfirmationStatus>,
) -> SignatureStatus {
    let confirmation = confirmation_status
        .map(|c| match c {
            TransactionConfirmationStatus::Processed => ConfirmationLevel::Processed,
            TransactionConfirmationStatus::Confirmed => ConfirmationLevel::Confirmed,
            TransactionConfirmationStatus::Finalized => ConfirmationLevel::Finalized,
        })
        .or({
            // Older nodes omit confirmation_status; mirror Agave's fallback.
            match confirmations {
                None => Some(ConfirmationLevel::Finalized),
                Some(0) => Some(ConfirmationLevel::Processed),
                Some(_) => Some(ConfirmationLevel::Confirmed),
            }
        });
    SignatureStatus {
        slot,
        err,
        confirmation,
    }
}

/// Whether a client's text says the node is being asked too often. What a program
/// logs can say anything, so a simulation's logs do not count.
fn says_rate_limited(lower: &str) -> bool {
    !lower.contains("log messages")
        && ["too many requests", "rate limit", "-32429"]
            .iter()
            .any(|words| lower.contains(words))
}

/// An error a client only gives as text. A timeout, a request that never
/// reached the node and one that got no answer are told apart from an answer
/// the node gave.
pub fn rpc_error(err: impl ToString) -> RpcError {
    let message = err.to_string();
    let lower = message.to_ascii_lowercase();
    if says_rate_limited(&lower) {
        RpcError::RateLimited
    } else if lower.contains("timed out") || lower.contains("timeout") {
        RpcError::Timeout
    } else if lower.contains("minimum context slot has not been reached") {
        RpcError::Behind
    } else if ["connection refused", "dns error", "error trying to connect"]
        .iter()
        .any(|words| lower.contains(words))
    {
        RpcError::Unreachable(message)
    } else if ["error sending request", "failed to fetch", "networkerror"]
        .iter()
        .any(|words| lower.contains(words))
    {
        RpcError::NoAnswer(message)
    } else {
        RpcError::Message(message)
    }
}

/// A token account the node parsed. `None` for anything else, such as a mint.
pub fn token_account(
    owner: Pubkey,
    pubkey: &str,
    data: &UiAccountData,
) -> Option<DiscoveredTokenAccount> {
    let UiAccountData::Json(parsed) = data else {
        return None;
    };
    let TokenAccountType::Account(info) = serde_json::from_value(parsed.parsed.clone()).ok()?
    else {
        return None;
    };
    Some(DiscoveredTokenAccount {
        pubkey: pubkey.parse().ok()?,
        owner: info.owner.parse().ok().unwrap_or(owner),
        mint: info.mint.parse().ok()?,
        amount: info.token_amount.amount.parse().unwrap_or(0),
        decimals: info.token_amount.decimals,
    })
}

/// The parts of a simulation result, named the same by both clients.
pub struct SimulationParts {
    /// The runtime's error as the node's JSON.
    pub err: Option<serde_json::Value>,
    pub logs: Option<Vec<String>>,
    pub units_consumed: Option<u64>,
    pub loaded_accounts_data_size: Option<u32>,
    pub accounts: Option<Vec<Option<UiAccount>>>,
    pub inner_instructions: Option<Vec<UiInnerInstructions>>,
    /// The program that returned data, and the data in base64.
    pub return_data: Option<(String, String)>,
}

pub fn simulation_reply(parts: SimulationParts) -> SimulationReply {
    SimulationReply {
        err: parts.err.filter(|err| !err.is_null()),
        logs: parts.logs.unwrap_or_default(),
        units_consumed: parts.units_consumed,
        loaded_accounts_data_size: parts.loaded_accounts_data_size,
        balances: parts
            .accounts
            .unwrap_or_default()
            .iter()
            .map(|account| account.as_ref().map_or(0, |account| account.lamports))
            .collect(),
        inner: parts
            .inner_instructions
            .unwrap_or_default()
            .iter()
            .map(inner_reply)
            .collect(),
        return_data: parts
            .return_data
            .and_then(|(program, data)| Some((program.parse().ok()?, STANDARD.decode(data).ok()?))),
    }
}

/// The calls of one instruction. A call the node sent parsed, or with data
/// that is not base58, is left out.
fn inner_reply(group: &UiInnerInstructions) -> InnerReply {
    InnerReply {
        instruction: usize::from(group.index),
        calls: group
            .instructions
            .iter()
            .filter_map(|call| {
                let UiInstruction::Compiled(call) = call else {
                    return None;
                };
                Some(CompiledCall {
                    program_id_index: call.program_id_index,
                    accounts: call.accounts.clone(),
                    data: bs58::decode(&call.data).into_vec().ok()?,
                    stack_height: call.stack_height,
                })
            })
            .collect(),
    }
}

/// What `account` earned, when it earned anything. The node answers one
/// entry per account asked, with nothing for one that earned nothing.
pub fn stake_reward(
    account: Pubkey,
    epoch: u64,
    amount: u64,
    post_balance: u64,
    commission: Option<u8>,
) -> Option<StakeReward> {
    (amount > 0).then_some(StakeReward {
        account,
        epoch,
        amount,
        post_balance,
        commission,
    })
}

/// One entry of `getSignaturesForAddress`, from either client.
pub fn address_signature(
    signature: &str,
    slot: u64,
    block_time: Option<i64>,
    err: Option<String>,
) -> Result<AddressSignature, RpcError> {
    let signature: Signature = signature
        .parse()
        .map_err(|_| RpcError::Message("The node returned an unreadable signature.".into()))?;
    Ok(AddressSignature {
        signature,
        slot,
        block_time,
        err,
    })
}

/// A `getTransaction` answer in base64, from either client.
pub fn landed_tx(answer: EncodedConfirmedTransactionWithStatusMeta) -> Result<LandedTx, RpcError> {
    let unreadable = || RpcError::Message("The node returned a transaction it cannot read.".into());
    let wire = match &answer.transaction.transaction {
        EncodedTransaction::Binary(blob, TransactionBinaryEncoding::Base64) => {
            STANDARD.decode(blob).map_err(|_| unreadable())?
        }
        EncodedTransaction::Binary(blob, TransactionBinaryEncoding::Base58)
        | EncodedTransaction::LegacyBinary(blob) => {
            bs58::decode(blob).into_vec().map_err(|_| unreadable())?
        }
        _ => return Err(unreadable()),
    };
    let Some(meta) = answer.transaction.meta else {
        return Ok(LandedTx {
            slot: answer.slot,
            block_time: answer.block_time,
            wire,
            err: None,
            fee: 0,
            loaded_writable: Vec::new(),
            loaded_readonly: Vec::new(),
            pre_balances: Vec::new(),
            post_balances: Vec::new(),
            token_balances: Vec::new(),
            logs: Vec::new(),
        });
    };
    let keys = |list: &[String]| -> Result<Vec<Pubkey>, RpcError> {
        list.iter()
            .map(|key| key.parse().map_err(|_| unreadable()))
            .collect()
    };
    let (loaded_writable, loaded_readonly) = match meta
        .loaded_addresses
        .as_ref()
        .map(|loaded| Ok::<_, RpcError>((keys(&loaded.writable)?, keys(&loaded.readonly)?)))
    {
        Some(loaded) => loaded?,
        None => (Vec::new(), Vec::new()),
    };
    let pre = meta.pre_token_balances.map(|list| list).unwrap_or_default();
    let post = meta
        .post_token_balances
        .map(|list| list)
        .unwrap_or_default();
    Ok(LandedTx {
        slot: answer.slot,
        block_time: answer.block_time,
        wire,
        err: meta.err.map(|err| err.to_string()),
        fee: meta.fee,
        loaded_writable,
        loaded_readonly,
        pre_balances: meta.pre_balances,
        post_balances: meta.post_balances,
        token_balances: token_changes(&pre, &post),
        logs: meta.log_messages.map(|logs| logs).unwrap_or_default(),
    })
}

/// Each token account's balance before and after, by its index. One that is
/// on one side only held nothing on the other.
fn token_changes(
    pre: &[UiTransactionTokenBalance],
    post: &[UiTransactionTokenBalance],
) -> Vec<TokenBalanceChange> {
    let amount = |balance: &UiTransactionTokenBalance| {
        balance.ui_token_amount.amount.parse::<u64>().unwrap_or(0)
    };
    let mut out: Vec<TokenBalanceChange> = Vec::new();
    for (is_pre, list) in [(true, pre), (false, post)] {
        for balance in list {
            let Ok(mint) = balance.mint.parse::<Pubkey>() else {
                continue;
            };
            let at = match out
                .iter()
                .position(|known| known.account_index == balance.account_index)
            {
                Some(at) => at,
                None => {
                    out.push(TokenBalanceChange {
                        account_index: balance.account_index,
                        mint,
                        owner: None,
                        decimals: balance.ui_token_amount.decimals,
                        pre: 0,
                        post: 0,
                    });
                    out.len() - 1
                }
            };
            let entry = &mut out[at];
            if entry.owner.is_none() {
                entry.owner = balance
                    .owner
                    .as_ref()
                    .map(|owner| owner.parse().ok())
                    .flatten();
            }
            if is_pre {
                entry.pre = amount(balance);
            } else {
                entry.post = amount(balance);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use solana_transaction_status_client_types::UiCompiledInstruction;

    #[test]
    fn a_simulation_result_becomes_a_reply() {
        let account = |lamports: u64| UiAccount {
            lamports,
            data: UiAccountData::Binary(
                String::new(),
                solana_account_decoder_client_types::UiAccountEncoding::Base64,
            ),
            owner: Pubkey::default().to_string(),
            executable: false,
            rent_epoch: 0,
            space: Some(0),
        };
        let reply = simulation_reply(SimulationParts {
            err: Some(serde_json::Value::Null),
            logs: Some(vec!["one".into()]),
            units_consumed: Some(150),
            loaded_accounts_data_size: Some(142),
            accounts: Some(vec![Some(account(7)), None]),
            inner_instructions: Some(vec![UiInnerInstructions {
                index: 1,
                instructions: vec![
                    UiInstruction::Compiled(UiCompiledInstruction {
                        program_id_index: 2,
                        accounts: vec![0, 1],
                        data: bs58::encode([2, 0, 0, 0]).into_string(),
                        stack_height: Some(2),
                    }),
                    UiInstruction::Compiled(UiCompiledInstruction {
                        program_id_index: 2,
                        accounts: vec![],
                        data: "not base58 0OIl".into(),
                        stack_height: None,
                    }),
                ],
            }]),
            return_data: Some((Pubkey::default().to_string(), STANDARD.encode([7, 8]))),
        });
        assert_eq!(reply.err, None);
        assert_eq!(reply.balances, vec![7, 0]);
        assert_eq!(reply.inner.len(), 1);
        assert_eq!(reply.inner[0].instruction, 1);
        assert_eq!(reply.inner[0].calls.len(), 1);
        assert_eq!(reply.inner[0].calls[0].data, vec![2, 0, 0, 0]);
        assert_eq!(reply.return_data, Some((Pubkey::default(), vec![7, 8])));
    }

    #[test]
    fn a_reward_of_nothing_is_no_reward() {
        let account = Pubkey::new_from_array([3; 32]);
        assert_eq!(stake_reward(account, 849, 0, 9, None), None);
        let reward = stake_reward(account, 849, 2500, 1_002_500, Some(5)).unwrap();
        assert_eq!((reward.amount, reward.commission), (2500, Some(5)));
    }

    #[test]
    fn only_a_timeout_is_told_apart() {
        assert!(matches!(
            rpc_error("operation timed out"),
            RpcError::Timeout
        ));
        assert!(matches!(rpc_error("HTTP 403"), RpcError::Message(_)));
        for limited in [
            "HTTP status client error (429 Too Many Requests)",
            "RPC response error -32429: limit reached",
            "Rate limit exceeded",
        ] {
            assert_eq!(rpc_error(limited), RpcError::RateLimited, "{limited}");
        }
        // A program's logs can say anything, and the words can pass by chance.
        let logged = "RPC response error -32002: Transaction simulation failed; 1 log messages:\n  Program log: too many requests";
        assert!(matches!(rpc_error(logged), RpcError::Message(_)));
        assert!(matches!(
            rpc_error("custom program error: 0x429"),
            RpcError::Message(_)
        ));
    }

    #[test]
    fn a_request_that_never_left_is_told_from_one_that_got_no_answer() {
        for never_left in ["connection refused", "dns error: no such host"] {
            assert!(matches!(rpc_error(never_left), RpcError::Unreachable(_)));
        }
        for no_answer in [
            "error sending request for url (http://node/)",
            "TypeError: Failed to fetch",
            "NetworkError when attempting to fetch resource.",
        ] {
            assert!(matches!(rpc_error(no_answer), RpcError::NoAnswer(_)));
        }
    }

    #[test]
    fn a_node_behind_the_slot_asked_is_told_apart() {
        assert!(matches!(
            rpc_error("RPC response error -32016: Minimum context slot has not been reached; "),
            RpcError::Behind
        ));
    }
}
