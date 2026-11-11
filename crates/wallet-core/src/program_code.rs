//! Which code a program account runs, read from the accounts a node returns.
//!
//! The hash is the one `solana-verify get-program-hash` prints and the
//! OtterSec verifier records: SHA-256 of the executable with its trailing
//! zero bytes cut, so the same build hashes the same whatever size of
//! account it was deployed into. A buffer's hash is the same digest of the
//! bytes after its header, which `solana-verify get-buffer-hash` prints, so
//! a buffer and the program deployed from it hash the same. Nothing here
//! opens a socket.

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

/// The upgradeable loader's state tags (`UpgradeableLoaderState`).
const UNINITIALIZED_TAG: u32 = 0;
const BUFFER_TAG: u32 = 1;
const PROGRAM_TAG: u32 = 2;
const PROGRAM_DATA_TAG: u32 = 3;
/// Tag, then an optional authority that always takes 33 bytes.
pub const BUFFER_HEADER: usize = 4 + 1 + 32;
/// Tag, then the program-data address.
const PROGRAM_LEN: usize = 4 + 32;
/// Tag, deploy slot, then an optional authority that always takes 33 bytes.
pub const PROGRAM_DATA_HEADER: usize = 4 + 8 + 1 + 32;
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

/// An account of the upgradeable loader (loader v3), read from its header.
/// The code it holds is not hashed yet: [`LoaderState::read`] does that.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoaderState<'a> {
    Uninitialized,
    /// Code being written, to deploy or upgrade a program from.
    Buffer {
        /// `None` when nobody can write to it any more.
        authority: Option<Pubkey>,
        code: &'a [u8],
    },
    /// A program, whose code is in the program-data account it names.
    Program {
        program_data: Pubkey,
    },
    /// The code of a deployed program.
    ProgramData {
        /// The slot it was last deployed in.
        slot: u64,
        /// `None` when nobody can upgrade the program.
        authority: Option<Pubkey>,
        code: &'a [u8],
    },
}

/// An account of the upgradeable loader, with the code it holds hashed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoaderAccount {
    Uninitialized,
    Buffer {
        /// `None` when nobody can write to it any more.
        authority: Option<Pubkey>,
        /// What `solana-verify get-buffer-hash` prints for it.
        hash: [u8; 32],
    },
    Program {
        program_data: Pubkey,
    },
    ProgramData(DeployedProgram),
}

/// What a program-data account says of the program it holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeployedProgram {
    /// The slot it was last deployed in.
    pub slot: u64,
    /// `None` when nobody can upgrade the program.
    pub authority: Option<Pubkey>,
    /// What `solana-verify get-program-hash` prints for the program.
    pub hash: [u8; 32],
}

/// Read an account owned by `owner` as the upgradeable loader lays it out:
/// `None` when the loader does not own it, or its header is not one the
/// loader writes (too short, an unknown state, an option tag other than 0
/// or 1).
pub fn loader_state<'a>(owner: &Pubkey, data: &'a [u8]) -> Option<LoaderState<'a>> {
    if *owner != BPF_LOADER_UPGRADEABLE_ID {
        return None;
    }
    let tag = u32::from_le_bytes(data.get(..4)?.try_into().ok()?);
    match tag {
        UNINITIALIZED_TAG => Some(LoaderState::Uninitialized),
        BUFFER_TAG if data.len() >= BUFFER_HEADER => Some(LoaderState::Buffer {
            authority: optional_pubkey_at(data, 4)?,
            code: &data[BUFFER_HEADER..],
        }),
        PROGRAM_TAG if data.len() >= PROGRAM_LEN => Some(LoaderState::Program {
            program_data: pubkey_at(data, 4),
        }),
        PROGRAM_DATA_TAG if data.len() >= PROGRAM_DATA_HEADER => Some(LoaderState::ProgramData {
            slot: u64::from_le_bytes(data[4..12].try_into().ok()?),
            authority: optional_pubkey_at(data, 12)?,
            code: &data[PROGRAM_DATA_HEADER..],
        }),
        _ => None,
    }
}

impl LoaderState<'_> {
    /// Hash the code the account holds, if it holds any. A program-data
    /// account holds up to 10 MiB, so a window runs this elsewhere.
    pub fn read(&self) -> LoaderAccount {
        match *self {
            LoaderState::Uninitialized => LoaderAccount::Uninitialized,
            LoaderState::Buffer { authority, code } => LoaderAccount::Buffer {
                authority,
                hash: executable_hash(code),
            },
            LoaderState::Program { program_data } => LoaderAccount::Program { program_data },
            LoaderState::ProgramData {
                slot,
                authority,
                code,
            } => LoaderAccount::ProgramData(DeployedProgram {
                slot,
                authority,
                hash: executable_hash(code),
            }),
        }
    }
}

/// [`loader_state`], with the code hashed.
pub fn loader_account(owner: &Pubkey, data: &[u8]) -> Option<LoaderAccount> {
    loader_state(owner, data).map(|state| state.read())
}

/// The hash of the code in a buffer account's `data`: the digest
/// `solana-verify get-buffer-hash` prints and the Squads clear signing
/// program's `verify_buffer_hash` checks, SHA-256 of the bytes after the
/// 37-byte header with trailing zeros cut. `None` when `data` is not laid out
/// as a buffer.
pub fn buffer_hash(data: &[u8]) -> Option<[u8; 32]> {
    match loader_state(&BPF_LOADER_UPGRADEABLE_ID, data)? {
        LoaderState::Buffer { code, .. } => Some(executable_hash(code)),
        _ => None,
    }
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
        BPF_LOADER_UPGRADEABLE_ID => match loader_state(owner, data) {
            Some(LoaderState::Program { program_data }) => Ok(ProgramAccount::Data(program_data)),
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
        BPF_LOADER_UPGRADEABLE_ID => match loader_state(owner, data) {
            Some(LoaderState::ProgramData {
                authority, code, ..
            }) => (authority, code),
            _ => return Err(malformed()),
        },
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

/// A bincode option tag at `offset`, then a key that takes its 32 bytes
/// whether it is there or not: the bytes after a none tag may be stale.
/// `None` for a tag other than 0 or 1. The caller checks the length.
fn optional_pubkey_at(data: &[u8], offset: usize) -> Option<Option<Pubkey>> {
    match data[offset] {
        0 => Some(None),
        1 => Some(Some(pubkey_at(data, offset + 1))),
        _ => None,
    }
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

    /// Code with zeros inside it, which the hash keeps, and its SHA-256 as
    /// Python's hashlib computes it.
    const CODE: &[u8] = b"\x7fELF\x00\x00code\x01";
    const CODE_SHA256: &str = "7bed2eb619d0304b5f5d25f175d9279bc1f04a1af39e59862716f91c101359b5";

    fn buffer(authority: Option<Pubkey>, padding: usize) -> Vec<u8> {
        let mut data = vec![1, 0, 0, 0];
        match authority {
            Some(key) => {
                data.push(1);
                data.extend(key.to_bytes());
            }
            None => data.extend([0; 33]),
        }
        assert_eq!(data.len(), BUFFER_HEADER);
        data.extend(CODE);
        data.extend(vec![0; padding]);
        data
    }

    fn program_data_holding(code: &[u8], padding: usize) -> Vec<u8> {
        let mut data = vec![3, 0, 0, 0];
        data.extend(521_170_062u64.to_le_bytes());
        data.push(1);
        data.extend([9; 32]);
        assert_eq!(data.len(), PROGRAM_DATA_HEADER);
        data.extend(code);
        data.extend(vec![0; padding]);
        data
    }

    #[test]
    fn each_loader_state_reads_from_its_header() {
        let key = Pubkey::new_from_array([9; 32]);
        assert_eq!(
            loader_account(&BPF_LOADER_UPGRADEABLE_ID, &[0, 0, 0, 0]),
            Some(LoaderAccount::Uninitialized)
        );

        let hash: [u8; 32] = hex::decode(CODE_SHA256).unwrap().try_into().unwrap();
        assert_eq!(
            loader_account(&BPF_LOADER_UPGRADEABLE_ID, &buffer(Some(key), 10)),
            Some(LoaderAccount::Buffer {
                authority: Some(key),
                hash
            })
        );

        let data_address = Pubkey::new_from_array([5; 32]);
        let mut program = vec![2, 0, 0, 0];
        program.extend(data_address.to_bytes());
        assert_eq!(
            loader_account(&BPF_LOADER_UPGRADEABLE_ID, &program),
            Some(LoaderAccount::Program {
                program_data: data_address
            })
        );

        assert_eq!(
            loader_account(&BPF_LOADER_UPGRADEABLE_ID, &program_data_holding(CODE, 64)),
            Some(LoaderAccount::ProgramData(DeployedProgram {
                slot: 521_170_062,
                authority: Some(key),
                hash,
            }))
        );
    }

    #[test]
    fn a_buffer_and_the_program_deployed_from_it_hash_the_same_however_padded() {
        let from_buffer = buffer_hash(&buffer(None, 3)).unwrap();
        assert_eq!(hex::encode(from_buffer), CODE_SHA256);
        assert_eq!(buffer_hash(&buffer(None, 4096)), Some(from_buffer));
        let Some(LoaderAccount::ProgramData(deployed)) =
            loader_account(&BPF_LOADER_UPGRADEABLE_ID, &program_data_holding(CODE, 777))
        else {
            panic!("not program data");
        };
        assert_eq!(deployed.hash, from_buffer);
        let code = program_code(
            &PROGRAM,
            &BPF_LOADER_UPGRADEABLE_ID,
            &program_data_holding(CODE, 0),
        )
        .unwrap();
        assert_eq!(code.hash, from_buffer);
    }

    #[test]
    fn a_none_authority_ignores_the_stale_key_after_it() {
        let mut data = buffer(None, 0);
        data[5..BUFFER_HEADER].copy_from_slice(&[8; 32]);
        assert_eq!(
            loader_account(&BPF_LOADER_UPGRADEABLE_ID, &data),
            Some(LoaderAccount::Buffer {
                authority: None,
                hash: buffer_hash(&buffer(None, 0)).unwrap(),
            })
        );
    }

    #[test]
    fn malformed_loader_accounts_are_not_loader_accounts() {
        let loader = &BPF_LOADER_UPGRADEABLE_ID;
        // Owned by another program.
        assert_eq!(loader_account(&BPF_LOADER_ID, &buffer(None, 0)), None);
        // Too short for a tag, or for the state's header.
        assert_eq!(loader_account(loader, &[]), None);
        assert_eq!(loader_account(loader, &[1, 0, 0]), None);
        assert_eq!(loader_account(loader, &buffer(None, 0)[..36]), None);
        assert_eq!(loader_account(loader, &[2, 0, 0, 0, 1, 2]), None);
        assert_eq!(
            loader_account(loader, &program_data_holding(b"", 0)[..44]),
            None
        );
        // An unknown state.
        assert_eq!(loader_account(loader, &[4, 0, 0, 0, 0, 0]), None);
        assert_eq!(loader_account(loader, &[0, 0, 0, 1]), None);
        // An option tag other than 0 or 1.
        let mut data = buffer(None, 0);
        data[4] = 2;
        assert_eq!(loader_account(loader, &data), None);
        assert_eq!(buffer_hash(&data), None);
        let mut data = program_data_holding(CODE, 0);
        data[12] = 7;
        assert_eq!(loader_account(loader, &data), None);
        // Not a buffer.
        assert_eq!(buffer_hash(&program_data_holding(CODE, 0)), None);
        // A buffer with nothing written yet hashes the empty string.
        assert_eq!(
            buffer_hash(&buffer(None, 0)[..BUFFER_HEADER]),
            Some(<[u8; 32]>::from(Sha256::digest(b"")))
        );
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
