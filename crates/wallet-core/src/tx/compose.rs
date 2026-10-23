//! Turn what a build form holds into a compiled transaction.
//!
//! The form keeps text exactly as it was typed. Parsing and the rules that tie
//! the fields together live here, so they can be tested without a window.

use crate::error::BuildError;
use crate::tx::builder::{
    compile_v1_budget, CompiledTx, ComputeBudgetSpec, NonceSpec, DEFAULT_LOADED_ACCOUNTS_DATA_SIZE,
    MAX_CU_LIMIT,
};
use solana_hash::Hash;
use solana_instruction::Instruction;
use solana_pubkey::Pubkey;
use std::str::FromStr;

/// Lifetime and compute-budget fields of a transaction, as typed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TxSettings {
    pub use_nonce: bool,
    pub nonce: Option<Pubkey>,
    pub nonce_authority: Option<Pubkey>,
    pub nonce_hash: String,
    /// Ignored when a durable nonce is used.
    pub blockhash: String,
    pub cu_limit: String,
    pub cu_price: String,
    pub loaded_accounts: String,
}

impl TxSettings {
    /// The durable nonce, when one was asked for. Account, authority and hash
    /// are required together.
    pub fn nonce_spec(&self) -> Result<Option<NonceSpec>, BuildError> {
        if !self.use_nonce {
            return Ok(None);
        }
        let (Some(account), Some(authority)) = (self.nonce, self.nonce_authority) else {
            return Err(message(
                "Nonce account, authority, and hash are required together.",
            ));
        };
        let hash = Hash::from_str(self.nonce_hash.trim())
            .map_err(|_| message("Nonce hash is not valid base58."))?;
        Ok(Some(NonceSpec {
            account,
            authority,
            hash,
        }))
    }

    /// The recent blockhash, unless a durable nonce replaces it. Blank is `None`.
    pub fn recent_blockhash(&self, has_nonce: bool) -> Result<Option<Hash>, BuildError> {
        let raw = self.blockhash.trim();
        if has_nonce || raw.is_empty() {
            return Ok(None);
        }
        Hash::from_str(raw)
            .map(Some)
            .map_err(|_| message("Recent blockhash is not valid base58."))
    }

    /// No durable nonce and no blockhash: the latest one has to be fetched.
    pub fn wants_blockhash(&self) -> bool {
        !self.use_nonce && self.blockhash.trim().is_empty()
    }

    /// A limit was left blank, so a simulation should size it.
    pub fn wants_limits(&self) -> bool {
        self.cu_limit.trim().is_empty() || self.loaded_accounts.trim().is_empty()
    }

    /// Fill each blank limit with what a simulation measured, plus a margin.
    /// A limit that was typed, or that the node did not measure, is left
    /// alone. Returns whether anything was filled.
    pub fn fill_limits(
        &mut self,
        units_consumed: Option<u64>,
        loaded_accounts_data_size: Option<u32>,
    ) -> bool {
        let mut filled = false;
        if let Some(units) = units_consumed {
            if self.cu_limit.trim().is_empty() {
                self.cu_limit = with_margin(units, MAX_CU_LIMIT).to_string();
                filled = true;
            }
        }
        if let Some(size) = loaded_accounts_data_size {
            if self.loaded_accounts.trim().is_empty() {
                self.loaded_accounts =
                    with_margin(u64::from(size), DEFAULT_LOADED_ACCOUNTS_DATA_SIZE).to_string();
                filled = true;
            }
        }
        filled
    }

    /// Compute-budget values. A blank field leaves that instruction out.
    pub fn budget(&self) -> Result<ComputeBudgetSpec, BuildError> {
        Ok(ComputeBudgetSpec {
            cu_limit: whole_number(&self.cu_limit, "CU limit")?,
            cu_price_micro_lamports: whole_number(&self.cu_price, "Priority fee")?,
            loaded_accounts_data_size: whole_number(&self.loaded_accounts, "Loaded account size")?,
        })
    }
}

/// Compile `instructions`, in order, behind the nonce advance and the
/// compute-budget instructions that `settings` asks for.
pub fn compile_draft(
    fee_payer: Option<Pubkey>,
    instructions: Vec<Instruction>,
    settings: &TxSettings,
) -> Result<CompiledTx, BuildError> {
    let fee_payer = fee_payer.ok_or_else(|| message("Pick a fee payer."))?;
    if instructions.is_empty() {
        return Err(message("Add at least one instruction."));
    }
    let nonce = settings.nonce_spec()?;
    let recent = settings.recent_blockhash(nonce.is_some())?;
    compile_v1_budget(fee_payer, instructions, nonce, recent, settings.budget()?)
}

/// Compile `instructions` only to be simulated: a blank compute limit becomes
/// the most a transaction may ask for, so the simulation measures what the
/// transaction uses and is not cut short. A missing blockhash becomes a
/// placeholder, which the node replaces when it simulates.
pub fn compile_probe(
    fee_payer: Option<Pubkey>,
    instructions: Vec<Instruction>,
    settings: &TxSettings,
) -> Result<CompiledTx, BuildError> {
    let mut settings = settings.clone();
    if settings.cu_limit.trim().is_empty() {
        settings.cu_limit = MAX_CU_LIMIT.to_string();
    }
    if settings.wants_blockhash() {
        settings.blockhash = Hash::default().to_string();
    }
    compile_draft(fee_payer, instructions, &settings)
}

/// How much is added to a measured limit, in percent.
const LIMIT_MARGIN_PERCENT: u64 = 10;

/// `measured` plus the margin, rounded up, and no more than `max`.
pub(crate) fn with_margin(measured: u64, max: u32) -> u32 {
    let margin = measured.saturating_mul(LIMIT_MARGIN_PERCENT).div_ceil(100);
    u32::try_from(measured.saturating_add(margin))
        .unwrap_or(u32::MAX)
        .min(max)
}

fn whole_number<T: FromStr>(raw: &str, label: &str) -> Result<Option<T>, BuildError> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(None);
    }
    raw.parse()
        .map(Some)
        .map_err(|_| BuildError::Message(format!("{label} must be a whole number.")))
}

fn message(text: &str) -> BuildError {
    BuildError::Message(text.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::pk;
    use crate::tx::decode::decode_v1_transaction;

    fn transfer() -> Vec<Instruction> {
        vec![crate::fixtures::transfer(pk(1), pk(2), 5)]
    }

    fn text(result: Result<CompiledTx, BuildError>) -> String {
        result.unwrap_err().to_string()
    }

    #[test]
    fn a_draft_needs_a_fee_payer_an_instruction_and_a_lifetime() {
        let settings = TxSettings::default();
        assert_eq!(
            text(compile_draft(None, transfer(), &settings)),
            "Pick a fee payer."
        );
        assert_eq!(
            text(compile_draft(Some(pk(1)), Vec::new(), &settings)),
            "Add at least one instruction."
        );
        assert_eq!(
            compile_draft(Some(pk(1)), transfer(), &settings).unwrap_err(),
            BuildError::BlockhashOffline
        );
    }

    #[test]
    fn nonce_fields_are_required_together_and_replace_the_blockhash() {
        let mut settings = TxSettings {
            use_nonce: true,
            nonce: Some(pk(7)),
            blockhash: "not base58 !".into(),
            ..TxSettings::default()
        };
        assert_eq!(
            text(compile_draft(Some(pk(1)), transfer(), &settings)),
            "Nonce account, authority, and hash are required together."
        );
        settings.nonce_authority = Some(pk(1));
        settings.nonce_hash = "0OIl".into();
        assert_eq!(
            text(compile_draft(Some(pk(1)), transfer(), &settings)),
            "Nonce hash is not valid base58."
        );
        settings.nonce_hash = Hash::new_from_array([4; 32]).to_string();
        // The blockhash text is not even parsed once a nonce is set.
        let compiled = compile_draft(Some(pk(1)), transfer(), &settings).unwrap();
        assert!(compiled.uses_durable_nonce);
        let decoded = decode_v1_transaction(&compiled.wire).unwrap();
        assert!(decoded.uses_durable_nonce);
        assert_eq!(decoded.instructions.len(), 2);
    }

    #[test]
    fn blank_settings_ask_for_a_blockhash_and_for_limits() {
        let mut settings = TxSettings::default();
        assert!(settings.wants_blockhash());
        assert!(settings.wants_limits());

        settings.blockhash = Hash::new_from_array([9; 32]).to_string();
        settings.cu_limit = "1000".into();
        assert!(!settings.wants_blockhash());
        // The loaded-accounts size is still blank.
        assert!(settings.wants_limits());
        settings.loaded_accounts = "2000".into();
        assert!(!settings.wants_limits());

        let on_nonce = TxSettings {
            use_nonce: true,
            ..TxSettings::default()
        };
        assert!(!on_nonce.wants_blockhash());
    }

    #[test]
    fn measured_limits_fill_blank_fields_with_a_margin() {
        let mut settings = TxSettings::default();
        assert!(settings.fill_limits(Some(150), Some(1_000)));
        // 10% of 150 is 15.
        assert_eq!(settings.cu_limit, "165");
        assert_eq!(settings.loaded_accounts, "1100");

        // A typed limit is kept, and a node that measured nothing fills nothing.
        let mut typed = TxSettings {
            cu_limit: "999".into(),
            ..TxSettings::default()
        };
        assert!(typed.fill_limits(Some(150), Some(1_000)));
        assert_eq!(typed.cu_limit, "999");
        assert_eq!(typed.loaded_accounts, "1100");
        assert!(!TxSettings::default().fill_limits(None, None));

        let mut capped = TxSettings::default();
        capped.fill_limits(Some(u64::from(MAX_CU_LIMIT)), None);
        assert_eq!(capped.cu_limit, MAX_CU_LIMIT.to_string());
    }

    #[test]
    fn a_probe_compiles_without_a_blockhash_at_the_highest_compute_limit() {
        let settings = TxSettings::default();
        let probe = compile_probe(Some(pk(1)), transfer(), &settings).unwrap();
        let config = solana_message::v1::deserialize(&probe.message[1..])
            .unwrap()
            .config;
        assert_eq!(config.compute_unit_limit, Some(MAX_CU_LIMIT));
        // What was typed is still checked before anything is sent to a node.
        assert_eq!(
            text(compile_probe(None, transfer(), &settings)),
            "Pick a fee payer."
        );
    }

    #[test]
    fn budget_fields_are_optional_whole_numbers_stated_in_the_header() {
        let mut settings = TxSettings {
            blockhash: Hash::new_from_array([9; 32]).to_string(),
            cu_limit: " 200000 ".into(),
            cu_price: "1.5".into(),
            ..TxSettings::default()
        };
        assert_eq!(
            text(compile_draft(Some(pk(1)), transfer(), &settings)),
            "Priority fee must be a whole number."
        );
        settings.cu_price = "25".into();
        let compiled = compile_draft(Some(pk(1)), transfer(), &settings).unwrap();
        let decoded = decode_v1_transaction(&compiled.wire).unwrap();
        assert_eq!(decoded.instructions.len(), 1);
        assert_eq!(decoded.instructions[0].accounts, vec![pk(1), pk(2)]);
        let config = solana_message::v1::deserialize(&compiled.message[1..])
            .unwrap()
            .config;
        assert_eq!(config.compute_unit_limit, Some(200_000));
        // 200,000 units at 25 micro-lamports each.
        assert_eq!(config.priority_fee, Some(5));
        assert_eq!(settings.budget().unwrap().loaded_accounts_data_size, None);
    }
}
