//! Deciding how a site's legacy or v0 transaction is signed on this device.

use crate::error::FlowError;
use crate::simulate::measure;
use solana_pubkey::Pubkey;
use wallet_core::simulate::{Simulation, SimulationError};
use wallet_core::{
    decode_v1_transaction, to_v1_swapping, Converted, DecodedTx, KeypairSigner, Limits, SolanaRpc,
    WalletSigner, SYSTEM_PROGRAM_ID,
};

/// What happens to a legacy or v0 request.
#[derive(Debug, PartialEq, Eq)]
pub enum Route {
    /// Signed as it is, in its own format.
    AsIs,
    /// Rewritten as v1, then routed as any v1 request. The keys made for it
    /// have to be kept until the transaction is signed.
    Converted(Converted, OneTimeKeys),
}

/// Keys made on this device for one request, each with the one-time key of
/// the site's it takes the place of. They sign the transaction last: an
/// authorization written into it voids every signature made before.
#[derive(Default)]
pub struct OneTimeKeys(Vec<(Pubkey, KeypairSigner)>);

impl OneTimeKeys {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn pubkeys(&self) -> Vec<Pubkey> {
        self.0.iter().map(|(_, key)| key.pubkey()).collect()
    }

    /// Each key with the site's key it replaces.
    pub fn pairs(&self) -> impl Iterator<Item = (Pubkey, &KeypairSigner)> {
        self.0.iter().map(|(old, key)| (*old, key))
    }

    pub fn signers(&self) -> Vec<&dyn WalletSigner> {
        self.0
            .iter()
            .map(|(_, key)| key as &dyn WalletSigner)
            .collect()
    }
}

impl std::fmt::Debug for OneTimeKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("OneTimeKeys").field(&self.pubkeys()).finish()
    }
}

impl PartialEq for OneTimeKeys {
    fn eq(&self, other: &Self) -> bool {
        self.pubkeys() == other.pubkeys()
    }
}

impl Eq for OneTimeKeys {}

/// Sign `tx` as it is when every unsigned slot is a key in `hot` and no
/// keyless wallet is asked (`keyless`). When `site_signs`, `tx` goes back to
/// a site that can still sign it, and an unsigned account it creates is left
/// for the site (see [`left_for_site`]) rather than counted against signing
/// it as it is. Otherwise rewrite it as v1, with
/// [`convert`]. `known` is every wallet of this device: a signature already
/// on `tx` from any other key stops the rewrite, unless the node has never
/// seen that key. Then it is a one-time key the site made, such as a new
/// mint: a key made here takes its place, and the rewrite has to simulate.
/// So does an unsigned one the transaction creates the account of, which
/// nobody could sign once the transaction is rewritten.
///
/// For a keyless wallet the rewrite is not what gets signed: it is rebuilt
/// first, and the rebuild only simulates once its authorization is signed.
/// Its simulation is left to then, and [`reshape_unsupported`] names a
/// failure.
pub async fn route(
    rpc: Option<&dyn SolanaRpc>,
    tx: &DecodedTx,
    hot: &[Pubkey],
    known: &[Pubkey],
    keyless: bool,
    site_signs: bool,
) -> Result<Route, FlowError> {
    let created = accounts_created(rpc, tx, known).await;
    route_created(rpc, tx, hot, known, keyless, site_signs, &created).await
}

/// [`route`], with the accounts `tx` creates already found by
/// [`accounts_created`].
pub async fn route_created(
    rpc: Option<&dyn SolanaRpc>,
    tx: &DecodedTx,
    hot: &[Pubkey],
    known: &[Pubkey],
    keyless: bool,
    site_signs: bool,
    created: &[Pubkey],
) -> Result<Route, FlowError> {
    let left = if site_signs {
        left_for_site(tx, known, created)
    } else {
        Vec::new()
    };
    let unsigned_here = tx
        .signers
        .iter()
        .filter(|slot| !slot.valid && !left.contains(&slot.pubkey))
        .all(|slot| hot.contains(&slot.pubkey));
    if !keyless && unsigned_here {
        return Ok(Route::AsIs);
    }
    // A node that cannot tell leaves the signature to stop the rewrite.
    let one_time = match rpc {
        Some(rpc) => one_time_signers(rpc, &replaceable_signers(tx, known, created))
            .await
            .unwrap_or_default(),
        None => Vec::new(),
    };
    let keys: Vec<(Pubkey, KeypairSigner)> = one_time
        .into_iter()
        .map(|old| (old, KeypairSigner::new_random()))
        .collect();
    let swaps: Vec<(Pubkey, Pubkey)> = keys.iter().map(|(old, key)| (*old, key.pubkey())).collect();
    let converted = if keyless {
        fit_limits(rpc, tx, known, &swaps).await?
    } else {
        convert_swapping(rpc, tx, known, &swaps).await?
    };
    Ok(Route::Converted(converted, OneTimeKeys(keys)))
}

/// Rewrite `tx` as v1. With a node, the compute and loaded-accounts limits are
/// what a simulation of the rewrite used, plus a margin, even when the site
/// filled them in. The probe asks for the highest of each so the simulation
/// is not cut short. Without a node, or when the simulation does not succeed,
/// the site's limits are kept when it set them, otherwise what the runtime
/// would have given the original. The site's price per unit is kept either way.
pub async fn convert(
    rpc: Option<&dyn SolanaRpc>,
    tx: &DecodedTx,
    known: &[Pubkey],
) -> Result<Converted, FlowError> {
    convert_swapping(rpc, tx, known, &[]).await
}

/// [`convert`], with each `(old, new)` of `swaps` put in place of a one-time
/// key the site signed with. Such a rewrite is refused unless a node says it
/// would succeed.
pub async fn convert_swapping(
    rpc: Option<&dyn SolanaRpc>,
    tx: &DecodedTx,
    known: &[Pubkey],
    swaps: &[(Pubkey, Pubkey)],
) -> Result<Converted, FlowError> {
    let converted = fit_limits(rpc, tx, known, swaps).await?;
    if swaps.is_empty() {
        return Ok(converted);
    }
    let Some(rpc) = rpc else {
        return Err(FlowError::Refused(
            "Replacing a one-time key needs a connection, to simulate the result.".into(),
        ));
    };
    let rewritten = decode_v1_transaction(&converted.wire).map_err(FlowError::refused)?;
    match measure(rpc, &rewritten).await?.error {
        None => Ok(converted),
        Some(failure) => Err(FlowError::Refused(reshape_unsupported(swaps, &failure))),
    }
}

async fn fit_limits(
    rpc: Option<&dyn SolanaRpc>,
    tx: &DecodedTx,
    known: &[Pubkey],
    swaps: &[(Pubkey, Pubkey)],
) -> Result<Converted, FlowError> {
    let rewrite = |limits| to_v1_swapping(tx, known, limits, swaps);
    let estimated = rewrite(Limits::Runtime).map_err(FlowError::refused)?;
    // Always probe when online: a site's compute and loaded-accounts limits
    // are discarded for the rewrite and replaced from the same run.
    let Some(rpc) = rpc else {
        return Ok(estimated);
    };
    let probe = rewrite(Limits::Probe)
        .ok()
        .and_then(|probe| decode_v1_transaction(&probe.wire).ok());
    let Some(probe) = probe else {
        return Ok(estimated);
    };
    match measure(rpc, &probe).await {
        Ok(simulation) if simulation.error.is_none() => {
            let measured = Limits::Measured {
                units: simulation.units_consumed,
                loaded_accounts: simulation.loaded_accounts_data_size,
            };
            rewrite(measured).map_err(FlowError::refused)
        }
        _ => Ok(estimated),
    }
}

/// Signers of `tx` that are not among `here`, the keys of this device, and
/// could be one-time keys: one with a valid signature already on it, or an
/// unsigned one among `created`, the accounts `tx` creates (see
/// [`accounts_created`]), as a site's new mint.
pub fn replaceable_signers(tx: &DecodedTx, here: &[Pubkey], created: &[Pubkey]) -> Vec<Pubkey> {
    tx.signers
        .iter()
        .filter(|slot| !here.contains(&slot.pubkey))
        .filter(|slot| slot.valid || (!slot.filled && created.contains(&slot.pubkey)))
        .map(|slot| slot.pubkey)
        .collect()
}

/// Unsigned signers of `tx` that are not among `known`, the wallets of this
/// device, and are among `created`, the accounts `tx` creates (see
/// [`accounts_created`]), as a site's new mint. A site that sent `tx` as it
/// is signs for them after this device does.
pub fn left_for_site(tx: &DecodedTx, known: &[Pubkey], created: &[Pubkey]) -> Vec<Pubkey> {
    tx.signers
        .iter()
        .filter(|slot| !slot.filled && !known.contains(&slot.pubkey))
        .filter(|slot| created.contains(&slot.pubkey))
        .map(|slot| slot.pubkey)
        .collect()
}

/// The accounts `tx` creates for a signer's key: each one a System
/// `CreateAccount` makes, or an `Allocate` gives its first data (Anchor's
/// `init` on an account that already holds lamports). Read from `tx`'s own
/// instructions and, given a `simulation` of `tx`, from the calls its
/// programs make: the Whirlpool program creates a new pool's vaults itself.
pub fn created_accounts(tx: &DecodedTx, simulation: Option<&Simulation>) -> Vec<Pubkey> {
    let own = tx
        .instructions
        .iter()
        .filter_map(|ix| created_by(&ix.program_id, &ix.data, &ix.accounts));
    let inner = simulation
        .into_iter()
        .flat_map(|simulation| &simulation.inner)
        .flat_map(|group| &group.calls)
        .filter_map(|call| {
            let ix = &call.instruction;
            let accounts: Vec<Pubkey> = ix.accounts.iter().map(|meta| meta.pubkey).collect();
            created_by(&ix.program_id, &ix.data, &accounts)
        });
    let mut created = Vec::new();
    for key in own.chain(inner) {
        if !created.contains(&key) {
            created.push(key);
        }
    }
    created
}

/// The account a System instruction creates: the new account of a
/// `CreateAccount`, the account of an `Allocate`.
fn created_by(program: &Pubkey, data: &[u8], accounts: &[Pubkey]) -> Option<Pubkey> {
    if *program != SYSTEM_PROGRAM_ID {
        return None;
    }
    match data.get(..4)? {
        tag if tag == 0u32.to_le_bytes() => accounts.get(1).copied(),
        tag if tag == 8u32.to_le_bytes() => accounts.first().copied(),
        _ => None,
    }
}

/// [`created_accounts`] of `tx`, simulating it when that can say more: when
/// an unsigned signer outside `known`, the wallets of this device, is not
/// created by `tx`'s own instructions. Without a node, or when the
/// simulation does not succeed, only those instructions are read.
pub async fn accounts_created(
    rpc: Option<&dyn SolanaRpc>,
    tx: &DecodedTx,
    known: &[Pubkey],
) -> Vec<Pubkey> {
    let own = created_accounts(tx, None);
    let unexplained = tx
        .signers
        .iter()
        .any(|slot| !slot.filled && !known.contains(&slot.pubkey) && !own.contains(&slot.pubkey));
    let Some(rpc) = rpc.filter(|_| unexplained) else {
        return own;
    };
    match measure(rpc, tx).await {
        Ok(simulation) if simulation.error.is_none() => created_accounts(tx, Some(&simulation)),
        _ => own,
    }
}

/// Which of `signers`, keys that signed on another device, the node has never
/// seen in a transaction. Such a key was made for this one transaction (a new
/// mint or account), so a key made here can take its place.
pub async fn one_time_signers(
    rpc: &dyn SolanaRpc,
    signers: &[Pubkey],
) -> Result<Vec<Pubkey>, FlowError> {
    let mut fresh = Vec::new();
    for signer in signers {
        if rpc.get_signatures_for_address(signer, 1).await?.is_empty() {
            fresh.push(*signer);
        }
    }
    Ok(fresh)
}

/// Why a transaction with its one-time keys replaced cannot be used.
pub fn reshape_unsupported(swaps: &[(Pubkey, Pubkey)], failure: &SimulationError) -> String {
    let keys = swaps
        .iter()
        .map(|(old, _)| old.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    let at = failure
        .instruction
        .map(|index| format!("instruction {}: ", index + 1))
        .unwrap_or_default();
    format!(
        "Reshaping this transaction is not supported for this website. It names {keys}, a key the site made for this transaction, and with a new key in its place the simulation fails ({at}{}). Ask the site for a new transaction, or sign the one it sent with a hot wallet.",
        failure.message
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{pk, FakeRpc};
    use wallet_core::simulate::{read_simulation, CompiledCall, InnerReply, SimulationReply};
    use wallet_core::{decode_transaction, LimitFrom, LookupTables, COMPUTE_BUDGET_PROGRAM_ID};

    /// A legacy transfer from `pk(1)` at 1,000 micro-lamports per unit and
    /// no limit, laid out by hand: signature count, an empty slot, then the
    /// message.
    fn priced_legacy() -> DecodedTx {
        let mut wire = vec![1u8];
        wire.extend_from_slice(&[0u8; 64]);
        wire.extend_from_slice(&[1, 0, 2, 4]);
        for key in [pk(1), pk(2), SYSTEM_PROGRAM_ID, COMPUTE_BUDGET_PROGRAM_ID] {
            wire.extend_from_slice(key.as_ref());
        }
        wire.extend_from_slice(&[1; 32]);
        wire.extend_from_slice(&[2, 3, 0, 9, 3]);
        wire.extend_from_slice(&1_000u64.to_le_bytes());
        wire.extend_from_slice(&[2, 2, 0, 1, 12]);
        wire.extend_from_slice(&2u32.to_le_bytes());
        wire.extend_from_slice(&5u64.to_le_bytes());
        decode_transaction(&wire, &LookupTables::new()).unwrap()
    }

    fn budget(converted: &Converted) -> wallet_core::TxBudget {
        decode_v1_transaction(&converted.wire).unwrap().budget
    }

    #[test]
    fn with_a_node_the_limits_are_what_the_rewrite_used_and_the_price_is_kept() {
        let rpc = FakeRpc {
            simulation: Some(SimulationReply {
                units_consumed: Some(150),
                loaded_accounts_data_size: Some(1_000),
                ..SimulationReply::default()
            }),
            ..FakeRpc::default()
        };
        let converted =
            pollster::block_on(convert(Some(&rpc), &priced_legacy(), &[pk(1)])).unwrap();
        assert_eq!(converted.limit_from, LimitFrom::Simulated);
        let budget = budget(&converted);
        assert_eq!(budget.compute_unit_limit, Some(165));
        assert_eq!(budget.loaded_accounts_data_size_limit, Some(1_100));
        assert_eq!(budget.priority_fee, Some(1));
        assert_eq!(rpc.asked(), 1);
    }

    /// A legacy transfer that already sets compute and loaded-accounts limits
    /// the site chose, plus a price per unit.
    fn limited_legacy() -> DecodedTx {
        let mut wire = vec![1u8];
        wire.extend_from_slice(&[0u8; 64]);
        wire.extend_from_slice(&[1, 0, 2, 4]);
        for key in [pk(1), pk(2), SYSTEM_PROGRAM_ID, COMPUTE_BUDGET_PROGRAM_ID] {
            wire.extend_from_slice(key.as_ref());
        }
        wire.extend_from_slice(&[1; 32]);
        // SetComputeUnitLimit 200_000, SetComputeUnitPrice 1_000,
        // SetLoadedAccountsDataSizeLimit 4_096, then a transfer.
        wire.extend_from_slice(&[4, 3, 0, 5, 2]);
        wire.extend_from_slice(&200_000u32.to_le_bytes());
        wire.extend_from_slice(&[3, 0, 9, 3]);
        wire.extend_from_slice(&1_000u64.to_le_bytes());
        wire.extend_from_slice(&[3, 0, 5, 4]);
        wire.extend_from_slice(&4_096u32.to_le_bytes());
        wire.extend_from_slice(&[2, 2, 0, 1, 12]);
        wire.extend_from_slice(&2u32.to_le_bytes());
        wire.extend_from_slice(&5u64.to_le_bytes());
        decode_transaction(&wire, &LookupTables::new()).unwrap()
    }

    #[test]
    fn a_sites_filled_limits_are_discarded_for_what_the_rewrite_used() {
        let tx = limited_legacy();
        assert_eq!(tx.budget.compute_unit_limit, Some(200_000));
        assert_eq!(tx.budget.loaded_accounts_data_size_limit, Some(4_096));
        let offline = pollster::block_on(convert(None, &tx, &[pk(1)])).unwrap();
        assert_eq!(offline.limit_from, LimitFrom::Site);
        assert_eq!(budget(&offline).compute_unit_limit, Some(200_000));
        assert_eq!(budget(&offline).loaded_accounts_data_size_limit, Some(4_096));

        let rpc = FakeRpc {
            simulation: Some(SimulationReply {
                units_consumed: Some(150),
                loaded_accounts_data_size: Some(1_000),
                ..SimulationReply::default()
            }),
            ..FakeRpc::default()
        };
        let converted = pollster::block_on(convert(Some(&rpc), &tx, &[pk(1)])).unwrap();
        assert_eq!(converted.limit_from, LimitFrom::Simulated);
        let budget = budget(&converted);
        assert_eq!(budget.compute_unit_limit, Some(165));
        assert_eq!(budget.loaded_accounts_data_size_limit, Some(1_100));
        assert_eq!(budget.priority_fee, Some(1));
        assert_eq!(rpc.asked(), 1);
    }

    #[test]
    fn without_a_node_or_a_working_simulation_the_runtime_limit_is_kept() {
        let offline = pollster::block_on(convert(None, &priced_legacy(), &[pk(1)])).unwrap();
        assert_eq!(offline.limit_from, LimitFrom::Runtime);
        assert_eq!(budget(&offline).compute_unit_limit, Some(3_000));
        assert_eq!(budget(&offline).priority_fee, Some(3));

        let failing = FakeRpc {
            simulation: Some(SimulationReply {
                units_consumed: Some(150),
                err: Some(serde_json::json!("AccountNotFound")),
                ..SimulationReply::default()
            }),
            ..FakeRpc::default()
        };
        let failed =
            pollster::block_on(convert(Some(&failing), &priced_legacy(), &[pk(1)])).unwrap();
        assert_eq!(failed, offline);

        // No simulation was set up, so the fake refuses that request.
        let refusing = FakeRpc::default();
        let refused =
            pollster::block_on(convert(Some(&refusing), &priced_legacy(), &[pk(1)])).unwrap();
        assert_eq!(refused, offline);
    }

    /// A legacy transaction paid by `pk(1)`, an offline key, that `pk(2)`
    /// also signs, as a site's new mint would. The signature of `pk(2)` is
    /// marked in place.
    fn site_signed_legacy() -> DecodedTx {
        let mut wire = vec![2u8];
        wire.extend_from_slice(&[0u8; 128]);
        wire.extend_from_slice(&[2, 0, 1, 3]);
        for key in [pk(1), pk(2), SYSTEM_PROGRAM_ID] {
            wire.extend_from_slice(key.as_ref());
        }
        wire.extend_from_slice(&[1; 32]);
        wire.extend_from_slice(&[1, 2, 2, 0, 1, 12]);
        wire.extend_from_slice(&2u32.to_le_bytes());
        wire.extend_from_slice(&5u64.to_le_bytes());
        let mut tx = decode_transaction(&wire, &LookupTables::new()).unwrap();
        tx.signers[1].filled = true;
        tx.signers[1].valid = true;
        tx
    }

    /// A legacy transaction paid by `pk(1)` that creates the account `pk(2)`,
    /// as a site's new mint, with both slots left empty.
    fn new_account_legacy() -> DecodedTx {
        let mut wire = vec![2u8];
        wire.extend_from_slice(&[0u8; 128]);
        wire.extend_from_slice(&[2, 0, 1, 3]);
        for key in [pk(1), pk(2), SYSTEM_PROGRAM_ID] {
            wire.extend_from_slice(key.as_ref());
        }
        wire.extend_from_slice(&[1; 32]);
        wire.extend_from_slice(&[1, 2, 2, 0, 1, 52]);
        wire.extend_from_slice(&0u32.to_le_bytes());
        wire.extend_from_slice(&1_000u64.to_le_bytes());
        wire.extend_from_slice(&82u64.to_le_bytes());
        wire.extend_from_slice(pk(7).as_ref());
        decode_transaction(&wire, &LookupTables::new()).unwrap()
    }

    /// A legacy transaction paid by `pk(1)` in which the program `pk(9)`
    /// creates the accounts `pk(2)` and `pk(3)` itself, as the Whirlpool
    /// program creates a new pool's vaults: both sign, and both slots are
    /// left empty. No instruction of the transaction's own creates them.
    fn created_inside_legacy() -> DecodedTx {
        let mut wire = vec![3u8];
        wire.extend_from_slice(&[0u8; 192]);
        wire.extend_from_slice(&[3, 0, 2, 5]);
        for key in [pk(1), pk(2), pk(3), SYSTEM_PROGRAM_ID, pk(9)] {
            wire.extend_from_slice(key.as_ref());
        }
        wire.extend_from_slice(&[1; 32]);
        wire.extend_from_slice(&[1, 4, 4, 0, 1, 2, 3, 1, 7]);
        decode_transaction(&wire, &LookupTables::new()).unwrap()
    }

    /// A node whose simulation of [`created_inside_legacy`] shows the
    /// program's `CreateAccount` of each vault, then an `Allocate` of the
    /// second (an account that already held lamports).
    fn creating_inside() -> FakeRpc {
        let create = |new: u8| CompiledCall {
            program_id_index: 3,
            accounts: vec![0, new],
            data: [0u32.to_le_bytes().as_slice(), &[0; 48]].concat(),
            stack_height: Some(2),
        };
        FakeRpc {
            simulation: Some(SimulationReply {
                units_consumed: Some(150),
                loaded_accounts_data_size: Some(1_000),
                inner: vec![InnerReply {
                    instruction: 0,
                    calls: vec![create(1), create(2)],
                }],
                ..SimulationReply::default()
            }),
            ..FakeRpc::default()
        }
    }

    #[test]
    fn accounts_a_program_creates_are_read_from_its_calls() {
        let tx = created_inside_legacy();
        assert!(created_accounts(&tx, None).is_empty());
        let rpc = creating_inside();
        let created = pollster::block_on(accounts_created(Some(&rpc), &tx, &[pk(1)]));
        assert_eq!(created, vec![pk(2), pk(3)]);
        assert_eq!(left_for_site(&tx, &[pk(1)], &created), vec![pk(2), pk(3)]);
        assert_eq!(
            replaceable_signers(&tx, &[pk(1)], &created),
            vec![pk(2), pk(3)]
        );

        // Allocate makes an account too.
        let allocate = SimulationReply {
            inner: vec![InnerReply {
                instruction: 0,
                calls: vec![CompiledCall {
                    program_id_index: 3,
                    accounts: vec![2],
                    data: [8u32.to_le_bytes().as_slice(), &[0; 8]].concat(),
                    stack_height: Some(2),
                }],
            }],
            ..SimulationReply::default()
        };
        let simulation = read_simulation(allocate, &tx, &[], &[]);
        assert_eq!(created_accounts(&tx, Some(&simulation)), vec![pk(3)]);
    }

    #[test]
    fn a_failed_simulation_or_no_node_reads_only_the_transactions_own_instructions() {
        let tx = created_inside_legacy();
        let mut failing = creating_inside();
        if let Some(reply) = failing.simulation.as_mut() {
            reply.err = Some(serde_json::json!("AccountNotFound"));
        }
        assert!(pollster::block_on(accounts_created(Some(&failing), &tx, &[pk(1)])).is_empty());
        assert!(pollster::block_on(accounts_created(None, &tx, &[pk(1)])).is_empty());
    }

    /// Nothing is simulated when the transaction's own instructions already
    /// account for every unsigned outside signer.
    #[test]
    fn a_transaction_that_creates_its_accounts_itself_is_not_simulated() {
        let tx = new_account_legacy();
        let rpc = succeeding();
        let created = pollster::block_on(accounts_created(Some(&rpc), &tx, &[pk(1)]));
        assert_eq!(created, vec![pk(2)]);
        assert_eq!(rpc.asked(), 0);
    }

    #[test]
    fn vaults_a_program_creates_are_left_to_the_site_or_replaced_by_keys_made_here() {
        let tx = created_inside_legacy();
        let rpc = creating_inside();
        let to_site = pollster::block_on(route(Some(&rpc), &tx, &[pk(1)], &[pk(1)], false, true));
        assert_eq!(to_site.unwrap(), Route::AsIs);

        let pasted = pollster::block_on(route(Some(&rpc), &tx, &[pk(1)], &[pk(1)], false, false));
        let Route::Converted(converted, keys) = pasted.unwrap() else {
            panic!("a pasted request is converted");
        };
        let made = keys.pubkeys();
        assert_eq!(made.len(), 2);
        assert_eq!(converted.replaced, vec![(pk(2), made[0]), (pk(3), made[1])]);
        let rewritten = decode_v1_transaction(&converted.wire).unwrap();
        // The new keys sign in place of the vaults, in whatever order the
        // rewrite puts them, after the payer.
        let mut signers: Vec<Pubkey> = rewritten.signers.iter().map(|slot| slot.pubkey).collect();
        assert_eq!(signers.remove(0), pk(1));
        signers.sort();
        let mut expected = made.clone();
        expected.sort();
        assert_eq!(signers, expected);
    }

    #[test]
    fn a_hot_payer_signs_a_sites_new_account_request_as_it_is_and_leaves_the_account_to_the_site() {
        let tx = new_account_legacy();
        assert_eq!(
            left_for_site(&tx, &[pk(1)], &created_accounts(&tx, None)),
            vec![pk(2)]
        );
        let to_site = pollster::block_on(route(None, &tx, &[pk(1)], &[pk(1)], false, true));
        assert_eq!(to_site.unwrap(), Route::AsIs);
        // Pasted, nobody signs after this device, so it is rewritten with a
        // key made here.
        let rpc = succeeding();
        let pasted = pollster::block_on(route(Some(&rpc), &tx, &[pk(1)], &[pk(1)], false, false));
        let Route::Converted(converted, keys) = pasted.unwrap() else {
            panic!("a pasted request is converted");
        };
        assert_eq!(converted.replaced, vec![(pk(2), keys.pubkeys()[0])]);
    }

    fn succeeding() -> FakeRpc {
        FakeRpc {
            simulation: Some(SimulationReply {
                units_consumed: Some(150),
                loaded_accounts_data_size: Some(1_000),
                ..SimulationReply::default()
            }),
            ..FakeRpc::default()
        }
    }

    #[test]
    fn a_key_the_node_never_saw_is_replaced_by_one_made_here() {
        let rpc = succeeding();
        let tx = site_signed_legacy();
        let Route::Converted(converted, keys) =
            pollster::block_on(route(Some(&rpc), &tx, &[], &[pk(1)], false, false)).unwrap()
        else {
            panic!("an offline payer is converted");
        };
        let fresh = keys.pubkeys();
        assert_eq!(fresh.len(), 1);
        assert_eq!(converted.replaced, vec![(pk(2), fresh[0])]);
        let v1 = decode_v1_transaction(&converted.wire).unwrap();
        let signers: Vec<Pubkey> = v1.signers.iter().map(|slot| slot.pubkey).collect();
        assert_eq!(signers, vec![pk(1), fresh[0]]);
    }

    #[test]
    fn a_replaced_key_that_fails_to_simulate_is_not_supported_for_the_site() {
        let rpc = FakeRpc {
            simulation: Some(SimulationReply {
                err: Some(serde_json::json!("InvalidSeeds")),
                ..SimulationReply::default()
            }),
            ..FakeRpc::default()
        };
        let err = pollster::block_on(route(
            Some(&rpc),
            &site_signed_legacy(),
            &[],
            &[pk(1)],
            false,
            false,
        ))
        .unwrap_err();
        let FlowError::Refused(text) = err else {
            panic!("refused, not {err:?}");
        };
        assert!(text.starts_with("Reshaping this transaction is not supported for this website."));
        assert!(text.contains(&pk(2).to_string()));
    }

    #[test]
    fn for_a_keyless_wallet_the_replaced_rewrite_is_simulated_after_its_rebuild() {
        // The rewrite alone fails, as it would with a Vector account paying.
        let rpc = FakeRpc {
            simulation: Some(SimulationReply {
                err: Some(serde_json::json!("InvalidAccountForFee")),
                ..SimulationReply::default()
            }),
            ..FakeRpc::default()
        };
        let routed = pollster::block_on(route(
            Some(&rpc),
            &site_signed_legacy(),
            &[],
            &[pk(1)],
            true,
            false,
        ))
        .unwrap();
        let Route::Converted(converted, keys) = routed else {
            panic!("a keyless wallet is converted");
        };
        assert_eq!(converted.replaced, vec![(pk(2), keys.pubkeys()[0])]);
    }

    #[test]
    fn a_key_used_on_chain_or_without_a_node_still_stops_the_rewrite() {
        let refused = FlowError::refused(wallet_core::ConvertError::OutsideSignature(pk(2)));
        let used = FakeRpc {
            used: vec![pk(2)],
            ..succeeding()
        };
        let tx = site_signed_legacy();
        assert_eq!(
            pollster::block_on(route(Some(&used), &tx, &[], &[pk(1)], false, false)).unwrap_err(),
            refused
        );
        assert_eq!(
            pollster::block_on(route(None, &tx, &[], &[pk(1)], false, false)).unwrap_err(),
            refused
        );
    }
}
