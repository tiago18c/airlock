//! Read a program's IDL from the chain and record the comparison. The stored
//! file is never replaced.
//!
//! An IDL is only ever read from its on-chain accounts: the program-metadata
//! account, or the legacy Anchor IDL account. Nothing here fetches a URL.

use crate::state::session::Session;
use dioxus::prelude::{ReadableExt, WritableExt};
use solana_pubkey::Pubkey;
use wallet_core::{IdlCheck, IdlError, IdlOrigin, IdlRecord};
use wallet_flows::idl::{counterpart, published_many, IdlReadError};

/// Why an on-chain IDL could not be read or compared.
#[derive(Debug, thiserror::Error)]
pub enum IdlCheckError {
    #[error("Offline. This check stays not checked.")]
    Offline,
    #[error(transparent)]
    Read(#[from] IdlReadError),
    #[error(transparent)]
    Idl(#[from] IdlError),
}

/// Read the on-chain bytes to compare with `record` and store the checksum
/// result. An IDL taken from program metadata is compared with that account,
/// any other with the Anchor IDL account. A program read from another
/// program's file is compared as that file, with that program's account.
/// A shell with no network returns before any socket is opened.
pub async fn check_idl(session: &Session, record: &IdlRecord) -> Result<IdlCheck, IdlCheckError> {
    let Ok(rpc) = session.rpc() else {
        return Err(IdlCheckError::Offline);
    };
    let file = record.host.unwrap_or(record.program_id);
    let remote = counterpart(rpc.as_ref(), &file, &record.origin).await?;
    session
        .record_idl_check(&file, &remote)
        .map_err(IdlCheckError::from)
}

#[derive(Default)]
pub struct MetadataFetch {
    pub imported: Vec<String>,
    pub missing: usize,
    pub errors: Vec<String>,
}

/// Store IDLs for programs this device does not have, reading every one in
/// two requests at most. Program-metadata is tried first. A missing or
/// unreadable account falls back to the Anchor IDL account. A program the
/// node said publishes neither is not asked about again.
/// A shell with no network returns an empty report and opens no socket.
pub async fn fetch_missing_idls(session: &Session, program_ids: &[Pubkey]) -> MetadataFetch {
    let Ok(rpc) = session.rpc() else {
        return MetadataFetch::default();
    };
    let node = session.rpc_url();
    let mut misses = session.idl_misses;
    let mut missing = 0usize;
    let asked: Vec<Pubkey> = program_ids
        .iter()
        .filter(|program_id| session.idl_body(program_id).is_none())
        .filter(|program_id| {
            let known = misses.peek().contains(&(node.clone(), **program_id));
            missing += usize::from(known);
            !known
        })
        .copied()
        .collect();
    let mut imported = Vec::new();
    let mut errors = Vec::new();
    if asked.is_empty() {
        return MetadataFetch {
            imported,
            missing,
            errors,
        };
    }
    let found = match published_many(rpc.as_ref(), &asked).await {
        Ok(found) => found,
        Err(err) => {
            errors.push(err.to_string());
            return MetadataFetch {
                imported,
                missing,
                errors,
            };
        }
    };
    for (program_id, published) in asked.iter().zip(found) {
        match published {
            Ok(Some((bytes, origin))) => {
                let source = if origin == IdlOrigin::Anchor {
                    "Anchor"
                } else {
                    "program metadata"
                };
                match session.import_idl_with_origin(&bytes, Some(*program_id), origin) {
                    Ok(record) => imported.push(format!("{} ({source})", record.name)),
                    Err(err) => errors.push(err.to_string()),
                }
            }
            Ok(None) => {
                misses.write().insert((node.clone(), *program_id));
                missing += 1;
            }
            Err(err) => errors.push(err.to_string()),
        }
    }
    MetadataFetch {
        imported,
        missing,
        errors,
    }
}
