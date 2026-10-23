//! Replacing a fee payer whose key is not on this device.

use super::*;
use crate::wallets::relay::{authority_slots, AuthorityId};

/// A fee payer the site named whose key is not on this device, and the hot
/// wallet that could pay in its place.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PayerSwap {
    /// The site's fee payer.
    pub from: Pubkey,
    /// The hot wallet on this device.
    pub to: Pubkey,
}

/// Whether the fee payer of `tx` could be replaced by a hot wallet of
/// `wallets`, so that its signature is no longer needed.
///
/// That holds when the payer only pays: no instruction names it, and it is
/// not a wallet of this device. The hot wallet is the fee payer set for the
/// programmatic signer the transaction asks to sign, when there is one, and
/// otherwise `selected`.
///
/// Once a Vector `Advance` signature is in place, a swap that would put a
/// new payer into an instruction the digest covers is refused: that changes
/// the message the account signed.
///
/// This is an offer, not a requirement. The site chose its payer, and may
/// refuse a transaction somebody else paid for. Simulate the result of
/// [`swap_fee_payer`] before offering it.
pub fn plan_payer_swap(
    tx: &DecodedTx,
    wallets: &[WalletRecord],
    selected: Option<WalletId>,
) -> Option<PayerSwap> {
    let from = tx.fee_payer()?;
    if wallets.iter().any(|wallet| wallet.address() == Some(from)) {
        return None;
    }
    // A message flags an account once, for every instruction that names it.
    // A payer an instruction names signs for that instruction too.
    if instruction_names(tx, &from) {
        return None;
    }
    let mut asked = wallets.iter().filter(|wallet| {
        wallet
            .address()
            .is_some_and(|address| tx.signers.iter().any(|slot| slot.pubkey == address))
    });
    // A vault's proposal is paid by this device's wallet already.
    if asked
        .clone()
        .any(|wallet| matches!(wallet.kind, WalletKind::Squads { .. }))
    {
        return None;
    }
    let hot = |id: WalletId| {
        wallets
            .iter()
            .find(|wallet| wallet.id == id && wallet.kind.can_sign())
            .and_then(|wallet| wallet.address())
    };
    let to = asked
        .find(|wallet| matches!(wallet.kind, WalletKind::Programmatic { .. }))
        .and_then(|wallet| signing_route(wallets, wallet.id).ok()?.fee_payer)
        .map(|(_, pubkey)| pubkey)
        .or_else(|| selected.and_then(hot))?;
    // A filled Advance covers instruction account flags. Making `to` the
    // fee payer when an instruction already names it would change those.
    let filled_advance = authority_slots(tx)
        .iter()
        .any(|slot| matches!(slot.authority, AuthorityId::Vector(_)) && slot.signed);
    if filled_advance && instruction_names(tx, &to) {
        return None;
    }
    Some(PayerSwap { from, to })
}

fn instruction_names(tx: &DecodedTx, key: &Pubkey) -> bool {
    tx.instructions
        .iter()
        .any(|instruction| instruction.program_id == *key || instruction.accounts.contains(key))
}

/// The same instructions, lifetime and budget, paid by `swap.to`. The old
/// payer is no longer in the transaction. Every signature it carried covered
/// the old message and is gone.
pub fn swap_fee_payer(tx: &DecodedTx, swap: &PayerSwap) -> Result<Vec<u8>, AdaptError> {
    let instructions = tx.native_instructions();
    Ok(compile_message(&swap.to, &instructions, tx.lifetime, tx.budget.config())?.wire)
}
