//! What the Sign screen and the request modal offer for a transaction, and what
//! a press will do. Decided from plain facts, so each rule can be read and tested
//! apart from the screens. The screens only gather the facts and draw the answers.

use wallet_core::squads::{ClearSigningAccepts, ShapeRefused};
use wallet_core::{ApprovalShape, NextSignature, QuickSign};

/// Whether a Sign press first moves the transaction to a fresh blockhash. A
/// relayed transaction is moved inside [`crate::sign`] and is not one of these.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockhashOnSign {
    /// Signed on the blockhash it has.
    Keep,
    /// Moved when the node answers, signed as it is when it does not.
    Fresh,
    /// The blockhash has expired: a press that cannot move it signs nothing.
    Required,
}

/// Where a transaction on the Sign screen stands with its blockhash.
#[derive(Clone, Copy, Debug)]
pub struct BlockhashFacts {
    pub online: bool,
    pub uses_nonce: bool,
    /// Signing moves it by itself, as it does a relayed transaction.
    pub refreshes_on_sign: bool,
    /// The countdown ran out, or the node said the blockhash is gone.
    pub clock_expired: bool,
    /// What the next press is for.
    pub step: NextSignature,
    /// Signatures this device can add now.
    pub signable: usize,
    /// It holds no signature over the transaction itself. An authorization does
    /// not count, because moving keeps one.
    pub untouched: bool,
    /// An authorization stands inside it.
    pub relayed: bool,
    pub from_browser: bool,
    /// Another screen of this app made it.
    pub app_built: bool,
}

/// What the blockhash of a transaction on the Sign screen means for its buttons.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlockhashPlan {
    pub on_sign: BlockhashOnSign,
    /// Expired, with nothing left here to sign: moving it is a press of its own.
    pub offer_refresh: bool,
    /// Expired, at a step where that matters. An authorization does not expire.
    pub expired: bool,
}

/// What a press does with the blockhash. It moves the transaction first when it
/// is about to sign the transaction itself, and only if the blockhash can still
/// be chosen. Before expiry, only a transaction this app made or relays is moved:
/// a site's, or another device's, is left as it came. Once expired, any
/// transaction on a blockhash is.
pub fn blockhash_plan(facts: BlockhashFacts) -> BlockhashPlan {
    let gone = facts.clock_expired && !facts.refreshes_on_sign;
    let this_apps = facts.untouched && ((facts.relayed && !facts.from_browser) || facts.app_built);
    let moves = facts.online
        && !facts.uses_nonce
        && !facts.refreshes_on_sign
        && facts.step == NextSignature::Transaction
        && facts.signable > 0
        && (gone || this_apps);
    BlockhashPlan {
        on_sign: match (moves, gone) {
            (false, _) => BlockhashOnSign::Keep,
            (true, false) => BlockhashOnSign::Fresh,
            (true, true) => BlockhashOnSign::Required,
        },
        offer_refresh: facts.online
            && !facts.uses_nonce
            && gone
            && !moves
            && facts.step != NextSignature::Authorization,
        expired: facts.clock_expired
            && matches!(
                facts.step,
                NextSignature::Transaction | NextSignature::Nothing
            ),
    }
}

/// What decides the buttons of the Sign screen.
#[derive(Clone, Copy, Debug)]
pub struct ButtonFacts {
    pub online: bool,
    /// A wallet that no key signs as is among the signers.
    pub needs_rebuild: bool,
    /// Something is under way for this transaction.
    pub busy: bool,
    pub step: NextSignature,
    /// Signatures this device can add now.
    pub signable: usize,
    /// Every signature is in.
    pub complete: bool,
    pub refreshes_on_sign: bool,
    pub blockhash: BlockhashPlan,
    /// A site that has it back may be sending it, or it has landed.
    pub site_holds: bool,
    /// It says yes to something it does not hold, and that is not settled:
    /// see [`blind_offer`].
    pub blind_holds: bool,
    /// It is still being read, so the screen does not say yet what it does.
    pub reading: bool,
    /// The node has not yet said what this very message does. Online, the
    /// transaction is signed only once it has: after an authorization that is
    /// what shows what the authorized transaction runs, and what fits its limits.
    pub simulating: bool,
}

/// Whether Sign can be pressed. An expired blockhash holds it back only if the
/// press would not move the transaction off it. The transaction itself waits for
/// its simulation; an authorization does not, since nothing simulates before it.
pub fn may_sign(facts: ButtonFacts) -> bool {
    facts.signable > 0
        && !facts.needs_rebuild
        && !facts.busy
        && !facts.blind_holds
        && !facts.reading
        && !(facts.simulating && facts.step == NextSignature::Transaction)
        && (facts.step == NextSignature::Authorization
            || !facts.blockhash.expired
            || facts.refreshes_on_sign
            || facts.blockhash.on_sign != BlockhashOnSign::Keep)
}

/// What decides whether a Sign press sends the transaction it signed.
#[derive(Clone, Copy, Debug)]
pub struct SignedFacts {
    pub online: bool,
    /// A site asked for it. It goes back to the site, which sends it, or the app
    /// sends it for the site when that is what the site asked: never both.
    pub from_site: bool,
    /// The press signed the transaction itself, not only an authorization.
    pub signed_transaction: bool,
    /// Every signature is in, authorizations and transaction.
    pub complete: bool,
}

/// Whether a Sign press goes on to submit what it signed. The press that signs
/// the transaction is the last informed step: it comes after the simulation, so
/// once every signature is in there is nothing left to decide.
pub fn submits_on_sign(facts: SignedFacts) -> bool {
    facts.online && !facts.from_site && facts.signed_transaction && facts.complete
}

/// Whether Submit can be pressed.
pub fn may_submit(facts: ButtonFacts) -> bool {
    facts.online
        && facts.complete
        && !facts.needs_rebuild
        && !facts.busy
        && !facts.blockhash.expired
        && !facts.site_holds
}

/// Where the instructions of a transaction that say yes to something it does
/// not hold stand: see `wallet_core::blind_in`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlindFacts {
    pub approvals: usize,
    /// How many of them a node has said what they say yes to. What another
    /// device said does not count: a device that cannot ask cannot check it.
    pub read: usize,
    /// A node is still being asked.
    pub reading: bool,
    /// The user said they have checked what this message says yes to.
    pub checked: bool,
}

/// What those instructions ask of the Sign screen.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BlindOffer {
    /// Ask the user to say they have checked it somewhere they trust.
    pub ask: bool,
    pub holds_sign: bool,
}

/// Sign waits for a node that is being asked. When no node has said what one of
/// them says yes to, which is always so on a device with no network, it waits
/// for the user's word that they have checked it somewhere they trust.
pub fn blind_offer(facts: BlindFacts) -> BlindOffer {
    let ask = !facts.reading && facts.read < facts.approvals;
    BlindOffer {
        ask,
        holds_sign: facts.reading || (ask && !facts.checked),
    }
}

/// Where a transaction stands for adding what the proposal it approves holds,
/// so the device that signs can read it: see [`crate::proposal_checks`].
#[derive(Clone, Debug)]
pub struct CheckFacts {
    pub online: bool,
    /// What its own instructions are to the clear signing program.
    pub shape: ApprovalShape,
    /// A node said what the proposal voted on stores, and it is a vault
    /// transaction this app reads: no config change, batch or lookup table.
    pub stored_read: bool,
    /// The vote's signature is not made yet and has to come from off this
    /// device: another device, or a hardware wallet. Not a hot key here, for
    /// which this device reads the proposal from its node already.
    pub signs_elsewhere: bool,
    /// The clear signing program runs on the cluster in use.
    pub program_on_cluster: bool,
    /// A wallet without a key is asked to sign: it is rebuilt first.
    pub needs_rebuild: bool,
    /// v1, the one format another device takes.
    pub shareable: bool,
    /// Signatures already made over the transaction.
    pub signatures_held: usize,
}

/// What the Sign screen offers about adding what a proposal holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckOffer {
    Nothing,
    /// Offer to add it. `voids` signatures already made would no longer apply.
    Add {
        voids: usize,
    },
    /// It would be offered but for this, which the user can change.
    CannotAdd(CheckBlock),
}

/// Why what a proposal holds cannot be added to votes that would otherwise
/// take it: the clear signing program deployed does not take it beside
/// this (`wallet_core::squads::ClearSigningAccepts`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckBlock {
    /// It lives on a durable nonce, whose System instruction the program
    /// refuses.
    DurableNonce,
    /// It executes the proposal too.
    Execute,
    /// Several members vote in it.
    SeveralVotes,
    /// The member votes through a wallet without a key, a programmatic
    /// signer or a Vector wallet, and the program does not take a check
    /// inside that wallet's `Submit` or `Passthrough`. The one deployed on
    /// devnet takes both.
    Authorization,
}

/// Offered only online, on a cluster where the program runs, for v1 votes
/// on a proposal whose stored vault transaction a node returned, when a
/// vote's signature comes from off this device, and when nothing else has to
/// be rebuilt first. A transaction with anything else in it gets nothing, and
/// one whose votes are all checked needs nothing. Of the rest, what `accepts`
/// says the program does not take is blocked, with the reason.
pub fn check_offer(facts: CheckFacts, accepts: ClearSigningAccepts) -> CheckOffer {
    let applies = facts.online
        && facts.program_on_cluster
        && facts.shareable
        && !facts.needs_rebuild
        && facts.stored_read
        && facts.signs_elsewhere;
    if !applies {
        return CheckOffer::Nothing;
    }
    let open = match facts.shape {
        ApprovalShape::Open(open) => open,
        ApprovalShape::Other | ApprovalShape::Checked => return CheckOffer::Nothing,
    };
    match accepts.refuses(open.facts()) {
        None => CheckOffer::Add {
            voids: facts.signatures_held,
        },
        Some(ShapeRefused::DurableNonce) => CheckOffer::CannotAdd(CheckBlock::DurableNonce),
        Some(ShapeRefused::Execute) => CheckOffer::CannotAdd(CheckBlock::Execute),
        Some(ShapeRefused::SeveralVotes) => CheckOffer::CannotAdd(CheckBlock::SeveralVotes),
        Some(ShapeRefused::InSubmit | ShapeRefused::InPassthrough) => {
            CheckOffer::CannotAdd(CheckBlock::Authorization)
        }
    }
}

/// Where one transaction of a batch stands on the Sign screen.
#[derive(Clone, Copy, Debug)]
pub struct BatchFacts {
    /// It belongs to a batch.
    pub in_batch: bool,
    /// It has been signed, or taken as it is, already.
    pub already_kept: bool,
    /// The site's request is still waiting.
    pub site_waiting: bool,
    pub signable: usize,
    pub needs_rebuild: bool,
    /// A key here is ready for the transaction, once an authorization comes.
    pub waiting_for_authority: bool,
    pub step: NextSignature,
    /// Signatures only another device can add.
    pub missing_elsewhere: usize,
    /// How many of those the site adds itself.
    pub left_for_site: usize,
    pub expired: bool,
}

/// Nothing here to sign, and nothing missing but what the site adds: the batch
/// can take the transaction as it is.
pub fn batch_keepable(facts: BatchFacts) -> bool {
    facts.in_batch
        && !facts.already_kept
        && facts.site_waiting
        && facts.signable == 0
        && !facts.needs_rebuild
        && !facts.waiting_for_authority
        && facts.step != NextSignature::Authorization
        && facts.missing_elsewhere == facts.left_for_site
        && !facts.expired
}

/// What the node said it would do with a site's transaction, in the request modal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Simulated {
    /// It has not answered yet, or has not been asked.
    Pending,
    Succeeds,
    /// It would fail.
    Fails,
    /// The node could not say.
    Unavailable,
}

/// Where a site's transaction stands in the request modal.
#[derive(Clone, Copy, Debug)]
pub struct ModalFacts {
    /// Preparing it finished.
    pub prepared: bool,
    /// The vault is locked. Preparing may rebuild it around a wallet's keys, so
    /// nothing goes further until it is open.
    pub locked: bool,
    /// It was rebuilt for a keyless wallet.
    pub rebuilt: bool,
    /// A key made here took the place of a site's one-time key.
    pub keys_replaced: bool,
    /// Authorizations this device can sign.
    pub authorizations_here: usize,
    pub verdict: Option<QuickSign>,
    /// Signatures the site adds itself once it has the transaction back.
    pub left_for_site: usize,
    /// IDLs for what it shows are still being looked for.
    pub looking_for_idls: bool,
    pub simulated: Simulated,
    pub clock_expired: bool,
    pub step: Option<NextSignature>,
}

/// What the request modal offers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ModalOffer {
    pub review: bool,
    /// Signing here, with no further step, finishes the request.
    pub sign: bool,
    /// Sign is offered, but something says to look twice: not every instruction
    /// was put in words by an IDL that ships with the app or that the user
    /// imported, or the node did not say it would succeed.
    pub cautious_sign: bool,
    /// Expired, at a step where that matters.
    pub expired: bool,
}

/// What the modal offers. Sign means signing completes the transaction now, so
/// not for one that still has to simulate or has an authorization to sign here:
/// that changes what runs, so Review signs it, simulates again and fits the
/// limits, and the transaction is signed on a later press. Nor for one that hands
/// over control of something, however well it reads, which takes the deliberate
/// step of Review. Nor before the node has said what it would do.
pub fn modal_offer(facts: ModalFacts) -> ModalOffer {
    let review = facts.prepared && !facts.locked;
    let expired = facts.clock_expired
        && matches!(
            facts.step,
            Some(NextSignature::Transaction | NextSignature::Nothing)
        );
    // A rebuild with a key made here simulates only once its authorization is
    // signed, which Review does before anything is sent.
    let unsimulated = facts.rebuilt && facts.keys_replaced;
    let sign = review
        && !unsimulated
        && facts.authorizations_here == 0
        && facts.verdict.is_some_and(|verdict| {
            verdict.completes_leaving(facts.left_for_site) && verdict.risky == 0
        })
        && !facts.looking_for_idls
        && facts.simulated != Simulated::Pending
        && !expired;
    // A summary missing an instruction, or worded by an IDL from the chain, still
    // lets Sign through, and so does a node that does not say it would succeed.
    // The button turns amber to say so.
    let cautious_sign = sign
        && (facts.simulated != Simulated::Succeeds
            || facts
                .verdict
                .is_some_and(|verdict| !verdict.allowed_leaving(facts.left_for_site)));
    ModalOffer {
        review,
        sign,
        cautious_sign,
        expired,
    }
}

/// The app cannot send what a site still has to sign.
pub fn can_go_out(sends: bool, left_for_site: usize) -> bool {
    !sends || left_for_site == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use NextSignature::{Authorization, Nothing, Transaction};

    fn a_vote(member: u8) -> wallet_core::V4Vote {
        let squad = wallet_core::squads::Squad::v4(&solana_pubkey::Pubkey::new_from_array([4; 32]));
        wallet_core::V4Vote {
            squad,
            proposal: squad.proposal(1),
            member: solana_pubkey::Pubkey::new_from_array([member; 32]),
            carried: false,
        }
    }

    /// One member's vote, alone.
    fn one_vote() -> wallet_core::OpenApproval {
        let vote = a_vote(5);
        wallet_core::OpenApproval {
            squad: vote.squad,
            proposal: vote.proposal,
            votes: vec![vote],
            execute: false,
            nonce: false,
            in_submit: false,
            in_passthrough: false,
        }
    }

    /// `open` with every vote run by a programmatic signer's `Submit`.
    fn in_submit(open: wallet_core::OpenApproval) -> wallet_core::OpenApproval {
        let votes = open
            .votes
            .iter()
            .map(|vote| wallet_core::V4Vote {
                carried: true,
                ..*vote
            })
            .collect();
        wallet_core::OpenApproval {
            votes,
            in_submit: true,
            ..open
        }
    }

    /// `open` with every vote run by a Vector `Passthrough`.
    fn in_passthrough(open: wallet_core::OpenApproval) -> wallet_core::OpenApproval {
        wallet_core::OpenApproval {
            in_submit: false,
            in_passthrough: true,
            ..in_submit(open)
        }
    }

    /// A lone vote through each kind of wallet without a key.
    fn carried_votes() -> [wallet_core::OpenApproval; 2] {
        [in_submit(one_vote()), in_passthrough(one_vote())]
    }

    /// Each shape beyond a vote alone, and why the deployed program blocks
    /// it, whether the member signs with its key or through a program.
    fn wider() -> Vec<(wallet_core::OpenApproval, CheckBlock)> {
        let top = vec![
            (
                wallet_core::OpenApproval {
                    nonce: true,
                    ..one_vote()
                },
                CheckBlock::DurableNonce,
            ),
            (
                wallet_core::OpenApproval {
                    execute: true,
                    ..one_vote()
                },
                CheckBlock::Execute,
            ),
            (
                wallet_core::OpenApproval {
                    votes: vec![a_vote(5), a_vote(6)],
                    ..one_vote()
                },
                CheckBlock::SeveralVotes,
            ),
            (
                wallet_core::OpenApproval {
                    votes: vec![a_vote(5), a_vote(6)],
                    execute: true,
                    ..one_vote()
                },
                CheckBlock::Execute,
            ),
        ];
        // A keyless wallet's rebuild takes no durable nonce, so that shape
        // is never carried.
        let carried: Vec<_> = top
            .iter()
            .filter(|(open, _)| !open.nonce)
            .flat_map(|(open, block)| {
                [
                    (in_submit(open.clone()), *block),
                    (in_passthrough(open.clone()), *block),
                ]
            })
            .collect();
        top.into_iter().chain(carried).collect()
    }

    /// A lone approval waiting on an offline member, online on devnet.
    fn lone() -> CheckFacts {
        CheckFacts {
            online: true,
            shape: ApprovalShape::Open(one_vote()),
            stored_read: true,
            signs_elsewhere: true,
            program_on_cluster: true,
            needs_rebuild: false,
            shareable: true,
            signatures_held: 0,
        }
    }

    const DEPLOYED: ClearSigningAccepts = ClearSigningAccepts::DEPLOYED;
    const EVERYTHING: ClearSigningAccepts = ClearSigningAccepts::EVERYTHING;

    #[test]
    fn a_lone_vote_from_elsewhere_on_a_stored_transaction_is_offered_the_checks() {
        for accepts in [DEPLOYED, EVERYTHING] {
            assert_eq!(check_offer(lone(), accepts), CheckOffer::Add { voids: 0 });
            // Signatures made already are said to be void.
            let signed = CheckFacts {
                signatures_held: 2,
                ..lone()
            };
            assert_eq!(check_offer(signed, accepts), CheckOffer::Add { voids: 2 });
        }
    }

    #[test]
    fn a_lone_vote_through_a_programmatic_signer_or_a_vector_wallet_is_offered_the_checks() {
        for open in carried_votes() {
            let facts = CheckFacts {
                shape: ApprovalShape::Open(open.clone()),
                ..lone()
            };
            for accepts in [DEPLOYED, EVERYTHING] {
                assert_eq!(
                    check_offer(facts.clone(), accepts),
                    CheckOffer::Add { voids: 0 },
                    "{open:?}"
                );
            }
            // A program that took neither carrier would block it, saying why.
            let top_only = ClearSigningAccepts {
                in_submit: false,
                in_passthrough: false,
                ..DEPLOYED
            };
            assert_eq!(
                check_offer(facts, top_only),
                CheckOffer::CannotAdd(CheckBlock::Authorization),
                "{open:?}"
            );
        }
    }

    #[test]
    fn each_condition_alone_takes_the_offer_away() {
        let without: [fn(&mut CheckFacts); 6] = [
            |f| f.online = false,
            |f| f.stored_read = false,
            |f| f.signs_elsewhere = false,
            |f| f.program_on_cluster = false,
            |f| f.needs_rebuild = true,
            |f| f.shareable = false,
        ];
        let shapes = std::iter::once(one_vote())
            .chain(carried_votes())
            .chain(wider().into_iter().map(|(open, _)| open));
        for open in shapes {
            for (i, change) in without.iter().enumerate() {
                for accepts in [DEPLOYED, EVERYTHING] {
                    let mut facts = CheckFacts {
                        shape: ApprovalShape::Open(open.clone()),
                        ..lone()
                    };
                    change(&mut facts);
                    assert_eq!(
                        check_offer(facts, accepts),
                        CheckOffer::Nothing,
                        "{i} {open:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn a_shape_the_deployed_program_refuses_says_why_and_is_offered_once_it_takes_it() {
        for (open, block) in wider() {
            let facts = CheckFacts {
                shape: ApprovalShape::Open(open.clone()),
                ..lone()
            };
            assert_eq!(
                check_offer(facts.clone(), DEPLOYED),
                CheckOffer::CannotAdd(block),
                "{open:?}"
            );
            assert_eq!(
                check_offer(facts, EVERYTHING),
                CheckOffer::Add { voids: 0 },
                "{open:?}"
            );
        }
        // Taking one thing more does not take the rest.
        let several = CheckFacts {
            shape: ApprovalShape::Open(wallet_core::OpenApproval {
                votes: vec![a_vote(5), a_vote(6)],
                execute: true,
                ..one_vote()
            }),
            ..lone()
        };
        let execute_only = ClearSigningAccepts {
            execute: true,
            ..DEPLOYED
        };
        assert_eq!(
            check_offer(several, execute_only),
            CheckOffer::CannotAdd(CheckBlock::SeveralVotes)
        );
        for accepts in [DEPLOYED, EVERYTHING] {
            for shape in [ApprovalShape::Other, ApprovalShape::Checked] {
                let facts = CheckFacts {
                    shape: shape.clone(),
                    ..lone()
                };
                assert_eq!(check_offer(facts, accepts), CheckOffer::Nothing);
            }
        }
    }

    /// A transaction a site sent, on a live blockhash, with this device's key
    /// still to sign, on an online device.
    fn from_a_site() -> BlockhashFacts {
        BlockhashFacts {
            online: true,
            uses_nonce: false,
            refreshes_on_sign: false,
            clock_expired: false,
            step: Transaction,
            signable: 1,
            untouched: true,
            relayed: false,
            from_browser: true,
            app_built: false,
        }
    }

    fn on_sign(facts: BlockhashFacts) -> BlockhashOnSign {
        blockhash_plan(facts).on_sign
    }

    #[test]
    fn a_sites_transaction_keeps_its_blockhash_until_it_expires() {
        assert_eq!(on_sign(from_a_site()), BlockhashOnSign::Keep);
        // Even with an authorization in it: it is the site's, and it may hold
        // on to what it sent.
        let relayed = BlockhashFacts {
            relayed: true,
            ..from_a_site()
        };
        assert_eq!(on_sign(relayed), BlockhashOnSign::Keep);
        let expired = BlockhashFacts {
            clock_expired: true,
            ..from_a_site()
        };
        assert_eq!(on_sign(expired), BlockhashOnSign::Required);
    }

    #[test]
    fn what_this_app_made_or_relays_moves_on_the_press_that_signs_it() {
        let built = BlockhashFacts {
            from_browser: false,
            app_built: true,
            ..from_a_site()
        };
        assert_eq!(on_sign(built), BlockhashOnSign::Fresh);
        let relayed = BlockhashFacts {
            from_browser: false,
            relayed: true,
            ..from_a_site()
        };
        assert_eq!(on_sign(relayed), BlockhashOnSign::Fresh);
    }

    #[test]
    fn a_signature_over_it_keeps_it_on_its_blockhash_until_it_expires() {
        let signed = BlockhashFacts {
            from_browser: false,
            app_built: true,
            untouched: false,
            ..from_a_site()
        };
        assert_eq!(on_sign(signed), BlockhashOnSign::Keep);
        assert_eq!(
            on_sign(BlockhashFacts {
                clock_expired: true,
                ..signed
            }),
            BlockhashOnSign::Required
        );
    }

    #[test]
    fn nothing_moves_what_cannot_or_need_not_be_moved() {
        let expired_built = BlockhashFacts {
            from_browser: false,
            app_built: true,
            clock_expired: true,
            ..from_a_site()
        };
        assert_eq!(on_sign(expired_built), BlockhashOnSign::Required);
        for keeps in [
            // No node to ask for a new one.
            BlockhashFacts {
                online: false,
                ..expired_built
            },
            // A durable nonce does not expire, and has no blockhash to move.
            BlockhashFacts {
                uses_nonce: true,
                ..expired_built
            },
            // Signing moves it by itself.
            BlockhashFacts {
                refreshes_on_sign: true,
                ..expired_built
            },
            // Nothing for this press to sign.
            BlockhashFacts {
                signable: 0,
                ..expired_built
            },
            // The press signs an authorization, which moving would not touch.
            BlockhashFacts {
                step: Authorization,
                ..expired_built
            },
            BlockhashFacts {
                step: Nothing,
                ..expired_built
            },
        ] {
            assert_eq!(on_sign(keeps), BlockhashOnSign::Keep, "{keeps:?}");
        }
    }

    #[test]
    fn moving_is_a_press_of_its_own_with_nothing_left_here_to_sign() {
        let expired = BlockhashFacts {
            clock_expired: true,
            ..from_a_site()
        };
        // There is a signature to make, and the press moves it.
        assert!(!blockhash_plan(expired).offer_refresh);
        let nothing_to_sign = BlockhashFacts {
            signable: 0,
            step: Nothing,
            ..expired
        };
        assert!(blockhash_plan(nothing_to_sign).offer_refresh);

        for not_offered in [
            BlockhashFacts {
                clock_expired: false,
                ..nothing_to_sign
            },
            BlockhashFacts {
                online: false,
                ..nothing_to_sign
            },
            BlockhashFacts {
                uses_nonce: true,
                ..nothing_to_sign
            },
            BlockhashFacts {
                refreshes_on_sign: true,
                ..nothing_to_sign
            },
            BlockhashFacts {
                step: Authorization,
                ..nothing_to_sign
            },
        ] {
            assert!(
                !blockhash_plan(not_offered).offer_refresh,
                "{not_offered:?}"
            );
        }
    }

    #[test]
    fn an_authorization_does_not_expire() {
        let at = |step, clock_expired| {
            blockhash_plan(BlockhashFacts {
                step,
                clock_expired,
                ..from_a_site()
            })
            .expired
        };
        assert!(at(Transaction, true));
        assert!(at(Nothing, true));
        assert!(!at(Authorization, true));
        assert!(!at(Transaction, false));
    }

    fn keep() -> BlockhashPlan {
        BlockhashPlan {
            on_sign: BlockhashOnSign::Keep,
            offer_refresh: false,
            expired: false,
        }
    }

    /// One this device can sign, on a live blockhash, with nothing under way.
    fn ready() -> ButtonFacts {
        ButtonFacts {
            online: true,
            needs_rebuild: false,
            busy: false,
            step: Transaction,
            signable: 1,
            complete: false,
            refreshes_on_sign: false,
            blockhash: keep(),
            site_holds: false,
            blind_holds: false,
            reading: false,
            simulating: false,
        }
    }

    #[test]
    fn saying_yes_to_what_is_not_held_waits_for_a_node_or_the_users_word() {
        let facts = |approvals, read, reading, checked| BlindFacts {
            approvals,
            read,
            reading,
            checked,
        };
        // Nothing of the kind on screen.
        assert_eq!(
            blind_offer(facts(0, 0, false, false)),
            BlindOffer::default()
        );
        // With no network nothing can be read: only the user can settle it.
        assert_eq!(
            blind_offer(facts(1, 0, false, false)),
            BlindOffer {
                ask: true,
                holds_sign: true
            }
        );
        assert_eq!(
            blind_offer(facts(1, 0, false, true)),
            BlindOffer {
                ask: true,
                holds_sign: false
            }
        );
        // A node being asked holds Sign and asks nothing yet, even if ticked.
        for checked in [false, true] {
            assert_eq!(
                blind_offer(facts(1, 0, true, checked)),
                BlindOffer {
                    ask: false,
                    holds_sign: true
                }
            );
        }
        // A node said for all of them: there is nothing to ask.
        assert_eq!(
            blind_offer(facts(2, 2, false, false)),
            BlindOffer::default()
        );
        // For only some of them: the rest still need the user.
        assert_eq!(
            blind_offer(facts(2, 1, false, false)),
            BlindOffer {
                ask: true,
                holds_sign: true
            }
        );
    }

    #[test]
    fn sign_needs_something_to_sign_and_nothing_in_the_way() {
        assert!(may_sign(ready()));
        for held in [
            ButtonFacts {
                reading: true,
                ..ready()
            },
            // Held even for the authorization, which is a signature too.
            ButtonFacts {
                blind_holds: true,
                ..ready()
            },
            ButtonFacts {
                blind_holds: true,
                step: Authorization,
                ..ready()
            },
            ButtonFacts {
                signable: 0,
                ..ready()
            },
            ButtonFacts {
                needs_rebuild: true,
                ..ready()
            },
            ButtonFacts {
                busy: true,
                ..ready()
            },
        ] {
            assert!(!may_sign(held), "{held:?}");
        }
    }

    #[test]
    fn the_transaction_waits_for_its_simulation_and_an_authorization_does_not() {
        let simulating = ButtonFacts {
            simulating: true,
            ..ready()
        };
        assert!(!may_sign(simulating));
        assert!(may_sign(ButtonFacts {
            step: Authorization,
            ..simulating
        }));
        // Submit has nothing to wait for: what it sends is already signed.
        assert!(may_submit(ButtonFacts {
            complete: true,
            signable: 0,
            step: Nothing,
            ..simulating
        }));
    }

    #[test]
    fn signing_the_transaction_submits_it_unless_a_site_asked_for_it() {
        let last = SignedFacts {
            online: true,
            from_site: false,
            signed_transaction: true,
            complete: true,
        };
        assert!(submits_on_sign(last));
        for held in [
            // The site sends it, or has the app send it, as it asked.
            SignedFacts {
                from_site: true,
                ..last
            },
            // No node: it is shared to a device that has one.
            SignedFacts {
                online: false,
                ..last
            },
            // Only an authorization: the transaction is signed on the next press.
            SignedFacts {
                signed_transaction: false,
                ..last
            },
            // Another device still signs.
            SignedFacts {
                complete: false,
                ..last
            },
        ] {
            assert!(!submits_on_sign(held), "{held:?}");
        }
    }

    #[test]
    fn an_expired_blockhash_holds_sign_back_unless_the_press_moves_it() {
        let expired = ButtonFacts {
            blockhash: BlockhashPlan {
                expired: true,
                ..keep()
            },
            ..ready()
        };
        assert!(!may_sign(expired));
        assert!(may_sign(ButtonFacts {
            refreshes_on_sign: true,
            ..expired
        }));
        assert!(may_sign(ButtonFacts {
            blockhash: BlockhashPlan {
                expired: true,
                on_sign: BlockhashOnSign::Required,
                offer_refresh: false,
            },
            ..expired
        }));
        assert!(may_sign(ButtonFacts {
            step: Authorization,
            ..expired
        }));
    }

    #[test]
    fn submit_needs_every_signature_a_node_and_a_live_blockhash() {
        let complete = ButtonFacts {
            complete: true,
            signable: 0,
            step: Nothing,
            ..ready()
        };
        assert!(may_submit(complete));
        for held in [
            ButtonFacts {
                complete: false,
                ..complete
            },
            ButtonFacts {
                online: false,
                ..complete
            },
            ButtonFacts {
                needs_rebuild: true,
                ..complete
            },
            ButtonFacts {
                busy: true,
                ..complete
            },
            ButtonFacts {
                site_holds: true,
                ..complete
            },
            ButtonFacts {
                blockhash: BlockhashPlan {
                    expired: true,
                    ..keep()
                },
                ..complete
            },
        ] {
            assert!(!may_submit(held), "{held:?}");
        }
    }

    /// One of a batch with nothing here to sign, and nothing else missing.
    fn nothing_to_add() -> BatchFacts {
        BatchFacts {
            in_batch: true,
            already_kept: false,
            site_waiting: true,
            signable: 0,
            needs_rebuild: false,
            waiting_for_authority: false,
            step: Nothing,
            missing_elsewhere: 0,
            left_for_site: 0,
            expired: false,
        }
    }

    #[test]
    fn a_batch_takes_a_transaction_only_with_nothing_missing_but_the_sites_own() {
        assert!(batch_keepable(nothing_to_add()));
        assert!(batch_keepable(BatchFacts {
            missing_elsewhere: 2,
            left_for_site: 2,
            ..nothing_to_add()
        }));
        for refused in [
            BatchFacts {
                in_batch: false,
                ..nothing_to_add()
            },
            BatchFacts {
                already_kept: true,
                ..nothing_to_add()
            },
            BatchFacts {
                site_waiting: false,
                ..nothing_to_add()
            },
            // This device could sign it, so it should.
            BatchFacts {
                signable: 1,
                ..nothing_to_add()
            },
            BatchFacts {
                needs_rebuild: true,
                ..nothing_to_add()
            },
            BatchFacts {
                waiting_for_authority: true,
                ..nothing_to_add()
            },
            BatchFacts {
                step: Authorization,
                ..nothing_to_add()
            },
            // Someone other than the site has yet to sign it.
            BatchFacts {
                missing_elsewhere: 2,
                left_for_site: 1,
                ..nothing_to_add()
            },
            BatchFacts {
                expired: true,
                ..nothing_to_add()
            },
        ] {
            assert!(!batch_keepable(refused), "{refused:?}");
        }
    }

    fn verdict() -> QuickSign {
        QuickSign {
            unread: 0,
            unvetted: 0,
            elsewhere: 0,
            signable: 1,
            risky: 0,
        }
    }

    /// Prepared, unlocked, read in full, and finished by signing here.
    fn clear() -> ModalFacts {
        ModalFacts {
            prepared: true,
            locked: false,
            rebuilt: false,
            keys_replaced: false,
            authorizations_here: 0,
            verdict: Some(verdict()),
            left_for_site: 0,
            looking_for_idls: false,
            simulated: Simulated::Succeeds,
            clock_expired: false,
            step: Some(Transaction),
        }
    }

    #[test]
    fn the_modal_offers_sign_when_signing_here_finishes_it() {
        let offer = modal_offer(clear());
        assert!(offer.review && offer.sign && !offer.cautious_sign && !offer.expired);
    }

    #[test]
    fn nothing_goes_further_until_it_is_prepared_and_the_vault_is_open() {
        for held in [
            ModalFacts {
                prepared: false,
                ..clear()
            },
            ModalFacts {
                locked: true,
                ..clear()
            },
        ] {
            assert_eq!(modal_offer(held), ModalOffer::default(), "{held:?}");
        }
    }

    #[test]
    fn what_needs_another_step_goes_to_review_not_sign() {
        for review_only in [
            // It can only simulate once its authorization is signed.
            ModalFacts {
                rebuilt: true,
                keys_replaced: true,
                ..clear()
            },
            ModalFacts {
                authorizations_here: 1,
                ..clear()
            },
            ModalFacts {
                verdict: None,
                ..clear()
            },
            // Another device has yet to sign it.
            ModalFacts {
                verdict: Some(QuickSign {
                    elsewhere: 1,
                    ..verdict()
                }),
                ..clear()
            },
            // Nothing here to sign.
            ModalFacts {
                verdict: Some(QuickSign {
                    signable: 0,
                    ..verdict()
                }),
                ..clear()
            },
            ModalFacts {
                looking_for_idls: true,
                ..clear()
            },
            ModalFacts {
                clock_expired: true,
                ..clear()
            },
            // It hands over control of something, however well it reads.
            ModalFacts {
                verdict: Some(QuickSign {
                    risky: 1,
                    ..verdict()
                }),
                ..clear()
            },
            // The node has not said what it would do.
            ModalFacts {
                simulated: Simulated::Pending,
                ..clear()
            },
        ] {
            let offer = modal_offer(review_only);
            assert!(offer.review && !offer.sign, "{review_only:?}");
            assert!(!offer.cautious_sign);
        }

        // A rebuild alone, or a key replaced with no rebuild, simulates as it is.
        for signs in [
            ModalFacts {
                rebuilt: true,
                ..clear()
            },
            ModalFacts {
                keys_replaced: true,
                ..clear()
            },
        ] {
            assert!(modal_offer(signs).sign, "{signs:?}");
        }
    }

    #[test]
    fn what_a_site_signs_after_this_device_does_not_hold_sign_back() {
        let left = ModalFacts {
            verdict: Some(QuickSign {
                elsewhere: 1,
                ..verdict()
            }),
            left_for_site: 1,
            ..clear()
        };
        assert!(modal_offer(left).sign);
    }

    #[test]
    fn sign_is_marked_when_the_summary_cannot_be_trusted() {
        for marked in [
            QuickSign {
                unread: 1,
                ..verdict()
            },
            QuickSign {
                unvetted: 1,
                ..verdict()
            },
        ] {
            let offer = modal_offer(ModalFacts {
                verdict: Some(marked),
                ..clear()
            });
            assert!(offer.sign && offer.cautious_sign, "{marked:?}");
        }
    }

    #[test]
    fn sign_is_marked_when_the_node_does_not_say_it_would_succeed() {
        for simulated in [Simulated::Fails, Simulated::Unavailable] {
            let offer = modal_offer(ModalFacts {
                simulated,
                ..clear()
            });
            assert!(offer.sign && offer.cautious_sign, "{simulated:?}");
        }
    }

    #[test]
    fn an_expired_blockhash_holds_the_modal_back_only_at_a_step_it_matters() {
        let at = |step| {
            modal_offer(ModalFacts {
                clock_expired: true,
                step,
                ..clear()
            })
        };
        assert!(at(Some(Transaction)).expired);
        assert!(at(Some(Nothing)).expired);
        assert!(!at(Some(Authorization)).expired);
        assert!(!at(None).expired);
        assert!(at(Some(Authorization)).sign);
    }

    #[test]
    fn the_app_sends_only_what_no_site_still_has_to_sign() {
        assert!(can_go_out(true, 0));
        assert!(!can_go_out(true, 1));
        assert!(can_go_out(false, 1));
    }
}
