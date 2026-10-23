//! Reading the IDL a program publishes on chain.
//!
//! An IDL is only ever read from its on-chain accounts: the program-metadata
//! account, or the legacy Anchor IDL account. Nothing here fetches a URL.

use solana_pubkey::Pubkey;
use thiserror::Error;
use wallet_core::{
    anchor_idl_address, idl_bytes_from_anchor_account, idl_bytes_from_program_metadata,
    program_metadata_idl_address, IdlError, IdlOrigin, RpcError, SolanaRpc, MAX_IDL_BYTES,
};

/// Why an on-chain IDL could not be read.
#[derive(Debug, Error)]
pub enum IdlReadError {
    #[error(transparent)]
    Rpc(#[from] RpcError),
    #[error(transparent)]
    Idl(#[from] IdlError),
    /// The node answered, but not with a usable IDL.
    #[error("{0}")]
    Remote(String),
}

/// What one program publishes: its IDL and which account it came from, or
/// `None` when it publishes neither.
pub type Published = Result<Option<(Vec<u8>, IdlOrigin)>, IdlReadError>;

/// The IDL `program_id` publishes, and which account it came from.
/// Program-metadata is tried first. A missing or unreadable account falls
/// back to the Anchor IDL account. `None` when the program publishes neither.
pub async fn published(rpc: &dyn SolanaRpc, program_id: &Pubkey) -> Published {
    published_many(rpc, &[*program_id])
        .await?
        .pop()
        .unwrap_or(Ok(None))
}

/// [`published`] for each of `program_ids`, in the order asked, in two
/// requests at most: every program-metadata account at once, then the Anchor
/// IDL account of each program that had no usable one.
pub async fn published_many(
    rpc: &dyn SolanaRpc,
    program_ids: &[Pubkey],
) -> Result<Vec<Published>, RpcError> {
    let mut found: Vec<Option<Published>> = program_ids.iter().map(|_| None).collect();

    let mut asked = Vec::new();
    for (index, program_id) in program_ids.iter().enumerate() {
        match program_metadata_idl_address(program_id) {
            Ok(address) => asked.push((index, address)),
            Err(err) => found[index] = Some(Err(err.into())),
        }
    }
    let addresses: Vec<Pubkey> = asked.iter().map(|(_, address)| *address).collect();
    for ((index, _), account) in asked.iter().zip(rpc.get_accounts(&addresses).await?) {
        let Some(bytes) =
            account.and_then(|account| idl_bytes_from_program_metadata(&account.data).ok())
        else {
            continue;
        };
        found[*index] = Some(if bytes.len() > MAX_IDL_BYTES {
            Err(IdlReadError::Remote(
                "Program-metadata IDL is larger than 2 MB.".into(),
            ))
        } else {
            Ok(Some((bytes, IdlOrigin::ProgramMetadata)))
        });
    }

    let mut asked = Vec::new();
    for (index, program_id) in program_ids.iter().enumerate() {
        if found[index].is_some() {
            continue;
        }
        match anchor_idl_address(program_id) {
            Ok(address) => asked.push((index, address)),
            Err(err) => found[index] = Some(Err(err.into())),
        }
    }
    let addresses: Vec<Pubkey> = asked.iter().map(|(_, address)| *address).collect();
    for ((index, _), account) in asked.iter().zip(rpc.get_accounts(&addresses).await?) {
        found[*index] = Some(match account {
            None => Ok(None),
            Some(account) => match idl_bytes_from_anchor_account(&account.data) {
                Ok(bytes) if bytes.len() > MAX_IDL_BYTES => Err(IdlReadError::Remote(
                    "Anchor IDL is larger than 2 MB.".into(),
                )),
                Ok(bytes) => Ok(Some((bytes, IdlOrigin::Anchor))),
                Err(err) => Err(err.into()),
            },
        });
    }

    Ok(found
        .into_iter()
        .map(|published| published.unwrap_or(Ok(None)))
        .collect())
}

/// The on-chain bytes to compare a stored IDL with. An IDL taken from
/// program metadata is compared with that account, any other with the Anchor
/// IDL account.
pub async fn counterpart(
    rpc: &dyn SolanaRpc,
    program_id: &Pubkey,
    origin: &IdlOrigin,
) -> Result<Vec<u8>, IdlReadError> {
    match origin {
        IdlOrigin::ProgramMetadata => {
            let address = program_metadata_idl_address(program_id)?;
            let Some(account) = rpc.get_account(&address).await? else {
                return Err(IdlReadError::Remote(
                    "No program-metadata IDL for this program.".into(),
                ));
            };
            let bytes = idl_bytes_from_program_metadata(&account.data)?;
            if bytes.len() > MAX_IDL_BYTES {
                return Err(IdlReadError::Remote(
                    "Program-metadata IDL is larger than 2 MB.".into(),
                ));
            }
            Ok(bytes)
        }
        IdlOrigin::Builtin | IdlOrigin::File | IdlOrigin::Anchor => {
            let address = anchor_idl_address(program_id)?;
            let Some(account) = rpc.get_account(&address).await? else {
                return Err(IdlReadError::Remote(
                    "No on-chain IDL account for this program.".into(),
                ));
            };
            Ok(idl_bytes_from_anchor_account(&account.data)?)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{pk, FakeRpc};
    use wallet_core::{anchor_idl_account_data, program_metadata_account_data, RawAccount};

    const NEW: &[u8] = br#"{"name":"from metadata"}"#;
    const OLD: &[u8] = br#"{"name":"from anchor"}"#;

    fn holding(data: Vec<u8>) -> RawAccount {
        RawAccount {
            lamports: 1,
            data,
            owner: pk(200),
            executable: false,
            rent_epoch: 0,
        }
    }

    fn metadata_at(rpc: FakeRpc, program: &Pubkey, data: Vec<u8>) -> FakeRpc {
        rpc.with_account(
            program_metadata_idl_address(program).unwrap(),
            holding(data),
        )
    }

    fn anchor_at(rpc: FakeRpc, program: &Pubkey, data: Vec<u8>) -> FakeRpc {
        rpc.with_account(anchor_idl_address(program).unwrap(), holding(data))
    }

    fn read(rpc: &FakeRpc, program: &Pubkey) -> Result<Option<(Vec<u8>, IdlOrigin)>, IdlReadError> {
        pollster::block_on(published(rpc, program))
    }

    #[test]
    fn program_metadata_is_read_before_the_anchor_account() {
        let program = pk(1);
        let rpc = metadata_at(
            FakeRpc::default(),
            &program,
            program_metadata_account_data(NEW),
        );
        let rpc = anchor_at(rpc, &program, anchor_idl_account_data(OLD));

        let found = read(&rpc, &program).unwrap().unwrap();
        assert_eq!(found, (NEW.to_vec(), IdlOrigin::ProgramMetadata));
        assert_eq!(rpc.asked(), 1, "the Anchor account is not asked for");
    }

    #[test]
    fn a_missing_or_unreadable_metadata_account_falls_back_to_anchor() {
        let program = pk(1);
        let rpc = anchor_at(FakeRpc::default(), &program, anchor_idl_account_data(OLD));
        let found = read(&rpc, &program).unwrap().unwrap();
        assert_eq!(found, (OLD.to_vec(), IdlOrigin::Anchor));

        let rpc = metadata_at(rpc, &program, vec![9; 120]);
        let found = read(&rpc, &program).unwrap().unwrap();
        assert_eq!(found, (OLD.to_vec(), IdlOrigin::Anchor));
    }

    #[test]
    fn a_program_that_publishes_nothing_costs_two_account_reads_and_nothing_else() {
        let rpc = FakeRpc::default();
        assert!(matches!(read(&rpc, &pk(1)), Ok(None)));
        assert_eq!(rpc.asked(), 2);
    }

    #[test]
    fn many_programs_cost_two_requests_in_all() {
        let (first, second, third) = (pk(1), pk(2), pk(3));
        let rpc = metadata_at(
            FakeRpc::default(),
            &first,
            program_metadata_account_data(NEW),
        );
        let rpc = anchor_at(rpc, &second, anchor_idl_account_data(OLD));
        let found = pollster::block_on(published_many(&rpc, &[first, second, third])).unwrap();
        assert_eq!(rpc.asked(), 2);
        assert!(matches!(
            &found[0],
            Ok(Some((_, IdlOrigin::ProgramMetadata)))
        ));
        assert!(matches!(&found[1], Ok(Some((_, IdlOrigin::Anchor)))));
        assert!(matches!(&found[2], Ok(None)));
    }

    #[test]
    fn an_anchor_account_that_is_not_an_idl_is_an_error_not_an_absence() {
        let program = pk(1);
        let rpc = anchor_at(FakeRpc::default(), &program, vec![9; 120]);
        assert!(matches!(read(&rpc, &program), Err(IdlReadError::Idl(_))));
    }

    #[test]
    fn an_idl_over_the_size_limit_is_refused() {
        let program = pk(1);
        let big = vec![b' '; MAX_IDL_BYTES + 1];
        let rpc = metadata_at(
            FakeRpc::default(),
            &program,
            program_metadata_account_data(&big),
        );
        let err = read(&rpc, &program).unwrap_err();
        assert!(matches!(&err, IdlReadError::Remote(text) if text.contains("2 MB")));
    }

    #[test]
    fn a_node_that_does_not_answer_is_a_node_error() {
        let rpc = FakeRpc {
            offline: true,
            ..FakeRpc::default()
        };
        assert!(matches!(
            read(&rpc, &pk(1)),
            Err(IdlReadError::Rpc(RpcError::Offline))
        ));
    }

    #[test]
    fn a_stored_idl_is_compared_with_the_account_it_came_from() {
        let program = pk(1);
        let rpc = metadata_at(
            FakeRpc::default(),
            &program,
            program_metadata_account_data(NEW),
        );
        let rpc = anchor_at(rpc, &program, anchor_idl_account_data(OLD));
        let compare = |origin| pollster::block_on(counterpart(&rpc, &program, &origin));

        assert_eq!(compare(IdlOrigin::ProgramMetadata).unwrap(), NEW);
        // A file, an embedded IDL and an Anchor IDL are compared with the Anchor account.
        for origin in [IdlOrigin::Anchor, IdlOrigin::File, IdlOrigin::Builtin] {
            assert_eq!(compare(origin).unwrap(), OLD);
        }
    }

    #[test]
    fn comparing_does_not_fall_back_to_the_other_account() {
        let program = pk(1);
        let only_anchor = anchor_at(FakeRpc::default(), &program, anchor_idl_account_data(OLD));
        let err = pollster::block_on(counterpart(
            &only_anchor,
            &program,
            &IdlOrigin::ProgramMetadata,
        ))
        .unwrap_err();
        assert!(matches!(&err, IdlReadError::Remote(text) if text.contains("program-metadata")));

        let only_metadata = metadata_at(
            FakeRpc::default(),
            &program,
            program_metadata_account_data(NEW),
        );
        let err = pollster::block_on(counterpart(&only_metadata, &program, &IdlOrigin::File))
            .unwrap_err();
        assert!(matches!(&err, IdlReadError::Remote(text) if text.contains("on-chain IDL")));
    }
}
