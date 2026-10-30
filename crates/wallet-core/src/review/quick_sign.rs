//! Whether a transaction can be signed from a short prompt, without the full
//! Sign screen.
//!
//! Three things have to hold. Every instruction was put in words, so the
//! prompt shows all of what is being signed. Those words came from an IDL the
//! user can stand behind: one that ships with the app or one they imported.
//! And this device can add every signature that is missing, so signing
//! finishes the job. Anything else needs the Sign screen, where raw
//! instructions can be inspected and signatures collected from other devices.
//!
//! A prompt may still offer to sign what finishes the job without the first
//! two, as long as it says the summary cannot be trusted.

use crate::idl::catalog::IdlOrigin;
use crate::review::explain::{ExplainedIx, ExplainedTx};
use crate::review::signing::{signing_status_in, LocalKeys};
use crate::tx::decode::DecodedTx;
use crate::wallets::relay::AuthContext;
use solana_pubkey::Pubkey;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QuickSign {
    /// Instructions, at any depth, that no stored IDL put in words.
    pub unread: usize,
    /// Instructions, at any depth, put in words by an IDL read from the
    /// chain. Whoever controls the program wrote it.
    pub unvetted: usize,
    /// Signatures only another device can add.
    pub elsewhere: usize,
    /// Signatures this device can add now.
    pub signable: usize,
}

impl QuickSign {
    /// Every instruction can be shown as what it does.
    pub fn cleartext(&self) -> bool {
        self.unread == 0
    }

    /// Signing here leaves nothing missing.
    pub fn completes(&self) -> bool {
        self.elsewhere == 0 && self.signable > 0
    }

    /// No instruction was worded by an IDL the program's own authority wrote.
    pub fn vetted(&self) -> bool {
        self.unvetted == 0
    }

    pub fn allowed(&self) -> bool {
        self.cleartext() && self.vetted() && self.completes()
    }

    /// [`Self::allowed`], when `left` of the missing signatures are for
    /// whoever sent the transaction to add once it has it back.
    pub fn allowed_leaving(&self, left: usize) -> bool {
        self.cleartext() && self.vetted() && self.completes_leaving(left)
    }

    /// Signing here leaves nothing missing but the `left` signatures of
    /// whoever sent the transaction, whatever its summary reads.
    pub fn completes_leaving(&self, left: usize) -> bool {
        self.signable > 0 && self.elsewhere == left
    }
}

/// Judge `tx` for a device holding the keys `local`. `explained` is `tx` as
/// read with the IDLs on this device.
///
/// An IDL read from the chain still explains its program, on this prompt and
/// on the Sign screen. It does not count toward signing from the prompt: a
/// site with a program of its own could publish one that words a draining
/// instruction harmlessly.
pub fn quick_sign(tx: &DecodedTx, local: &[Pubkey], explained: &ExplainedTx) -> QuickSign {
    quick_sign_in(
        tx,
        &LocalKeys {
            keys: local,
            vector: &[],
        },
        &AuthContext::default(),
        explained,
    )
}

/// [`quick_sign`] for a device that may hold Vector accounts' keys, with what
/// `ctx` says of those accounts so their signatures' places are known.
pub fn quick_sign_in(
    tx: &DecodedTx,
    local: &LocalKeys,
    ctx: &AuthContext,
    explained: &ExplainedTx,
) -> QuickSign {
    let status = signing_status_in(tx, local, ctx);
    let vetted = |program_id: &Pubkey| {
        explained.programs.iter().any(|line| {
            line.program_id == *program_id
                && matches!(line.origin, Some(IdlOrigin::Builtin | IdlOrigin::File))
        })
    };
    let mut reading = Reading::default();
    for instruction in &explained.instructions {
        reading.count(instruction, &vetted);
    }
    QuickSign {
        unread: reading.unread,
        unvetted: reading.unvetted,
        elsewhere: status.missing_elsewhere(),
        signable: status.signable,
    }
}

#[derive(Default)]
struct Reading {
    unread: usize,
    unvetted: usize,
}

impl Reading {
    fn count(&mut self, instruction: &ExplainedIx, vetted: &impl Fn(&Pubkey) -> bool) {
        match (&instruction.inner, &instruction.idl) {
            // An instruction this app opened is read by what it carries.
            (Some(inner), _) => {
                for instruction in &inner.instructions {
                    self.count(instruction, vetted);
                }
            }
            // Extra accounts mean the program may read a layout the IDL does
            // not describe.
            (None, Some(idl)) if idl.unnamed_accounts == 0 => {
                if !vetted(&instruction.program_id) {
                    self.unvetted += 1;
                }
            }
            _ => self.unread += 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::{blockhash, compile, pk, transfer};
    use crate::idl::catalog::{IdlOrigin, IdlStore, MemoryBodies};
    use crate::keys::signer::{KeypairSigner, WalletSigner};
    use crate::review::explain::explain_transaction;
    use crate::review::signing::sign_locally;
    use crate::tx::decode::decode_v1_transaction;
    use crate::wallets::programmatic::{programmatic_signer_address, ExecutorNonce};
    use crate::wallets::relay::wrap_for_programmatic;
    use crate::wallets::squads::Squad;
    use solana_instruction::{AccountMeta, Instruction};
    use wallet_vault::store::MemoryStore;

    fn catalog() -> IdlStore<MemoryStore, MemoryBodies> {
        IdlStore::open(MemoryStore::new(), MemoryBodies::new()).with_builtins()
    }

    fn judge(wire: &[u8], local: &[Pubkey]) -> QuickSign {
        let tx = decode_v1_transaction(wire).unwrap();
        quick_sign(&tx, local, &explain_transaction(&catalog(), &tx))
    }

    #[test]
    fn a_transfer_this_device_can_finish_is_allowed() {
        let key = KeypairSigner::new_random();
        let wire = compile(key.pubkey(), vec![transfer(key.pubkey(), pk(2), 5)]).wire;
        let verdict = judge(&wire, &[key.pubkey()]);
        assert_eq!(
            verdict,
            QuickSign {
                unread: 0,
                unvetted: 0,
                elsewhere: 0,
                signable: 1
            }
        );
        assert!(verdict.allowed());

        // Once signed there is nothing left to add.
        let signed = sign_locally(&wire, &[&key as &dyn WalletSigner]).unwrap();
        let verdict = judge(&signed.wire, &[key.pubkey()]);
        assert!(verdict.cleartext() && !verdict.completes() && !verdict.allowed());
    }

    #[test]
    fn a_signature_from_elsewhere_sends_it_to_review() {
        let key = KeypairSigner::new_random();
        let wire = compile(key.pubkey(), vec![transfer(pk(7), pk(2), 5)]).wire;
        let verdict = judge(&wire, &[key.pubkey()]);
        assert_eq!((verdict.signable, verdict.elsewhere), (1, 1));
        assert!(verdict.cleartext() && !verdict.allowed());
        // A locked vault holds no key at all.
        let verdict = judge(&wire, &[]);
        assert_eq!((verdict.signable, verdict.elsewhere), (0, 2));
    }

    #[test]
    fn an_instruction_nothing_can_read_sends_it_to_review() {
        let key = KeypairSigner::new_random();
        let opaque = Instruction {
            program_id: pk(200),
            accounts: vec![AccountMeta::new(key.pubkey(), true)],
            data: vec![1, 2, 3],
        };
        let wire = compile(
            key.pubkey(),
            vec![transfer(key.pubkey(), pk(2), 5), opaque.clone(), opaque],
        )
        .wire;
        let verdict = judge(&wire, &[key.pubkey()]);
        assert_eq!(verdict.unread, 2);
        assert!(verdict.completes() && !verdict.cleartext() && !verdict.allowed());
        // Signed without a summary, it would still be complete.
        assert!(verdict.completes_leaving(0) && !verdict.allowed_leaving(0));

        // The System IDL does not know this tag either.
        let unknown = Instruction {
            program_id: crate::ids::SYSTEM_PROGRAM_ID,
            accounts: vec![AccountMeta::new(key.pubkey(), true)],
            data: vec![250, 0, 0, 0],
        };
        let wire = compile(key.pubkey(), vec![unknown]).wire;
        assert_eq!(judge(&wire, &[key.pubkey()]).unread, 1);
    }

    #[test]
    fn an_account_the_idl_does_not_describe_makes_the_instruction_unread() {
        let key = KeypairSigner::new_random();
        let mut padded = transfer(key.pubkey(), pk(2), 5);
        padded
            .accounts
            .push(AccountMeta::new_readonly(pk(9), false));
        let wire = compile(key.pubkey(), vec![padded]).wire;
        assert_eq!(judge(&wire, &[key.pubkey()]).unread, 1);
    }

    #[test]
    fn an_idl_read_from_the_chain_explains_but_does_not_allow_signing_from_the_prompt() {
        let key = KeypairSigner::new_random();
        let wire = compile(key.pubkey(), vec![transfer(key.pubkey(), pk(2), 5)]).wire;
        let tx = decode_v1_transaction(&wire).unwrap();
        let system = include_bytes!("../../idls/system.json");
        let judged = |origin: IdlOrigin| {
            let mut catalog = catalog();
            catalog.import_with_origin(system, None, origin).unwrap();
            let explained = explain_transaction(&catalog, &tx);
            // Whatever its origin, the IDL puts the transfer in words.
            assert!(explained.instructions[0].idl.is_some());
            quick_sign(&tx, &[key.pubkey()], &explained)
        };

        for origin in [IdlOrigin::ProgramMetadata, IdlOrigin::Anchor] {
            let verdict = judged(origin);
            assert_eq!((verdict.unread, verdict.unvetted), (0, 1));
            assert!(verdict.cleartext() && verdict.completes());
            assert!(!verdict.vetted() && !verdict.allowed());
        }
        // The same file, put there by the user, counts.
        assert!(judged(IdlOrigin::File).allowed());
    }

    #[test]
    fn an_idl_from_the_chain_for_a_program_inside_an_authorization_is_counted_too() {
        let hot = KeypairSigner::new_random();
        let authority = KeypairSigner::new_random();
        let address = programmatic_signer_address(&authority.pubkey());
        let built = compile(hot.pubkey(), vec![transfer(address, pk(2), 5)]).wire;
        let wire = wrap_for_programmatic(
            &decode_v1_transaction(&built).unwrap(),
            &[authority.pubkey()],
            ExecutorNonce {
                account: pk(9),
                authority: address,
                value: blockhash(3),
            },
            hot.pubkey(),
            blockhash(1),
        )
        .unwrap();
        let tx = decode_v1_transaction(&wire).unwrap();
        let mut catalog = catalog();
        catalog
            .import_with_origin(
                include_bytes!("../../idls/system.json"),
                None,
                IdlOrigin::ProgramMetadata,
            )
            .unwrap();
        let verdict = quick_sign(
            &tx,
            &[hot.pubkey(), authority.pubkey()],
            &explain_transaction(&catalog, &tx),
        );
        // The transfer sits two levels down, inside Submit and the executor call.
        assert_eq!((verdict.unread, verdict.unvetted), (0, 1));
        assert!(!verdict.allowed());
    }

    #[test]
    fn an_authorization_is_read_by_what_it_carries() {
        let hot = KeypairSigner::new_random();
        let authority = KeypairSigner::new_random();
        let address = programmatic_signer_address(&authority.pubkey());
        let wrap = |instructions: Vec<Instruction>| {
            let built = compile(hot.pubkey(), instructions).wire;
            wrap_for_programmatic(
                &decode_v1_transaction(&built).unwrap(),
                &[authority.pubkey()],
                ExecutorNonce {
                    account: pk(9),
                    authority: address,
                    value: blockhash(3),
                },
                hot.pubkey(),
                blockhash(1),
            )
            .unwrap()
        };
        let both = [hot.pubkey(), authority.pubkey()];

        // Submit and the executor call are opened. The transfer inside reads.
        let wire = wrap(vec![transfer(address, pk(2), 5)]);
        let verdict = judge(&wire, &both);
        assert_eq!(
            verdict,
            QuickSign {
                unread: 0,
                unvetted: 0,
                elsewhere: 0,
                signable: 2
            }
        );
        // With the authority on another device, the payer has to wait.
        let verdict = judge(&wire, &[hot.pubkey()]);
        assert_eq!((verdict.signable, verdict.elsewhere), (0, 1));
        assert!(!verdict.allowed());

        // A stored Squads transaction is opened too, and the proposal that
        // goes with it reads with the Squads IDL the app ships.
        let squad = Squad::v4(&pk(50));
        let vault = squad.vault(0);
        let wire = wrap(vec![
            squad
                .create_transaction(1, 0, &[transfer(vault, pk(2), 5)], &address, &hot.pubkey())
                .unwrap(),
            squad.create_proposal(1, &address, &hot.pubkey()),
        ]);
        let verdict = judge(&wire, &both);
        assert_eq!(verdict.unread, 0);
        assert!(verdict.allowed());
    }

    #[test]
    fn a_vector_wrap_is_allowed_when_scheme_and_keys_are_local() {
        use crate::review::explain::{explain_with_note, DisplayNote};
        use crate::review::signing::LocalKeys;
        use crate::wallets::relay::{wrap_for_vector, AuthContext, VectorContext, VectorWrap};
        use crate::wallets::vector::{address, SchemeId, PROGRAM_ID};
        use solana_instruction::{AccountMeta, Instruction};

        let payer = KeypairSigner::new_random();
        let authority = KeypairSigner::new_random();
        let account = address(&PROGRAM_ID, SchemeId::Ed25519, authority.pubkey().as_ref()).unwrap();
        let built = compile(payer.pubkey(), vec![transfer(account, pk(2), 5)]).wire;
        let wrap = VectorWrap {
            account,
            scheme: SchemeId::Ed25519,
            key: authority.pubkey().to_bytes().to_vec(),
            next_key: None,
        };
        let wire = wrap_for_vector(
            &decode_v1_transaction(&built).unwrap(),
            &[wrap],
            payer.pubkey(),
            blockhash(2),
        )
        .unwrap();
        let tx = decode_v1_transaction(&wire).unwrap();
        let known = [VectorContext {
            account,
            scheme: SchemeId::Ed25519,
            nonce: Some([3; 32]),
            key: Some(authority.pubkey().to_bytes().to_vec()),
            leaf_floor: 0,
        }];
        let note = DisplayNote::default().with_vector_schemes(&known);
        let explained = explain_with_note(&catalog(), &tx, &note);
        let local = LocalKeys {
            keys: &[payer.pubkey()],
            vector: &[account],
        };
        let ctx = AuthContext { vector: &known };
        let verdict = quick_sign_in(&tx, &local, &ctx, &explained);
        assert_eq!(
            verdict,
            QuickSign {
                unread: 0,
                unvetted: 0,
                elsewhere: 0,
                signable: 2
            }
        );
        assert!(verdict.allowed());

        // The Passthrough reads the same without the scheme.
        let blind = explain_transaction(&catalog(), &tx);
        assert_eq!(quick_sign_in(&tx, &local, &ctx, &blind), verdict);

        // An opaque instruction inside the Passthrough is unread too.
        let opaque = Instruction {
            program_id: pk(200),
            accounts: vec![AccountMeta::new(account, true)],
            data: vec![1, 2, 3],
        };
        let built = compile(payer.pubkey(), vec![opaque]).wire;
        let wire = wrap_for_vector(
            &decode_v1_transaction(&built).unwrap(),
            &[VectorWrap {
                account,
                scheme: SchemeId::Ed25519,
                key: authority.pubkey().to_bytes().to_vec(),
                next_key: None,
            }],
            payer.pubkey(),
            blockhash(2),
        )
        .unwrap();
        let tx = decode_v1_transaction(&wire).unwrap();
        let explained = explain_with_note(&catalog(), &tx, &note);
        let verdict = quick_sign_in(&tx, &local, &ctx, &explained);
        assert_eq!(verdict.unread, 1);
        assert!(!verdict.allowed());
    }
}
