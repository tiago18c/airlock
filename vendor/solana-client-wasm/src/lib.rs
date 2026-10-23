#![allow(dead_code, deprecated)]

#[macro_use]
extern crate serde_derive;

mod client;
mod constants;
mod error;
mod methods;
mod provider;
mod request;
mod response;
mod sleep;

#[cfg(feature = "pubsub")]
mod pubsub;

pub mod utils;

/// Compatibility shim so the vendored playground sources can keep writing
/// `crate::solana_sdk::…` against split Agave 4.2 crates (including transaction v1).
pub mod solana_sdk {
    pub use solana_account as account;
    pub use solana_clock as clock;
    pub use solana_commitment_config as commitment_config;
    pub use solana_epoch_info as epoch_info;
    pub use solana_epoch_schedule as epoch_schedule;
    pub use solana_hash as hash;
    pub use solana_inflation as inflation;
    pub use solana_message as message;
    pub use solana_nonce as nonce;
    pub use solana_pubkey as pubkey;

    pub mod native_token {
        pub const LAMPORTS_PER_SOL: u64 = 1_000_000_000;
    }

    pub mod signature {
        pub use solana_keypair::Keypair;
        pub use solana_signature::Signature;
        pub use solana_signer::Signer;
    }

    pub mod signer {
        pub use solana_signer::Signer;
    }

    pub mod transaction {
        pub use solana_transaction::versioned::VersionedTransaction;
        pub use solana_transaction::Transaction;
        pub use solana_transaction_error::TransactionError;
    }

    pub mod system_program {
        use solana_pubkey::Pubkey;
        pub fn id() -> Pubkey {
            Pubkey::default()
        }
    }
}

/// Compatibility shim replacing playground `solana-extra-wasm`.
pub mod solana_extra_wasm {
    pub mod account_decoder {
        pub use solana_account_decoder_client_types::{
            UiAccount, UiAccountData, UiAccountEncoding, UiDataSliceConfig,
        };

        pub trait UiAccountExt {
            fn decode(&self) -> Option<solana_account::Account>;
        }

        impl UiAccountExt for UiAccount {
            fn decode(&self) -> Option<solana_account::Account> {
                let data = self.data.decode()?;
                let owner = self.owner.parse().ok()?;
                Some(solana_account::Account {
                    lamports: self.lamports,
                    data,
                    owner,
                    executable: self.executable,
                    rent_epoch: self.rent_epoch,
                })
            }
        }

        pub mod parse_token {
            pub use solana_account_decoder_client_types::token::{
                TokenAccountType, UiTokenAccount, UiTokenAmount,
            };
        }
    }

    pub mod transaction_status {
        pub use solana_transaction_status_client_types::{
            EncodedConfirmedTransactionWithStatusMeta, TransactionConfirmationStatus,
            TransactionDetails, UiConfirmedBlock, UiTransactionEncoding,
        };
    }

    pub mod utils {
        pub use crate::sleep::sleep;
    }

    pub mod program {
        pub mod vote {
            pub mod vote_state {
                pub const MAX_LOCKOUT_HISTORY: usize = 31;
            }
        }
    }
}

pub use {
    client::WasmClient, error::ClientError, methods::GetAccountInfoResponse,
    request::ClientRequest, response::ClientResponse,
};

pub type ClientResult<T> = std::result::Result<T, error::ClientError>;
