//! Fetching address lookup tables a v0 transaction needs.

use crate::error::FlowError;
use wallet_core::{lookup_tables_needed, parse_lookup_table_addresses, LookupTables, SolanaRpc};

/// Read every lookup table named by `wire` from the node. A wire that names
/// none needs no node, so `rpc` may be absent.
pub async fn resolve_lookup_tables(
    rpc: Option<&dyn SolanaRpc>,
    wire: &[u8],
) -> Result<LookupTables, FlowError> {
    let needed = lookup_tables_needed(wire).map_err(FlowError::refused)?;
    if needed.is_empty() {
        return Ok(LookupTables::new());
    }
    let Some(rpc) = rpc else {
        return Err(FlowError::Refused(
            "This v0 transaction loads accounts from lookup tables, which can only be read online."
                .into(),
        ));
    };
    let accounts = rpc.get_accounts(&needed).await?;
    let mut tables = LookupTables::new();
    for (table, account) in needed.iter().zip(accounts) {
        let Some(account) = account else {
            return Err(FlowError::Refused(format!(
                "Address lookup table {table} does not exist."
            )));
        };
        let addresses = parse_lookup_table_addresses(&account.data).map_err(FlowError::refused)?;
        tables.insert(*table, addresses);
    }
    Ok(tables)
}
