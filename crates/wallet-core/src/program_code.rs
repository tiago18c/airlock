//! Which code a program account runs, read from the accounts a node returns.
//!
//! The hash is the one `solana-verify get-program-hash` prints and the
//! OtterSec verifier records: SHA-256 of the executable with its trailing
//! zero bytes cut, so the same build hashes the same whatever size of
//! account it was deployed into. Nothing here opens a socket.

use sha2::{Digest, Sha256};
use solana_pubkey::Pubkey;
use thiserror::Error;

pub const NATIVE_LOADER_ID: Pubkey =
    Pubkey::from_str_const("NativeLoader1111111111111111111111111111111");
pub const BPF_LOADER_DEPRECATED_ID: Pubkey =
    Pubkey::from_str_const("BPFLoader1111111111111111111111111111111111");
pub const BPF_LOADER_ID: Pubkey =
    Pubkey::from_str_const("BPFLoader2111111111111111111111111111111111");
pub const BPF_LOADER_UPGRADEABLE_ID: Pubkey =
    Pubkey::from_str_const("BPFLoaderUpgradeab1e11111111111111111111111");
pub const LOADER_V4_ID: Pubkey =
    Pubkey::from_str_const("LoaderV411111111111111111111111111111111111");

/// Tag, deploy slot, then an optional authority that always takes 33 bytes.
const PROGRAM_DATA_HEADER: usize = 4 + 8 + 1 + 32;
/// Deploy slot, authority (or next version), status.
const LOADER_V4_HEADER: usize = 8 + 32 + 8;
const LOADER_V4_FINALIZED: u64 = 2;

/// Where a program's executable lives, read from its program account.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProgramAccount {
    /// Run by the validator itself. There is no executable to hash.
    Native,
    /// The executable is in the program account itself.
    Code,
    /// The executable is in this program-data account (upgradeable loader).
    Data(Pubkey),
}

/// The code a program runs and who may replace it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProgramCode {
    pub hash: [u8; 32],
    /// `None` when nobody can change the program.
    pub authority: Option<Pubkey>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ProgramCodeError {
    #[error("{0} is not an executable account")]
    NotExecutable(Pubkey),
    #[error("{0} is owned by {1}, which is not a program loader")]
    UnknownLoader(Pubkey, Pubkey),
    #[error("the account of {0} is not laid out as its loader writes it")]
    Malformed(Pubkey),
}

/// Where the executable of `program_id` is, from its account's owner and data.
pub fn program_account(
    program_id: &Pubkey,
    owner: &Pubkey,
    executable: bool,
    data: &[u8],
) -> Result<ProgramAccount, ProgramCodeError> {
    if *owner == NATIVE_LOADER_ID {
        return Ok(ProgramAccount::Native);
    }
    if !executable {
        return Err(ProgramCodeError::NotExecutable(*program_id));
    }
    match *owner {
        BPF_LOADER_ID | BPF_LOADER_DEPRECATED_ID | LOADER_V4_ID => Ok(ProgramAccount::Code),
        BPF_LOADER_UPGRADEABLE_ID => match data {
            [2, 0, 0, 0, address @ ..] if address.len() >= 32 => {
                let address: [u8; 32] = address[..32].try_into().expect("32 bytes");
                Ok(ProgramAccount::Data(Pubkey::new_from_array(address)))
            }
            _ => Err(ProgramCodeError::Malformed(*program_id)),
        },
        other => Err(ProgramCodeError::UnknownLoader(*program_id, other)),
    }
}

/// The code in the account that holds `program_id`'s executable: the program
/// account for [`ProgramAccount::Code`], its program-data account for
/// [`ProgramAccount::Data`]. `owner` is the program account's owner.
pub fn program_code(
    program_id: &Pubkey,
    owner: &Pubkey,
    data: &[u8],
) -> Result<ProgramCode, ProgramCodeError> {
    let malformed = || ProgramCodeError::Malformed(*program_id);
    let (authority, executable) = match *owner {
        BPF_LOADER_ID | BPF_LOADER_DEPRECATED_ID => (None, data),
        BPF_LOADER_UPGRADEABLE_ID => {
            if data.len() < PROGRAM_DATA_HEADER || data[..4] != [3, 0, 0, 0] {
                return Err(malformed());
            }
            let authority = match data[12] {
                0 => None,
                1 => Some(pubkey_at(data, 13)),
                _ => return Err(malformed()),
            };
            (authority, &data[PROGRAM_DATA_HEADER..])
        }
        LOADER_V4_ID => {
            if data.len() < LOADER_V4_HEADER {
                return Err(malformed());
            }
            let status = u64::from_le_bytes(data[40..48].try_into().expect("8 bytes"));
            // A finalized program's address field names its next version,
            // not someone who can change it.
            let authority = (status != LOADER_V4_FINALIZED).then(|| pubkey_at(data, 8));
            (authority, &data[LOADER_V4_HEADER..])
        }
        other => return Err(ProgramCodeError::UnknownLoader(*program_id, other)),
    };
    Ok(ProgramCode {
        hash: executable_hash(executable),
        authority,
    })
}

/// SHA-256 of `executable` without its trailing zero bytes.
pub fn executable_hash(executable: &[u8]) -> [u8; 32] {
    let end = executable
        .iter()
        .rposition(|byte| *byte != 0)
        .map_or(0, |last| last + 1);
    Sha256::digest(&executable[..end]).into()
}

fn pubkey_at(data: &[u8], offset: usize) -> Pubkey {
    Pubkey::new_from_array(data[offset..offset + 32].try_into().expect("32 bytes"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROGRAM: Pubkey = Pubkey::new_from_array([7; 32]);
    const ELF: &[u8] = b"\x7fELF some code";

    fn program_data(authority: Option<Pubkey>, padding: usize) -> Vec<u8> {
        let mut data = vec![3, 0, 0, 0];
        data.extend(42u64.to_le_bytes());
        match authority {
            Some(key) => {
                data.push(1);
                data.extend(key.to_bytes());
            }
            None => data.extend([0; 33]),
        }
        data.extend(ELF);
        data.extend(vec![0; padding]);
        data
    }

    #[test]
    fn the_hash_ignores_the_zeros_an_account_is_padded_with() {
        let small = program_code(&PROGRAM, &BPF_LOADER_UPGRADEABLE_ID, &program_data(None, 3));
        let large = program_code(
            &PROGRAM,
            &BPF_LOADER_UPGRADEABLE_ID,
            &program_data(None, 900),
        );
        assert_eq!(small, large);
        assert_eq!(small.unwrap().hash, <[u8; 32]>::from(Sha256::digest(ELF)));
    }

    #[test]
    fn an_upgradeable_program_names_its_authority_until_it_is_frozen() {
        let key = Pubkey::new_from_array([9; 32]);
        let code = program_code(
            &PROGRAM,
            &BPF_LOADER_UPGRADEABLE_ID,
            &program_data(Some(key), 0),
        )
        .unwrap();
        assert_eq!(code.authority, Some(key));
        let frozen =
            program_code(&PROGRAM, &BPF_LOADER_UPGRADEABLE_ID, &program_data(None, 0)).unwrap();
        assert_eq!(frozen.authority, None);
        assert_eq!(code.hash, frozen.hash);
    }

    #[test]
    fn the_program_account_says_where_the_executable_is() {
        let data_address = Pubkey::new_from_array([5; 32]);
        let mut account = vec![2, 0, 0, 0];
        account.extend(data_address.to_bytes());
        assert_eq!(
            program_account(&PROGRAM, &BPF_LOADER_UPGRADEABLE_ID, true, &account),
            Ok(ProgramAccount::Data(data_address))
        );
        assert_eq!(
            program_account(&PROGRAM, &BPF_LOADER_ID, true, ELF),
            Ok(ProgramAccount::Code)
        );
        assert_eq!(
            program_account(&PROGRAM, &NATIVE_LOADER_ID, true, b"system_program"),
            Ok(ProgramAccount::Native)
        );
        assert_eq!(
            program_account(&PROGRAM, &BPF_LOADER_ID, false, ELF),
            Err(ProgramCodeError::NotExecutable(PROGRAM))
        );
    }

    #[test]
    fn a_finalized_loader_v4_program_has_no_authority() {
        let key = Pubkey::new_from_array([9; 32]);
        let account = |status: u64| {
            let mut data = 1u64.to_le_bytes().to_vec();
            data.extend(key.to_bytes());
            data.extend(status.to_le_bytes());
            data.extend(ELF);
            data
        };
        let deployed = program_code(&PROGRAM, &LOADER_V4_ID, &account(1)).unwrap();
        assert_eq!(deployed.authority, Some(key));
        let finalized = program_code(&PROGRAM, &LOADER_V4_ID, &account(2)).unwrap();
        assert_eq!(finalized.authority, None);
        assert_eq!(deployed.hash, finalized.hash);
    }

    #[test]
    fn a_program_data_account_with_a_bad_header_is_refused() {
        let mut data = program_data(None, 0);
        data[0] = 2;
        assert_eq!(
            program_code(&PROGRAM, &BPF_LOADER_UPGRADEABLE_ID, &data),
            Err(ProgramCodeError::Malformed(PROGRAM))
        );
    }
}
