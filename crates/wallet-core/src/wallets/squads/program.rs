//! The two Squads programs, and what differs between them.

use super::*;

/// May create transactions and proposals.
pub const PERMISSION_INITIATE: u8 = 1;

/// May approve, reject and cancel.
pub const PERMISSION_VOTE: u8 = 2;

/// May execute an approved proposal.
pub const PERMISSION_EXECUTE: u8 = 4;

pub const PERMISSION_ALL: u8 = PERMISSION_INITIATE | PERMISSION_VOTE | PERMISSION_EXECUTE;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SquadsProgram {
    #[default]
    V4,
    SmartAccount,
}

impl SquadsProgram {
    pub const fn program_id(self) -> Pubkey {
        match self {
            Self::V4 => SQUADS_V4_PROGRAM_ID,
            Self::SmartAccount => SQUADS_SMART_ACCOUNT_PROGRAM_ID,
        }
    }

    pub fn from_program_id(program_id: &Pubkey) -> Option<Self> {
        [Self::V4, Self::SmartAccount]
            .into_iter()
            .find(|program| program.program_id() == *program_id)
    }

    /// The program's IDL, as shipped with the app.
    pub(super) fn client(self) -> &'static ProgramClient {
        static V4: OnceLock<ProgramClient> = OnceLock::new();
        static SMART_ACCOUNT: OnceLock<ProgramClient> = OnceLock::new();
        let cell = match self {
            Self::V4 => &V4,
            Self::SmartAccount => &SMART_ACCOUNT,
        };
        cell.get_or_init(|| {
            let idl = builtin_idl(&self.program_id()).expect("the Squads IDLs ship with the app");
            ProgramClient::from_json(idl.json).expect("an embedded IDL is well-formed")
        })
    }

    /// Build instruction `name` of the IDL. A failure here is a mistake in
    /// this module, not in what the user asked for.
    pub(super) fn instruction(
        self,
        name: &str,
        accounts: &[(&str, Pubkey)],
        args: Option<Value>,
    ) -> Instruction {
        let mut method = self
            .client()
            .method(name)
            .unwrap_or_else(|err| panic!("the IDL has no {name}: {err}"));
        for (account, address) in accounts {
            method = method.account(*account, *address);
        }
        if let Some(args) = args {
            method = method.argument("args", args);
        }
        method
            .build()
            .unwrap_or_else(|err| panic!("{name} does not build from its IDL: {err}"))
    }

    /// Decode `data` as account `name` of the IDL. `None` when it is some
    /// other account, or not an account of this program.
    pub(super) fn account(self, name: &str, data: &[u8]) -> Option<Value> {
        let parsed = self.client().parse_account_data(data).ok()??;
        let found = match parsed.path.last()? {
            codama_nodes::Node::Account(account) => account.name.to_string(),
            _ => return None,
        };
        (found == name).then_some(parsed.data)
    }

    pub(super) const fn prefix(self) -> &'static [u8] {
        match self {
            Self::V4 => b"multisig",
            Self::SmartAccount => b"smart_account",
        }
    }

    pub(super) fn address(self, seeds: &[&[u8]]) -> Pubkey {
        Pubkey::find_program_address(seeds, &self.program_id()).0
    }

    /// The program-wide account that names the treasury and the creation fee.
    pub fn program_config(self) -> Pubkey {
        self.address(&[self.prefix(), b"program_config"])
    }

    /// The treasury a creation pays its fee to, read from the program config's data.
    pub fn treasury(self, program_config_data: &[u8]) -> Option<Pubkey> {
        key(field(
            &self.account("programConfig", program_config_data)?,
            "treasury",
        )?)
    }

    /// Smart account only: how many have been created so far. The next one is
    /// numbered one higher.
    pub fn smart_account_count(program_config_data: &[u8]) -> Option<u128> {
        number(field(
            &Self::SmartAccount.account("programConfig", program_config_data)?,
            "smartAccountIndex",
        )?)
    }
}
