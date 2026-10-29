//! What the chain and OtterSec say about each program now.

use crate::osec::{self, Verification};
use solana_pubkey::Pubkey;
use wallet_core::program_code::{
    program_account, program_code, ProgramAccount, ProgramCode, BPF_LOADER_UPGRADEABLE_ID,
};
use wallet_core::{
    anchor_idl_address, idl_bytes_from_anchor_account, idl_bytes_from_program_metadata,
    program_metadata_idl_address, sha256_bytes, IdlNetwork, RawAccount, SolanaRpc,
};
use wallet_rpc::Rpc;

/// Program-data accounts are up to a few megabytes each. Ask for a few at a
/// time so one answer stays small enough for a public node.
const PROGRAM_DATA_PER_REQUEST: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Code {
    Native,
    Program(ProgramCode),
}

#[derive(Debug)]
pub struct Observed {
    pub code: Result<Code, String>,
    /// SHA-256 of the IDL in the canonical program-metadata account.
    pub metadata_idl: Result<Option<[u8; 32]>, String>,
    /// SHA-256 of the IDL in the Anchor IDL account.
    pub anchor_idl: Result<Option<[u8; 32]>, String>,
    /// `None` when OtterSec was not asked: not mainnet, a native program, or
    /// turned off.
    pub builds: Option<Result<Vec<Verification>, String>>,
}

impl Observed {
    /// The build OtterSec verified for the code running now.
    pub fn verified(&self) -> Option<&Verification> {
        match (&self.code, &self.builds) {
            (Ok(Code::Program(code)), Some(Ok(builds))) => {
                osec::verified_build(builds, &code.hash, code.authority.as_ref())
            }
            _ => None,
        }
    }
}

pub async fn observe(
    rpc: &Rpc,
    osec: Option<&osec::Client>,
    network: IdlNetwork,
    programs: &[Pubkey],
) -> Result<Vec<Observed>, String> {
    let accounts = rpc
        .get_accounts(programs)
        .await
        .map_err(|err| format!("{network:?} program accounts: {err}"))?;

    let mut codes: Vec<Result<Code, String>> = Vec::with_capacity(programs.len());
    let mut data_accounts: Vec<(usize, Pubkey)> = Vec::new();
    for (index, (program_id, account)) in programs.iter().zip(&accounts).enumerate() {
        let Some(account) = account else {
            codes.push(Err("no account at this address".into()));
            continue;
        };
        codes.push(
            match program_account(
                program_id,
                &account.owner,
                account.executable,
                &account.data,
            ) {
                Ok(ProgramAccount::Native) => Ok(Code::Native),
                Ok(ProgramAccount::Code) => program_code(program_id, &account.owner, &account.data)
                    .map(Code::Program)
                    .map_err(|err| err.to_string()),
                Ok(ProgramAccount::Data(address)) => {
                    data_accounts.push((index, address));
                    Err("program data not read".into())
                }
                Err(err) => Err(err.to_string()),
            },
        );
    }
    for batch in data_accounts.chunks(PROGRAM_DATA_PER_REQUEST) {
        let addresses: Vec<Pubkey> = batch.iter().map(|(_, address)| *address).collect();
        let found = rpc
            .get_accounts(&addresses)
            .await
            .map_err(|err| format!("{network:?} program data: {err}"))?;
        for ((index, address), account) in batch.iter().zip(found) {
            let program_id = &programs[*index];
            codes[*index] = match account {
                None => Err(format!("its program data account {address} is missing")),
                Some(account) => {
                    program_code(program_id, &BPF_LOADER_UPGRADEABLE_ID, &account.data)
                        .map(Code::Program)
                        .map_err(|err| err.to_string())
                }
            };
        }
    }

    let metadata = idl_hashes(
        rpc,
        programs,
        |program_id| program_metadata_idl_address(program_id).map_err(|err| err.to_string()),
        |account| idl_bytes_from_program_metadata(&account.data).map_err(|err| err.to_string()),
    )
    .await
    .map_err(|err| format!("{network:?} program-metadata IDLs: {err}"))?;
    let anchor = idl_hashes(
        rpc,
        programs,
        |program_id| anchor_idl_address(program_id).map_err(|err| err.to_string()),
        |account| idl_bytes_from_anchor_account(&account.data).map_err(|err| err.to_string()),
    )
    .await
    .map_err(|err| format!("{network:?} Anchor IDLs: {err}"))?;

    let mut observed = Vec::with_capacity(programs.len());
    for (((program_id, code), metadata_idl), anchor_idl) in
        programs.iter().zip(codes).zip(metadata).zip(anchor)
    {
        let builds = match (osec, &code) {
            (Some(client), Ok(Code::Program(_))) if network == IdlNetwork::Mainnet => {
                Some(client.builds(program_id).await)
            }
            _ => None,
        };
        observed.push(Observed {
            code,
            metadata_idl,
            anchor_idl,
            builds,
        });
    }
    Ok(observed)
}

/// The SHA-256 of the IDL each program keeps in the account `address` names.
async fn idl_hashes(
    rpc: &Rpc,
    programs: &[Pubkey],
    address: impl Fn(&Pubkey) -> Result<Pubkey, String>,
    decode: impl Fn(&RawAccount) -> Result<Vec<u8>, String>,
) -> Result<Vec<Result<Option<[u8; 32]>, String>>, String> {
    let addresses: Vec<Result<Pubkey, String>> = programs.iter().map(address).collect();
    let asked: Vec<Pubkey> = addresses.iter().filter_map(|a| a.clone().ok()).collect();
    let mut found = rpc
        .get_accounts(&asked)
        .await
        .map_err(|err| err.to_string())?
        .into_iter();
    Ok(addresses
        .into_iter()
        .map(|address| {
            address?;
            match found.next().flatten() {
                None => Ok(None),
                Some(account) => decode(&account).map(|bytes| Some(sha256_bytes(&bytes))),
            }
        })
        .collect())
}
