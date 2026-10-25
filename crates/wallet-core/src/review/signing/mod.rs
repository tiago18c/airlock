//! Where a transaction stands on its way to being signed, and the one order
//! in which a device signs it.
//!
//! Two kinds of signature can be asked for. A *transaction* signature covers
//! the whole message. An *authorization* signature covers the message inside a
//! programmatic signer's `Submit`, or for a Vector account the transaction
//! without its signatures, and is stored in the transaction's data. So
//! authorizations are signed first, and a transaction signature is only worth
//! making once every authorization is in place.

use crate::error::DecodeError;
use crate::error::SignError;
use crate::keys::signer::WalletSigner;
use crate::tx::decode::{decode_v1_transaction, parse_wire, DecodedTx, TxFormat};
use crate::tx::signatures::apply_local_signatures;
use crate::wallets::relay::{
    authority_slots_in, sign_authorizations_in, sign_vector_authorizations_in, AuthContext,
    AuthorityId,
};
use crate::wallets::vector::{VectorKey, XmssRetryCache};
use solana_hash::Hash;
use solana_pubkey::Pubkey;

mod steps;
pub use steps::{
    sign_steps, SignFinish, SignStep, SignStepInputs, SignStepKind, SignStepState, SimulationStage,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignerRole {
    /// Signs the transaction and pays its fee.
    FeePayer,
    /// Signs the transaction.
    Signer,
    /// Signs an authorization, as the programmatic signer at `address`.
    Authority { address: Pubkey },
}

/// What the next Sign press is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NextSignature {
    /// An authorization still has to be signed.
    Authorization,
    /// Every authorization is in, and an outer signature is still open.
    Transaction,
    /// Nothing left to sign; Submit is next.
    Nothing,
}

/// How a decoded message stays valid, for the Sign screen's copy.
///
/// Distinct from [`crate::Lifetime`], which is what a compile or refresh
/// fetches from the node.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TxLifetime {
    Blockhash(Hash),
    DurableNonce {
        account: Pubkey,
    },
    /// An authorization on its own has no blockhash or nonce.
    None,
}

/// What Sign is about to do, given who still has to sign.
pub fn next_step(status: &SigningStatus) -> NextSignature {
    let authority_open = status.lines.iter().any(|line| {
        matches!(line.role, SignerRole::Authority { .. }) && line.state != SignerState::Signed
    });
    if authority_open {
        return NextSignature::Authorization;
    }
    let outer_open = status.lines.iter().any(|line| {
        matches!(line.role, SignerRole::FeePayer | SignerRole::Signer)
            && line.state != SignerState::Signed
    });
    if outer_open {
        NextSignature::Transaction
    } else {
        NextSignature::Nothing
    }
}

/// How `tx` stays valid: its blockhash, its durable nonce, or nothing when
/// there is only an authorization to speak of.
pub fn tx_lifetime(tx: &DecodedTx) -> TxLifetime {
    if tx.uses_durable_nonce {
        let account = tx
            .instructions
            .first()
            .and_then(|ix| ix.accounts.first())
            .copied();
        if let Some(account) = account {
            return TxLifetime::DurableNonce { account };
        }
    }
    TxLifetime::Blockhash(tx.lifetime)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignerState {
    Signed,
    /// Something is in the signature's place and it does not verify.
    Invalid,
    /// Not signed, and this device holds the key.
    Local,
    /// Not signed, and the key is somewhere else.
    Missing,
}

/// Whose signature a line of the status is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignerKey {
    /// A key that signs the transaction.
    Key(Pubkey),
    /// Whoever signs an authorization. Not always a key the transaction names.
    Authority(AuthorityId),
}

impl SignerKey {
    /// The address that stands for this signer on screen when no controlling
    /// key is known separately.
    pub fn shown_as(&self) -> Pubkey {
        match self {
            Self::Key(pubkey) => *pubkey,
            Self::Authority(authority) => authority.shown_as(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SignerLine {
    pub key: SignerKey,
    /// Who signs this authorization when that is a key the transaction does
    /// not name: a Vector account's Ed25519 authority, from context. Shown
    /// before "for" on the Sign screen.
    pub signing_as: Option<Pubkey>,
    pub role: SignerRole,
    pub state: SignerState,
    /// This device holds the key, whether or not it has signed yet.
    pub held_here: bool,
}

impl SignerLine {
    /// The chip before "for": the controlling key when known, otherwise what
    /// the transaction names.
    pub fn authority_chip(&self) -> Pubkey {
        self.signing_as.unwrap_or_else(|| self.key.shown_as())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SigningStatus {
    /// Authorities first, then the transaction's signers in their order.
    pub lines: Vec<SignerLine>,
    /// Signatures this device can add right now.
    pub signable: usize,
    /// This device holds a key for the transaction, and has to wait for an
    /// authorization from elsewhere before using it.
    pub waiting_for_authority: bool,
    /// Vector accounts whose signature's place the context does not give:
    /// nothing can be signed or placed for them until it does.
    pub unplaced_authorizations: usize,
    /// Vector signatures that are in place but could not be checked, because
    /// the context lacks the account's nonce or key.
    pub unverified_authorizations: usize,
}

impl SigningStatus {
    pub fn signed(&self) -> usize {
        self.count(|state| state == SignerState::Signed)
    }

    pub fn total(&self) -> usize {
        self.lines.len()
    }

    /// Nothing is left to sign: the transaction can be submitted.
    pub fn complete(&self) -> bool {
        self.signed() == self.total()
    }

    /// Signatures only another device can add.
    pub fn missing_elsewhere(&self) -> usize {
        self.count(|state| matches!(state, SignerState::Missing | SignerState::Invalid))
    }

    /// Authorizations only another device can sign. An invalid one still has
    /// to come from elsewhere.
    pub fn authorizations_elsewhere(&self) -> usize {
        self.lines
            .iter()
            .filter(|line| {
                matches!(line.role, SignerRole::Authority { .. })
                    && matches!(line.state, SignerState::Missing | SignerState::Invalid)
            })
            .count()
    }

    /// Authorizations a key of this device signs. A press that signs any
    /// of them signs nothing else.
    pub fn authorizations_here(&self) -> usize {
        self.lines
            .iter()
            .filter(|line| {
                matches!(line.role, SignerRole::Authority { .. })
                    && line.state == SignerState::Local
            })
            .count()
    }

    fn count(&self, wanted: impl Fn(SignerState) -> bool) -> usize {
        self.lines.iter().filter(|line| wanted(line.state)).count()
    }
}

/// The keys this device holds, as far as signing a transaction goes.
#[derive(Clone, Copy, Debug, Default)]
pub struct LocalKeys<'a> {
    /// Keys that sign the transaction or a `Submit`.
    pub keys: &'a [Pubkey],
    /// Vector accounts whose key is on this device.
    pub vector: &'a [Pubkey],
}

/// Who has to sign `tx`, who has, and what this device, holding the keys
/// `local`, can do next.
pub fn signing_status(tx: &DecodedTx, local: &[Pubkey]) -> SigningStatus {
    signing_status_in(
        tx,
        &LocalKeys {
            keys: local,
            vector: &[],
        },
        &AuthContext::default(),
    )
}

/// [`signing_status`] for a device that may hold Vector accounts' keys,
/// with what `ctx` says of those accounts.
pub fn signing_status_in(tx: &DecodedTx, local: &LocalKeys, ctx: &AuthContext) -> SigningStatus {
    let keys = local.keys;
    let open = |signed: bool, invalid: bool, here: bool| {
        if signed {
            SignerState::Signed
        } else if here {
            SignerState::Local
        } else if invalid {
            SignerState::Invalid
        } else {
            SignerState::Missing
        }
    };
    let slots = authority_slots_in(tx, ctx);
    let vector = |authority: &AuthorityId| matches!(authority, AuthorityId::Vector(_));
    let unplaced_authorizations = slots
        .iter()
        .filter(|slot| vector(&slot.authority) && slot.place.is_none())
        .count();
    let unverified_authorizations = slots
        .iter()
        .filter(|slot| vector(&slot.authority) && slot.signed && slot.verified.is_none())
        .count();
    let mut lines: Vec<SignerLine> = slots
        .iter()
        .map(|slot| {
            let here = match slot.authority {
                AuthorityId::Ed25519(pubkey) => keys.contains(&pubkey),
                // A key signs only once where its signature goes is known.
                AuthorityId::Vector(account) => {
                    slot.place.is_some() && local.vector.contains(&account)
                }
            };
            let signing_as = match slot.authority {
                AuthorityId::Vector(account) => {
                    ctx.of(&account).and_then(|known| known.signing_pubkey())
                }
                AuthorityId::Ed25519(_) => None,
            };
            SignerLine {
                key: SignerKey::Authority(slot.authority),
                signing_as,
                role: SignerRole::Authority {
                    address: slot.address,
                },
                state: if slot.verified == Some(false) {
                    SignerState::Invalid
                } else {
                    open(slot.signed, false, here)
                },
                held_here: here,
            }
        })
        .collect();
    let authorities = lines.len();
    let authorized = lines.iter().all(|line| line.state == SignerState::Signed);
    let stateful = |line: &SignerLine| match line.key {
        SignerKey::Authority(AuthorityId::Vector(account)) => ctx
            .of(&account)
            .is_some_and(|known| known.scheme.is_stateful()),
        _ => false,
    };
    let is_vector =
        |line: &SignerLine| matches!(line.key, SignerKey::Authority(AuthorityId::Vector(_)));
    // A programmatic `Submit` has to be signed before any Vector key: the
    // Vector digest covers those signatures.
    let submit_open = lines.iter().any(|line| {
        matches!(line.key, SignerKey::Authority(AuthorityId::Ed25519(_)))
            && line.state != SignerState::Signed
    });
    // A stateful (XMSS) key signs last: hold it until every other authority
    // is already Signed, so its leaf is not spent on a declined Advance.
    let others_open = lines
        .iter()
        .any(|line| line.state != SignerState::Signed && !stateful(line));
    let authorizable = lines
        .iter()
        .filter(|line| {
            line.state == SignerState::Local
                && !(stateful(line) && others_open)
                && !(is_vector(line) && submit_open)
        })
        .count();
    // Every authorization this device cannot make leaves the message open.
    let stays_open = lines
        .iter()
        .any(|line| matches!(line.state, SignerState::Missing | SignerState::Invalid));

    lines.extend(
        tx.signers
            .iter()
            .enumerate()
            .map(|(index, slot)| SignerLine {
                key: SignerKey::Key(slot.pubkey),
                signing_as: None,
                role: if index == 0 {
                    SignerRole::FeePayer
                } else {
                    SignerRole::Signer
                },
                state: open(slot.valid, slot.filled, keys.contains(&slot.pubkey)),
                held_here: keys.contains(&slot.pubkey),
            }),
    );
    let outer_local = lines[authorities..]
        .iter()
        .filter(|line| line.state == SignerState::Local)
        .count();

    SigningStatus {
        signable: authorizable + if stays_open { 0 } else { outer_local },
        waiting_for_authority: !authorized && stays_open && outer_local > 0,
        unplaced_authorizations,
        unverified_authorizations,
        lines,
    }
}

/// What one press of Sign did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalSigning {
    pub wire: Vec<u8>,
    pub authorizations: usize,
    pub signatures: usize,
    /// Earlier transaction signatures that an authorization made void.
    pub dropped: usize,
    /// Every authorization is signed and a key here signs the transaction,
    /// on a press of its own.
    pub transaction_next: bool,
}

/// Sign everything `signers` can sign, in order: authorizations, then, on a
/// later press once none is outstanding, the transaction itself. A press
/// that signs an authorization stops there: the authorization changes what
/// the transaction runs, so it is simulated again and its limits fitted
/// before anything signs over them. Authorizations exist only in
/// v1 transactions this app built; legacy and v0 get transaction signatures only.
/// Signing reads only the message bytes, so a v0 wire needs no lookup tables.
pub fn sign_locally(wire: &[u8], signers: &[&dyn WalletSigner]) -> Result<LocalSigning, SignError> {
    sign_locally_in(wire, signers, &[], &AuthContext::default())
}

/// [`sign_locally`], also signing for the Vector accounts of `vector`, at the
/// nonces `ctx` holds for them.
pub fn sign_locally_in(
    wire: &[u8],
    signers: &[&dyn WalletSigner],
    vector: &[VectorKey],
    ctx: &AuthContext,
) -> Result<LocalSigning, SignError> {
    sign_locally_in_with(wire, signers, vector, ctx, None)
}

/// [`sign_locally_in`], remembering XMSS salts in `cache`.
pub fn sign_locally_in_with(
    wire: &[u8],
    signers: &[&dyn WalletSigner],
    vector: &[VectorKey],
    ctx: &AuthContext,
    cache: Option<&mut XmssRetryCache>,
) -> Result<LocalSigning, SignError> {
    let unreadable = |e: DecodeError| SignError::Message(e.to_string());
    let format = parse_wire(wire).map_err(unreadable)?.format;
    let (authorized_wire, authorizations, dropped, outstanding) = if format == TxFormat::V1 {
        let submitted = sign_authorizations_in(wire, signers, ctx)?;
        let advanced = sign_vector_authorizations_in(&submitted.wire, vector, ctx, cache)?;
        let tx = decode_v1_transaction(&advanced.wire).map_err(unreadable)?;
        let outstanding = authority_slots_in(&tx, ctx).iter().any(|slot| !slot.signed);
        (
            advanced.wire,
            submitted.signed + advanced.signed,
            submitted.dropped + advanced.dropped,
            outstanding,
        )
    } else {
        (wire.to_vec(), 0, 0, false)
    };
    let before = parse_wire(&authorized_wire)
        .map_err(unreadable)?
        .valid_signatures();

    // An authorization signed now holds the transaction for the next press.
    let transaction_next = authorizations > 0 && !outstanding && {
        let tx = decode_v1_transaction(&authorized_wire).map_err(unreadable)?;
        let keys: Vec<_> = signers.iter().map(|signer| signer.pubkey()).collect();
        tx.signers
            .iter()
            .any(|slot| !slot.filled && keys.contains(&slot.pubkey))
    };
    let (wire, signatures) = if outstanding || authorizations > 0 {
        (authorized_wire, 0)
    } else {
        let wire = apply_local_signatures(&authorized_wire, signers)?;
        let after = parse_wire(&wire)
            .map(|view| view.valid_signatures())
            .unwrap_or(before);
        (wire, after.saturating_sub(before))
    };
    if authorizations == 0 && signatures == 0 {
        return Err(SignError::NoLocalSigner);
    }
    Ok(LocalSigning {
        wire,
        authorizations,
        signatures,
        dropped,
        transaction_next,
    })
}

#[cfg(test)]
mod tests;
