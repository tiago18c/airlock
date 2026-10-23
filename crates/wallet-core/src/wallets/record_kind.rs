//! What a wallet kind answers about itself, and the settings that change
//! after it is added.

use crate::ids::WalletId;
use crate::wallets::record::{
    KeyPhrase, OfflinePath, Protocol, VectorSettings, WalletClass, WalletKind,
};
use crate::wallets::squads::Squad;
use solana_pubkey::Pubkey;

impl WalletKind {
    /// The vault holds a key that signs for this wallet's own address.
    pub fn can_sign(&self) -> bool {
        matches!(self, Self::Keypair { .. } | Self::Seed { .. })
    }

    /// Whether this wallet can pay fees for another: it is a key, here or
    /// on another device. An offline payer's signature is brought in like
    /// any other signature made elsewhere.
    pub fn can_pay(&self) -> bool {
        self.can_sign() || matches!(self, Self::Offline { .. })
    }

    pub fn class(&self) -> WalletClass {
        match self {
            Self::Keypair { .. } | Self::Seed { .. } => WalletClass::Hot,
            Self::Hardware { .. } => WalletClass::Hardware,
            Self::Programmatic { .. } | Self::Squads { .. } => WalletClass::SmartContract,
            Self::Offline { .. } => WalletClass::Offline,
        }
    }

    /// The wallet chosen to pay fees for this wallet, when one is set: a hot
    /// wallet, or an offline one (see [`Self::can_pay`]).
    pub fn fee_payer(&self) -> Option<WalletId> {
        match self {
            Self::Offline { fee_payer, .. }
            | Self::Programmatic { fee_payer, .. }
            | Self::Squads { fee_payer, .. } => *fee_payer,
            _ => None,
        }
    }

    /// Set the fee payer. Does nothing for a wallet that pays its own fees.
    pub fn set_fee_payer(&mut self, wallet: Option<WalletId>) {
        if let Self::Offline { fee_payer, .. }
        | Self::Programmatic { fee_payer, .. }
        | Self::Squads { fee_payer, .. } = self
        {
            *fee_payer = wallet;
        }
    }

    /// Choose the wallet that acts as a member of this multisig. It no
    /// longer approves together with itself.
    pub fn set_member_wallet(&mut self, wallet: Option<WalletId>) {
        if let Self::Squads {
            member_wallet,
            approves_with,
            ..
        } = self
        {
            *member_wallet = wallet;
            approves_with.retain(|id| Some(*id) != wallet);
        }
    }

    /// Choose the wallets that approve together with the acting member, in
    /// the order they vote. One named twice counts once, and the acting
    /// member is left out.
    pub fn set_approves_with(&mut self, wallets: Vec<WalletId>) {
        if let Self::Squads {
            member_wallet,
            approves_with,
            ..
        } = self
        {
            approves_with.clear();
            for id in wallets {
                if Some(id) != *member_wallet && !approves_with.contains(&id) {
                    approves_with.push(id);
                }
            }
        }
    }

    /// Choose the durable nonce account a combined approval uses when an
    /// offline wallet votes in it. Nothing about it is checked here: the
    /// chain decides when it is used.
    pub fn set_approval_nonce(&mut self, nonce: Option<Pubkey>) {
        if let Self::Squads { approval_nonce, .. } = self {
            *approval_nonce = nonce;
        }
    }

    /// The executor nonce account of an Ed25519 programmatic signer.
    pub fn executor_nonce(&self) -> Option<Pubkey> {
        match self {
            Self::Programmatic {
                protocol: Protocol::Ed25519Signer { nonce, .. },
                ..
            } => *nonce,
            _ => None,
        }
    }

    /// Set the executor nonce account. Does nothing for any other wallet.
    pub fn set_executor_nonce(&mut self, account: Option<Pubkey>) {
        if let Self::Programmatic {
            protocol: Protocol::Ed25519Signer { nonce, .. },
            ..
        } = self
        {
            *nonce = account;
        }
    }

    /// The durable nonce account of an offline wallet.
    pub fn durable_nonce(&self) -> Option<Pubkey> {
        match self {
            Self::Offline { durable_nonce, .. } => *durable_nonce,
            _ => None,
        }
    }

    /// Set the durable nonce account. Does nothing for any other wallet.
    pub fn set_durable_nonce(&mut self, account: Option<Pubkey>) {
        if let Self::Offline { durable_nonce, .. } = self {
            *durable_nonce = account;
        }
    }

    /// Which path an offline wallet takes when it is chosen.
    pub fn default_path(&self) -> Option<OfflinePath> {
        match self {
            Self::Offline { default_path, .. } => *default_path,
            _ => None,
        }
    }

    /// Choose the default path. Does nothing for any other wallet.
    pub fn set_default_path(&mut self, path: Option<OfflinePath>) {
        if let Self::Offline { default_path, .. } = self {
            *default_path = path;
        }
    }

    /// Make `path` the default when none is chosen yet: the first path set
    /// up is the one taken.
    pub fn default_path_unless_chosen(&mut self, path: OfflinePath) {
        if self.default_path().is_none() {
            self.set_default_path(Some(path));
        }
    }

    /// The scheme, key and authority of a Vector wallet.
    pub fn vector_settings(&self) -> Option<VectorSettings<'_>> {
        match self {
            Self::Programmatic {
                protocol: Protocol::Vector { scheme, key, .. },
                authority,
                ..
            } => Some(VectorSettings {
                scheme: *scheme,
                key: &key.0,
                authority,
            }),
            _ => None,
        }
    }

    /// The recovery phrase a Vector wallet's keys are made from, when they
    /// are. In a summary its words are empty: only the number of the key
    /// and whether it was restored are shown.
    pub fn vector_phrase(&self) -> Option<&KeyPhrase> {
        match self {
            Self::Programmatic {
                protocol:
                    Protocol::Vector {
                        phrase: Some(phrase),
                        ..
                    },
                ..
            } => Some(phrase),
            _ => None,
        }
    }

    /// The authority key of an Ed25519 programmatic signer.
    pub fn signer_authority(&self) -> Option<Pubkey> {
        match self {
            Self::Programmatic {
                protocol: Protocol::Ed25519Signer { authority, .. },
                ..
            } => Some(*authority),
            _ => None,
        }
    }

    pub fn squad(&self) -> Option<Squad> {
        match self {
            Self::Squads {
                program, multisig, ..
            } => Some(Squad {
                program: *program,
                address: *multisig,
            }),
            _ => None,
        }
    }
}
