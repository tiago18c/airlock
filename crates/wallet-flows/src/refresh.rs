//! Reading what a node knows about a set of addresses.

use solana_pubkey::Pubkey;
use wallet_core::{parse_nonce_account, ChainSnapshot, Lifetime, RpcError, SolanaRpc};

/// Fetch the one thing a transaction's lifetime needs, in one request: the
/// state of `nonce` when a durable nonce is used, the latest blockhash
/// otherwise.
pub async fn fetch_lifetime(
    rpc: &dyn SolanaRpc,
    nonce: Option<Pubkey>,
) -> Result<Lifetime, RpcError> {
    let Some(pubkey) = nonce else {
        return rpc.get_latest_blockhash().await.map(Lifetime::Blockhash);
    };
    let account = rpc
        .get_account(&pubkey)
        .await?
        .ok_or_else(|| RpcError::Message(format!("Nonce account {pubkey} does not exist.")))?;
    parse_nonce_account(pubkey, &account)
        .map(Lifetime::Nonce)
        .ok_or_else(|| RpcError::Message(format!("{pubkey} is not an initialized nonce account.")))
}

/// A node answers for at most this many accounts in one request.
const ACCOUNTS_PER_REQUEST: usize = 100;

/// The lamports of each of `owners`, one request per hundred. An address
/// with no account holds nothing.
pub async fn read_balances(
    rpc: &dyn SolanaRpc,
    owners: &[Pubkey],
) -> Result<Vec<(Pubkey, u64)>, RpcError> {
    let mut balances = Vec::with_capacity(owners.len());
    for batch in owners.chunks(ACCOUNTS_PER_REQUEST) {
        let held = rpc.get_lamports(batch).await?;
        balances.extend(
            batch
                .iter()
                .zip(held)
                .map(|(owner, lamports)| (*owner, lamports.unwrap_or(0))),
        );
    }
    Ok(balances)
}

/// [`read_balances`] from a node that has seen at least `min_slot`, with
/// the oldest slot a request was answered at.
pub async fn read_balances_since(
    rpc: &dyn SolanaRpc,
    owners: &[Pubkey],
    min_slot: u64,
) -> Result<(u64, Vec<(Pubkey, u64)>), RpcError> {
    let mut balances = Vec::with_capacity(owners.len());
    let mut slot = u64::MAX;
    for batch in owners.chunks(ACCOUNTS_PER_REQUEST) {
        let (answered, held) = rpc.get_lamports_since(batch, min_slot).await?;
        slot = slot.min(answered);
        balances.extend(
            batch
                .iter()
                .zip(held)
                .map(|(owner, lamports)| (*owner, lamports.unwrap_or(0))),
        );
    }
    Ok((if balances.is_empty() { min_slot } else { slot }, balances))
}

/// What reading the token accounts of some owners came to. What was read is
/// worth keeping, whatever came after it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TokenRead {
    pub snapshot: ChainSnapshot,
    /// How many owners were asked about, from the first, before anything stopped
    /// the read.
    pub asked: usize,
    /// How many of them the node answered with an error. Each is left out of
    /// [`ChainSnapshot::token_owners`], so what the app holds for it is kept.
    pub refused: usize,
    /// What the first of those errors said.
    pub why_refused: Option<String>,
    /// What stopped the read before it got through them. A node that does not
    /// answer, or says it is being asked too often, would only do so again for the
    /// next owner.
    pub stopped: Option<RpcError>,
}

impl TokenRead {
    /// What to tell of the owners the node refused, when there were any.
    pub fn refusal(&self) -> Option<String> {
        let why = self.why_refused.as_deref()?;
        Some(format!(
            "The node would not read the token accounts of {} address(es): {why}",
            self.refused
        ))
    }
}

/// The token accounts of each of `owners`.
pub async fn read_tokens(rpc: &dyn SolanaRpc, owners: &[Pubkey]) -> Result<TokenRead, RpcError> {
    read_tokens_since(rpc, owners, 0).await
}

/// [`read_tokens`] from a node that has seen at least `min_slot`. A node
/// behind it stops the read with [`RpcError::Behind`], to be asked again.
pub async fn read_tokens_since(
    rpc: &dyn SolanaRpc,
    owners: &[Pubkey],
    min_slot: u64,
) -> Result<TokenRead, RpcError> {
    let mut read = TokenRead::default();
    for owner in owners {
        match rpc.get_token_accounts_by_owner_since(owner, min_slot).await {
            Ok(mut tokens) => {
                read.snapshot.tokens.append(&mut tokens);
                read.snapshot.token_owners.push(*owner);
            }
            Err(RpcError::Behind) => return Err(RpcError::Behind),
            Err(err) if err.stops_a_read() => {
                read.stopped = Some(err);
                return Ok(read);
            }
            Err(err) => {
                read.refused += 1;
                read.why_refused.get_or_insert_with(|| err.to_string());
            }
        }
        read.asked += 1;
    }
    Ok(read)
}

/// The most owners one background read asks about. Each takes two requests, one
/// per token program, and a public node allows about forty of those in ten seconds.
pub const TOKENS_PER_TURN: usize = 20;

/// What is left of a turn for the book, however many wallets there are.
const BOOK_SHARE: usize = 5;

/// Whose token accounts one background read asks about.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TokenTurn {
    pub owners: Vec<Pubkey>,
    /// How many of them, from the first, are wallets.
    wallets: usize,
}

/// Whose turn it is. A book can hold more addresses than a node will answer for at
/// once, so each read takes some, and the next read goes on from there. Wallets come
/// first and more often, since they are what someone here acts with.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TokenTurns {
    wallets: usize,
    book: usize,
}

impl TokenTurns {
    /// Who the next read asks about. A list that has changed since only moves
    /// where the turns fall.
    pub fn next(&self, wallets: &[Pubkey], book: &[Pubkey]) -> TokenTurn {
        let for_wallets = if book.is_empty() {
            TOKENS_PER_TURN
        } else {
            TOKENS_PER_TURN - BOOK_SHARE
        };
        let wallet_count = wallets.len().min(for_wallets);
        let book_count = book.len().min(TOKENS_PER_TURN - wallet_count);
        let from = |list: &[Pubkey], start: usize, count: usize| -> Vec<Pubkey> {
            (0..count)
                .map(|at| list[(start + at) % list.len()])
                .collect()
        };
        let mut owners = from(wallets, self.wallets, wallet_count);
        owners.extend(from(book, self.book, book_count));
        TokenTurn {
            owners,
            wallets: wallet_count,
        }
    }

    /// Go on after the first `asked` of `turn`. Whoever a stopped read did not get
    /// to goes first next time.
    pub fn advance(&mut self, turn: &TokenTurn, asked: usize) {
        self.wallets = self.wallets.wrapping_add(asked.min(turn.wallets));
        self.book = self.book.wrapping_add(asked.saturating_sub(turn.wallets));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{nonce_account, pk, FakeRpc};
    use solana_hash::Hash;
    use wallet_core::{DiscoveredNonce, DiscoveredTokenAccount, Holdings, TokenFlag};

    #[test]
    fn a_failed_token_request_keeps_stored_holdings_and_flags() {
        let (owner, token) = (pk(1), pk(2));
        let mut holdings = Holdings::default();
        holdings.replace_owners(
            &[owner],
            &[DiscoveredTokenAccount {
                pubkey: token,
                owner,
                mint: pk(3),
                amount: 5,
                decimals: 0,
            }],
        );
        assert!(holdings.set_flag(&owner, &token, Some(TokenFlag::Red)));

        let rpc = FakeRpc {
            tokens_fail: true,
            ..FakeRpc::default()
        };
        let read = pollster::block_on(read_tokens(&rpc, &[owner])).unwrap();
        assert!(read.snapshot.token_owners.is_empty());
        assert_eq!((read.asked, read.refused), (1, 1));
        assert_eq!(read.stopped, None);
        assert_eq!(
            read.refusal().unwrap(),
            "The node would not read the token accounts of 1 address(es): token accounts are not served"
        );

        holdings.replace_owners(&read.snapshot.token_owners, &read.snapshot.tokens);
        assert_eq!(holdings.flag_of(&token), Some(TokenFlag::Red));
    }

    #[test]
    fn what_was_read_before_a_node_says_stop_is_kept_and_nothing_more_is_asked() {
        for stop in [RpcError::RateLimited, RpcError::Offline, RpcError::Timeout] {
            let rpc = FakeRpc {
                tokens_stop_after: Some((1, stop.clone())),
                ..FakeRpc::default()
            };
            let read = pollster::block_on(read_tokens(&rpc, &[pk(1), pk(2), pk(3)])).unwrap();
            assert_eq!(read.snapshot.token_owners, [pk(1)], "{stop}");
            assert_eq!(read.asked, 1);
            assert_eq!(read.stopped, Some(stop));
            // One answered, and one that said stop. The third was never asked.
            assert_eq!(rpc.asked(), 2);
        }
    }

    #[test]
    fn a_node_that_is_behind_is_asked_again_rather_than_taken_as_an_answer() {
        let rpc = FakeRpc::default();
        rpc.slot.store(5, std::sync::atomic::Ordering::Relaxed);
        assert_eq!(
            pollster::block_on(read_tokens_since(&rpc, &[pk(1)], 9)).unwrap_err(),
            RpcError::Behind
        );
    }

    fn pks(from: u8, count: u8) -> Vec<Pubkey> {
        (from..from + count).map(pk).collect()
    }

    #[test]
    fn everyone_gets_a_turn_and_a_turn_stays_within_what_a_node_allows() {
        let (wallets, book) = (pks(1, 3), pks(100, 50));
        let mut turns = TokenTurns::default();
        let mut seen = std::collections::HashMap::<Pubkey, usize>::new();
        for _ in 0..5 {
            let turn = turns.next(&wallets, &book);
            assert_eq!(turn.owners.len(), TOKENS_PER_TURN);
            for owner in &turn.owners {
                *seen.entry(*owner).or_default() += 1;
            }
            turns.advance(&turn, turn.owners.len());
        }
        // Each wallet every time. The book takes turns, 17 at a time, so each
        // of its 50 comes around once and some twice.
        for wallet in &wallets {
            assert_eq!(seen[wallet], 5);
        }
        for address in &book {
            assert!((1..=2).contains(&seen[address]), "{address}");
        }
    }

    #[test]
    fn many_wallets_take_turns_too_and_leave_the_book_a_share() {
        let (wallets, book) = (pks(1, 40), pks(100, 10));
        let mut turns = TokenTurns::default();
        let first = turns.next(&wallets, &book);
        assert_eq!(first.owners.len(), TOKENS_PER_TURN);
        assert_eq!(first.owners[..15], wallets[..15]);
        assert_eq!(first.owners[15..], book[..5]);
        turns.advance(&first, first.owners.len());
        let second = turns.next(&wallets, &book);
        assert_eq!(second.owners[..15], wallets[15..30]);
        assert_eq!(second.owners[15..], book[5..10]);

        // With no book the wallets have it all.
        let alone = TokenTurns::default().next(&pks(1, 40), &[]);
        assert_eq!(alone.owners, pks(1, 20));
    }

    #[test]
    fn a_short_list_is_read_whole_each_time() {
        let mut turns = TokenTurns::default();
        let (wallets, book) = (pks(1, 2), pks(100, 3));
        for _ in 0..3 {
            let turn = turns.next(&wallets, &book);
            let mut owners = turn.owners.clone();
            owners.sort();
            let mut all = [wallets.clone(), book.clone()].concat();
            all.sort();
            assert_eq!(owners, all);
            turns.advance(&turn, turn.owners.len());
        }
        assert!(TokenTurns::default().next(&[], &[]).owners.is_empty());
    }

    #[test]
    fn whoever_a_stopped_read_did_not_get_to_goes_first_next_time() {
        let (wallets, book) = (pks(1, 2), pks(100, 30));
        let mut turns = TokenTurns::default();

        // Stopped among the wallets: they start where it stopped, and the book
        // has not moved.
        let turn = turns.next(&wallets, &book);
        turns.advance(&turn, 1);
        let next = turns.next(&wallets, &book);
        assert_eq!(next.owners[0], wallets[1]);
        assert_eq!(next.owners[2], book[0]);

        // Stopped in the book: it starts where it stopped.
        let mut turns = TokenTurns::default();
        let turn = turns.next(&wallets, &book);
        turns.advance(&turn, 7);
        let next = turns.next(&wallets, &book);
        assert_eq!(next.owners[2], book[5]);

        // Asked no one: nothing moves.
        let mut turns = TokenTurns::default();
        let turn = turns.next(&wallets, &book);
        turns.advance(&turn, 0);
        assert_eq!(turns, TokenTurns::default());
    }

    #[test]
    fn a_lifetime_is_one_request_for_the_blockhash_or_the_nonce() {
        let (authority, nonce) = (pk(7), pk(3));
        let (account, hash) = nonce_account(authority);
        let rpc = FakeRpc::default().with_account(nonce, account);

        let recent = pollster::block_on(fetch_lifetime(&rpc, None)).unwrap();
        assert_eq!(recent, Lifetime::Blockhash(Hash::new_from_array([9; 32])));
        assert_eq!(rpc.asked(), 1);

        let found = pollster::block_on(fetch_lifetime(&rpc, Some(nonce))).unwrap();
        assert_eq!(
            found,
            Lifetime::Nonce(DiscoveredNonce {
                pubkey: nonce,
                authority,
                hash,
            })
        );
        assert_eq!(rpc.asked(), 2);

        let err = pollster::block_on(fetch_lifetime(&FakeRpc::default(), Some(nonce))).unwrap_err();
        assert!(err.to_string().contains("does not exist"));
    }

    #[test]
    fn balances_are_one_request_per_hundred_addresses() {
        let owners: Vec<Pubkey> = (0..150u8).map(pk).collect();
        let rpc = FakeRpc::default().with_balance(pk(3), 7);
        let balances = pollster::block_on(read_balances(&rpc, &owners)).unwrap();
        assert_eq!(rpc.asked(), 2);
        assert_eq!(balances.len(), 150);
        assert_eq!(balances[3], (pk(3), 7));
        assert_eq!(balances[4], (pk(4), 0));
    }

    #[test]
    fn a_node_that_is_down_stops_the_token_read() {
        let rpc = FakeRpc {
            offline: true,
            ..FakeRpc::default()
        };
        let read = pollster::block_on(read_tokens(&rpc, &[pk(1), pk(2)])).unwrap();
        assert_eq!(read.stopped, Some(RpcError::Offline));
        assert_eq!(rpc.asked(), 1);
    }
}
