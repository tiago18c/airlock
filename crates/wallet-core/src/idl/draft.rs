//! One instruction being filled in from a stored IDL.

use crate::idl::build::{
    build_idl_instruction, idl_instruction_form, IdlArgKind, IdlBuildFailure, IdlBuildInput,
};
use codama_dynamic_client::ProgramClient;
use solana_instruction::Instruction;
use solana_pubkey::Pubkey;

/// The values a build form has collected for one IDL instruction.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IdlDraft {
    pub program_id: Option<Pubkey>,
    pub program_name: String,
    pub instruction: String,
    pub accounts: Vec<(String, Option<Pubkey>)>,
    pub arguments: Vec<(String, String)>,
    /// Derived accounts the encoder could not fill, or that the user chose to
    /// fill instead of taking the IDL default. The form asks for these.
    pub manual: Vec<String>,
}

impl IdlDraft {
    /// No program chosen yet.
    pub fn empty() -> Self {
        Self::default()
    }

    /// A blank draft for `instruction`, or for the program's first instruction.
    /// Every account and argument the form will ask for gets its slot, with
    /// booleans off and enums on their first variant.
    pub fn for_instruction(
        program_id: Pubkey,
        program_name: &str,
        client: &ProgramClient,
        instruction: Option<&str>,
    ) -> Self {
        let name = instruction
            .map(str::to_string)
            .or_else(|| {
                client
                    .instruction_names()
                    .first()
                    .map(|name| (*name).to_string())
            })
            .unwrap_or_default();
        let mut draft = Self {
            program_id: Some(program_id),
            program_name: program_name.to_string(),
            instruction: name.clone(),
            ..Self::default()
        };
        if let Ok(form) = idl_instruction_form(client, &name, &IdlBuildInput::default()) {
            draft.accounts = form
                .accounts
                .into_iter()
                .map(|account| (account.name, None))
                .collect();
            draft.arguments = form
                .arguments
                .into_iter()
                .map(|argument| (argument.name, default_argument_text(&argument.kind)))
                .collect();
        }
        draft
    }

    pub fn input(&self) -> IdlBuildInput {
        IdlBuildInput {
            accounts: self.accounts.clone(),
            arguments: self.arguments.clone(),
        }
    }

    pub fn account(&self, name: &str) -> Option<Pubkey> {
        self.accounts
            .iter()
            .find(|(entry, _)| entry == name)
            .and_then(|(_, address)| *address)
    }

    pub fn argument(&self, name: &str) -> &str {
        self.arguments
            .iter()
            .find(|(entry, _)| entry == name)
            .map_or("", |(_, value)| value.as_str())
    }

    pub fn is_manual(&self, name: &str) -> bool {
        self.manual.iter().any(|entry| entry == name)
    }

    /// Ask for a derived account instead of taking its default, or go back to
    /// the default. Going back forgets the typed address.
    pub fn set_manual(&mut self, name: &str, manual: bool) {
        self.manual.retain(|entry| entry != name);
        if manual {
            self.manual.push(name.to_string());
        } else {
            self.set_account(name, None);
        }
    }

    pub fn set_account(&mut self, name: &str, address: Option<Pubkey>) {
        match self.accounts.iter_mut().find(|(entry, _)| entry == name) {
            Some(slot) => slot.1 = address,
            None => self.accounts.push((name.to_string(), address)),
        }
    }

    pub fn set_argument(&mut self, name: &str, text: String) {
        match self.arguments.iter_mut().find(|(entry, _)| entry == name) {
            Some(slot) => slot.1 = text,
            None => self.arguments.push((name.to_string(), text)),
        }
    }

    /// Encode the instruction. When an account cannot be derived it becomes a
    /// manual field on this draft, and the error names it.
    pub fn build(&mut self, client: &ProgramClient) -> Result<Instruction, IdlBuildFailure> {
        let built = build_idl_instruction(client, &self.instruction, &self.input());
        if let Err(IdlBuildFailure::FillAddress { account, .. }) = &built {
            if !self.is_manual(account) {
                self.manual.push(account.clone());
            }
        }
        built
    }
}

fn default_argument_text(kind: &IdlArgKind) -> String {
    match kind {
        IdlArgKind::Bool => "false".to_string(),
        IdlArgKind::Enum { variants } => variants.first().cloned().unwrap_or_default(),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pk(n: u8) -> Pubkey {
        Pubkey::new_from_array([n; 32])
    }

    #[test]
    fn slots_are_updated_in_place_or_added() {
        let mut draft = IdlDraft::empty();
        assert_eq!(draft.account("payer"), None);
        assert_eq!(draft.argument("amount"), "");

        draft.set_account("payer", Some(pk(1)));
        draft.set_account("payer", Some(pk(2)));
        draft.set_argument("amount", "5".into());
        draft.set_argument("amount", "7".into());

        assert_eq!(draft.accounts, vec![("payer".to_string(), Some(pk(2)))]);
        assert_eq!(
            draft.arguments,
            vec![("amount".to_string(), "7".to_string())]
        );
        assert_eq!(draft.account("payer"), Some(pk(2)));
        assert_eq!(draft.argument("amount"), "7");
        assert_eq!(draft.input().accounts, draft.accounts);
    }

    #[test]
    fn a_system_transfer_draft_gets_its_slots_and_builds() {
        let client = ProgramClient::from_json(include_str!("../../tests/fixtures/system-idl.json"))
            .expect("system idl");
        let program_id = client.program_address();
        let mut draft =
            IdlDraft::for_instruction(program_id, "system", &client, Some("transferSol"));
        assert_eq!(draft.instruction, "transferSol");
        assert!(draft.accounts.iter().any(|(name, _)| name == "source"));
        assert!(draft.arguments.iter().any(|(name, _)| name == "amount"));

        draft.set_account("source", Some(pk(1)));
        draft.set_account("destination", Some(pk(2)));
        draft.set_argument("amount", "5".into());
        let instruction = draft.build(&client).expect("builds");
        assert_eq!(instruction.program_id, program_id);
        assert!(draft.manual.is_empty());
    }
}
