use super::*;
use crate::fixtures::{blockhash, pk, transfer};
use crate::keys::signer::{KeypairSigner, WalletSigner};
use crate::review::signing::{sign_locally, signing_status};
use crate::tx::builder::{compile_v1, NonceSpec, DEFAULT_CU_PER_INSTRUCTION};
use crate::tx::signatures::apply_local_signatures;
use crate::wallets::programmatic::{
    programmatic_signer_address, Execution, Submission, ED25519_SIGNER_PROGRAM_ID,
    EXECUTOR_NONCE_LEN,
};
use crate::wallets::squads::{
    SquadsProgram, StoredTransaction, PERMISSION_ALL, PERMISSION_EXECUTE, PERMISSION_INITIATE,
    PERMISSION_VOTE,
};

/// A vault with a hot wallet that pays, a hot authority key, its
/// programmatic signer, and whatever a test adds.
struct Desk {
    payer: KeypairSigner,
    authority: KeypairSigner,
    wallets: Vec<WalletRecord>,
    payer_id: WalletId,
    signer_id: WalletId,
    /// The programmatic signer's address.
    address: Pubkey,
    nonce: Pubkey,
}

fn desk() -> Desk {
    let payer = KeypairSigner::new_random();
    let authority = KeypairSigner::new_random();
    let payer_wallet = WalletRecord::from_keypair_seed("Fees", payer.seed_bytes());
    let authority_wallet = WalletRecord::from_keypair_seed("Authority", authority.seed_bytes());
    let mut signer =
        WalletRecord::programmatic("Signer", authority.pubkey(), Some(authority_wallet.id));
    signer.kind.set_fee_payer(Some(payer_wallet.id));
    let nonce = pk(90);
    signer.kind.set_executor_nonce(Some(nonce));
    Desk {
        address: signer.address().unwrap(),
        payer_id: payer_wallet.id,
        signer_id: signer.id,
        wallets: vec![payer_wallet, authority_wallet, signer],
        payer,
        authority,
        nonce,
    }
}

impl Desk {
    fn keys(&self) -> [&dyn WalletSigner; 2] {
        [&self.payer, &self.authority]
    }

    /// Sign `wire` with both keys: the authorization on one press, the
    /// transaction on the next. Returns the second press.
    fn sign_both(&self, wire: &[u8]) -> crate::review::signing::LocalSigning {
        let first = sign_locally(wire, &self.keys()).unwrap();
        assert_eq!((first.authorizations, first.signatures), (1, 0));
        assert!(first.transaction_next);
        let second = sign_locally(&first.wire, &self.keys()).unwrap();
        assert_eq!((second.authorizations, second.signatures), (0, 1));
        second
    }

    fn nonce_account(&self) -> (Pubkey, RawAccount) {
        let mut data = vec![0u8; EXECUTOR_NONCE_LEN];
        data[..32].copy_from_slice(blockhash(7).as_ref());
        data[32..64].copy_from_slice(self.address.as_ref());
        (self.nonce, owned(EXECUTOR_NONCE_PROGRAM_ID, data))
    }

    /// Add a multisig acted on by wallet `member`, and return its squad.
    fn add_squad(&mut self, program: SquadsProgram, member: WalletId) -> Squad {
        let squad = match program {
            SquadsProgram::V4 => Squad::v4(&pk(50)),
            SquadsProgram::SmartAccount => Squad::smart_account(7),
        };
        let mut wallet = WalletRecord::squads("Treasury", squad, Vec::new(), 1);
        if let WalletKind::Squads { member_wallet, .. } = &mut wallet.kind {
            *member_wallet = Some(member);
        }
        wallet.kind.set_fee_payer(Some(self.payer_id));
        self.wallets.push(wallet);
        squad
    }
}

fn owned(owner: Pubkey, data: Vec<u8>) -> RawAccount {
    RawAccount {
        lamports: 1,
        data,
        owner,
        executable: false,
        rent_epoch: 0,
    }
}

/// A multisig account as the program lays it out.
fn squad_account(
    squad: &Squad,
    threshold: u16,
    time_lock: u32,
    transaction_index: u64,
    members: &[(Pubkey, u8)],
) -> (Pubkey, RawAccount) {
    // Anchor's account tag, then the fields before the threshold: create
    // key and config authority (v4), or number and settings authority.
    let mut data = match squad.program {
        SquadsProgram::V4 => {
            [hex::decode("e07479ba44a14fec").unwrap(), vec![0u8; 32 + 32]].concat()
        }
        SquadsProgram::SmartAccount => {
            [hex::decode("dfb3a3beb1e043ad").unwrap(), vec![0u8; 16 + 32]].concat()
        }
    };
    data.extend_from_slice(&threshold.to_le_bytes());
    data.extend_from_slice(&time_lock.to_le_bytes());
    data.extend_from_slice(&transaction_index.to_le_bytes());
    data.extend_from_slice(&0u64.to_le_bytes());
    data.push(0);
    if squad.program == SquadsProgram::SmartAccount {
        data.extend_from_slice(&0u64.to_le_bytes());
    }
    data.push(255);
    data.extend_from_slice(&(members.len() as u32).to_le_bytes());
    for (key, permissions) in members {
        data.extend_from_slice(key.as_ref());
        data.push(*permissions);
    }
    if squad.program == SquadsProgram::SmartAccount {
        // Account utilization, no policy seed, one reserved byte.
        data.extend_from_slice(&[0, 0, 0]);
    }
    (squad.address, owned(squad.program.program_id(), data))
}

/// What a site builds for an address it was given: that address pays and
/// sends five lamports.
fn site_tx(from: Pubkey) -> DecodedTx {
    let wire = compile_v1(
        from,
        vec![transfer(from, pk(2), 5)],
        None,
        Some(blockhash(1)),
    )
    .unwrap()
    .wire;
    decode_v1_transaction(&wire).unwrap()
}

fn signer_keys(tx: &DecodedTx) -> Vec<Pubkey> {
    tx.signers.iter().map(|slot| slot.pubkey).collect()
}

/// `address` is in the transaction and does not sign it.
fn is_plain_account(tx: &DecodedTx, address: &Pubkey) -> bool {
    tx.accounts
        .iter()
        .any(|account| account.pubkey == *address && !account.signer)
}

mod payer;
mod programmatic;
mod refusals;
mod squads;
mod vector;
