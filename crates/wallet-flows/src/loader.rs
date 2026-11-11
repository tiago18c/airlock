//! The program-data account an upgradeable program names, read from a node.
//!
//! The account is checked when it is read; hashing the code it holds is
//! left to [`ProgramData::read`], which a caller with a window runs away
//! from it, since the account holds up to 10 MiB.

use solana_pubkey::Pubkey;
use thiserror::Error;
use wallet_core::program_code::{
    loader_state, DeployedProgram, LoaderAccount, LoaderState, BPF_LOADER_UPGRADEABLE_ID,
};
use wallet_core::{RawAccount, RpcError, SolanaRpc};

/// Why a program's program-data account says nothing of its code.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ProgramDataError {
    #[error(transparent)]
    Rpc(#[from] RpcError),
    #[error("No account is at {0}: the program was closed.")]
    Missing(Pubkey),
    #[error("{0} is owned by {1}, not by the upgradeable loader.")]
    NotLoader(Pubkey, Pubkey),
    #[error("{0} is not a program data account.")]
    NotProgramData(Pubkey),
}

/// A program-data account, checked to be one.
#[derive(Debug)]
pub struct ProgramData {
    pub address: Pubkey,
    account: RawAccount,
}

impl ProgramData {
    /// The slot, the upgrade authority and the hash of the code. It hashes
    /// the whole account.
    pub fn read(&self) -> Result<DeployedProgram, ProgramDataError> {
        match check(&self.address, &self.account)?.read() {
            LoaderAccount::ProgramData(deployed) => Ok(deployed),
            _ => Err(ProgramDataError::NotProgramData(self.address)),
        }
    }
}

/// Read the program-data account at `address`, the one a program account
/// names. Refused when there is none, or the account is not a program-data
/// account of the upgradeable loader.
pub async fn program_data(
    rpc: &dyn SolanaRpc,
    address: Pubkey,
) -> Result<ProgramData, ProgramDataError> {
    let account = rpc
        .get_account(&address)
        .await?
        .ok_or(ProgramDataError::Missing(address))?;
    check(&address, &account)?;
    Ok(ProgramData { address, account })
}

fn check<'a>(
    address: &Pubkey,
    account: &'a RawAccount,
) -> Result<LoaderState<'a>, ProgramDataError> {
    if account.owner != BPF_LOADER_UPGRADEABLE_ID {
        return Err(ProgramDataError::NotLoader(*address, account.owner));
    }
    match loader_state(&account.owner, &account.data) {
        Some(state @ LoaderState::ProgramData { .. }) => Ok(state),
        _ => Err(ProgramDataError::NotProgramData(*address)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{pk, FakeRpc};
    use wallet_core::program_code::executable_hash;

    const CODE: &[u8] = b"\x7fELF code";

    fn program_data_account(authority: Option<Pubkey>) -> RawAccount {
        let mut data = vec![3, 0, 0, 0];
        data.extend(77u64.to_le_bytes());
        match authority {
            Some(key) => {
                data.push(1);
                data.extend(key.to_bytes());
            }
            None => data.extend([0; 33]),
        }
        data.extend(CODE);
        data.extend([0; 100]);
        RawAccount {
            lamports: 1,
            data,
            owner: BPF_LOADER_UPGRADEABLE_ID,
            executable: false,
            rent_epoch: u64::MAX,
        }
    }

    #[test]
    fn a_program_data_account_reads_as_its_slot_authority_and_hash() {
        let rpc = FakeRpc::default().with_account(pk(3), program_data_account(Some(pk(4))));
        let read = pollster::block_on(program_data(&rpc, pk(3))).unwrap();
        assert_eq!(read.address, pk(3));
        assert_eq!(
            read.read(),
            Ok(DeployedProgram {
                slot: 77,
                authority: Some(pk(4)),
                hash: executable_hash(CODE),
            })
        );

        let rpc = FakeRpc::default().with_account(pk(3), program_data_account(None));
        let read = pollster::block_on(program_data(&rpc, pk(3))).unwrap();
        assert_eq!(read.read().unwrap().authority, None);
    }

    #[test]
    fn a_closed_program_has_no_program_data() {
        let rpc = FakeRpc::default();
        assert_eq!(
            pollster::block_on(program_data(&rpc, pk(3))).unwrap_err(),
            ProgramDataError::Missing(pk(3))
        );
    }

    #[test]
    fn an_account_of_another_owner_is_refused() {
        let mut account = program_data_account(None);
        account.owner = pk(9);
        let rpc = FakeRpc::default().with_account(pk(3), account);
        assert_eq!(
            pollster::block_on(program_data(&rpc, pk(3))).unwrap_err(),
            ProgramDataError::NotLoader(pk(3), pk(9))
        );
    }

    #[test]
    fn a_loader_account_in_another_state_is_refused() {
        let mut account = program_data_account(None);
        account.data[0] = 1; // a buffer
        let rpc = FakeRpc::default().with_account(pk(3), account);
        assert_eq!(
            pollster::block_on(program_data(&rpc, pk(3))).unwrap_err(),
            ProgramDataError::NotProgramData(pk(3))
        );

        let mut account = program_data_account(None);
        account.data.truncate(20);
        let rpc = FakeRpc::default().with_account(pk(3), account);
        assert_eq!(
            pollster::block_on(program_data(&rpc, pk(3))).unwrap_err(),
            ProgramDataError::NotProgramData(pk(3))
        );
    }

    #[test]
    fn a_node_that_cannot_be_asked_says_so() {
        let rpc = FakeRpc {
            offline: true,
            ..FakeRpc::default()
        };
        assert_eq!(
            pollster::block_on(program_data(&rpc, pk(3))).unwrap_err(),
            ProgramDataError::Rpc(RpcError::Offline)
        );
    }
}
