//! Transaction history: what the wallets did on chain, as nodes listed it,
//! and what this app sent. Everything in it can be read from a node again, so
//! it is a cache, saved deferred to `history.json`. A transaction read in
//! full is kept, so it is read once.

use serde::{Deserialize, Serialize};
use solana_pubkey::Pubkey;
use solana_signature::Signature;
use wallet_core::{AddressSignature, Cluster, LandedTx};
use wallet_vault::store::{check_saveable, load_json, LoadProblem, VaultStore};
use wallet_vault::VaultError;

use crate::notices::TxStatus;

/// How many transactions are kept, across every cluster. Past it the oldest
/// go, never one still waiting for its outcome.
pub const MAX_ENTRIES: usize = 5_000;

/// How often a node may say it does not have a transaction before it is no
/// longer asked.
pub const MAX_MISSES: u8 = 3;

/// One transaction a wallet took part in.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryEntry {
    #[serde(with = "text")]
    pub signature: Signature,
    pub cluster: Cluster,
    /// The wallet addresses it names, as far as known.
    #[serde(default, with = "texts")]
    pub addresses: Vec<Pubkey>,
    #[serde(default)]
    pub slot: Option<u64>,
    /// When its block was made, in unix seconds, if the node said.
    #[serde(default)]
    pub block_time: Option<i64>,
    /// When this app first saw it, in unix seconds.
    pub seen: i64,
    pub outcome: Outcome,
    /// What the screen that sent it called it, when it was sent from here.
    #[serde(default)]
    pub label: Option<String>,
    /// Its bytes as sent from here, until the node's copy is read.
    #[serde(default, with = "maybe_base64")]
    pub sent_wire: Option<Vec<u8>>,
    /// The node's copy, with what running it did.
    #[serde(default)]
    pub landed: Option<LandedTx>,
    /// How often a node said it does not have it.
    #[serde(default)]
    pub misses: u8,
}

/// What came of a transaction.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Outcome {
    /// Sent from here, and not landed yet.
    Pending,
    Succeeded,
    /// It landed and failed: its fee was paid, and nothing else it did stayed.
    Failed {
        error: String,
    },
}

impl HistoryEntry {
    /// The bytes to read it from: the node's, or what was sent from here.
    pub fn wire(&self) -> Option<&[u8]> {
        self.landed
            .as_ref()
            .map(|landed| landed.wire.as_slice())
            .or(self.sent_wire.as_deref())
    }

    /// Whether it is worth asking a node for it in full.
    pub fn wants_details(&self) -> bool {
        self.landed.is_none() && self.outcome != Outcome::Pending && self.misses < MAX_MISSES
    }

    /// Newest first: one still waiting on top, then by slot, then by when
    /// this app saw it.
    fn order_key(&self) -> (bool, u64, i64) {
        (self.slot.is_none(), self.slot.unwrap_or(0), self.seen)
    }
}

/// Newest first. Saved deferred to `history.json`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct History {
    entries: Vec<HistoryEntry>,
    /// The wallets whose first transaction on a cluster is kept: a node
    /// listed fewer than it was asked for, going back. Nothing older is asked for.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    starts: Vec<ListStart>,
}

/// Where a wallet's history on a cluster begins.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListStart {
    cluster: Cluster,
    #[serde(with = "text")]
    address: Pubkey,
}

impl History {
    pub fn entries(&self) -> &[HistoryEntry] {
        &self.entries
    }

    pub fn get(&self, signature: &Signature) -> Option<&HistoryEntry> {
        self.entries
            .iter()
            .find(|entry| entry.signature == *signature)
    }

    /// The transactions of `cluster`, newest first.
    pub fn on<'a>(&'a self, cluster: &'a Cluster) -> impl Iterator<Item = &'a HistoryEntry> + 'a {
        self.entries
            .iter()
            .filter(move |entry| entry.cluster == *cluster)
    }

    /// A transaction this app sent. It shows at once, read from `wire`, and
    /// is waiting for its outcome.
    pub fn record_sent(
        &mut self,
        signature: Signature,
        cluster: &Cluster,
        wire: &[u8],
        addresses: Vec<Pubkey>,
        label: &str,
        now: i64,
    ) {
        if let Some(entry) = self.find_mut(&signature, cluster) {
            entry.sent_wire.get_or_insert_with(|| wire.to_vec());
            entry.label.get_or_insert_with(|| label.to_string());
            add_addresses(&mut entry.addresses, &addresses);
            return;
        }
        self.entries.push(HistoryEntry {
            signature,
            cluster: cluster.clone(),
            addresses,
            slot: None,
            block_time: None,
            seen: now,
            outcome: Outcome::Pending,
            label: Some(label.to_string()),
            sent_wire: Some(wire.to_vec()),
            landed: None,
            misses: 0,
        });
        self.settle();
    }

    /// What the tracker found of a transaction sent from here. One that did
    /// not land goes: history is what happened on chain. Returns whether
    /// anything changed.
    pub fn set_status(&mut self, signature: &Signature, status: &TxStatus) -> bool {
        let Some(at) = self
            .entries
            .iter()
            .position(|entry| entry.signature == *signature)
        else {
            return false;
        };
        let entry = &mut self.entries[at];
        match status {
            TxStatus::Sent => return false,
            TxStatus::Confirmed { slot } => {
                entry.outcome = Outcome::Succeeded;
                entry.slot.get_or_insert(*slot);
            }
            TxStatus::Failed { error } => {
                entry.outcome = Outcome::Failed {
                    error: error.clone(),
                };
            }
            TxStatus::Rejected { .. }
            | TxStatus::Expired
            | TxStatus::Unknown
            | TxStatus::NotSent => {
                // A listing found it, so it landed whatever the tracker says.
                if entry.outcome != Outcome::Pending || entry.landed.is_some() {
                    return false;
                }
                self.entries.remove(at);
                return true;
            }
        }
        self.settle();
        true
    }

    /// What a node listed for `address` on `cluster`, newest first. Returns
    /// whether anything changed.
    pub fn merge_listed(
        &mut self,
        cluster: &Cluster,
        address: Pubkey,
        listed: &[AddressSignature],
        now: i64,
    ) -> bool {
        let before = self.entries.clone();
        for item in listed {
            let outcome = match &item.err {
                Some(error) => Outcome::Failed {
                    error: error.clone(),
                },
                None => Outcome::Succeeded,
            };
            if let Some(entry) = self.find_mut(&item.signature, cluster) {
                add_addresses(&mut entry.addresses, &[address]);
                entry.slot = Some(item.slot);
                entry.block_time = item.block_time.or(entry.block_time);
                entry.outcome = outcome;
                continue;
            }
            self.entries.push(HistoryEntry {
                signature: item.signature,
                cluster: cluster.clone(),
                addresses: vec![address],
                slot: Some(item.slot),
                block_time: item.block_time,
                seen: now,
                outcome,
                label: None,
                sent_wire: None,
                landed: None,
                misses: 0,
            });
        }
        self.settle();
        self.entries != before
    }

    /// One page a node listed for `address` on `cluster`, asked for `limit`:
    /// the newest, or those before the oldest kept. A short page means it
    /// reached the address's first transaction. Returns whether anything changed.
    pub fn merge_page(
        &mut self,
        cluster: &Cluster,
        address: Pubkey,
        listed: &[AddressSignature],
        limit: usize,
        now: i64,
    ) -> bool {
        let mut changed = self.merge_listed(cluster, address, listed, now);
        if listed.len() < limit && !self.reached_start(cluster, &address) {
            self.starts.push(ListStart {
                cluster: cluster.clone(),
                address,
            });
            changed = true;
        }
        changed
    }

    /// The oldest transaction of `address` on `cluster` a node listed: where
    /// a listing of older ones goes on from.
    pub fn oldest_listed(&self, cluster: &Cluster, address: &Pubkey) -> Option<Signature> {
        self.on(cluster)
            .filter(|entry| entry.slot.is_some() && entry.addresses.contains(address))
            .last()
            .map(|entry| entry.signature)
    }

    /// Whether the first transaction of `address` on `cluster` is kept, so
    /// there is nothing older to list.
    pub fn reached_start(&self, cluster: &Cluster, address: &Pubkey) -> bool {
        self.starts
            .iter()
            .any(|start| start.cluster == *cluster && start.address == *address)
    }

    /// Up to `limit` transactions of `cluster` that have not been read in
    /// full, newest first.
    pub fn wanting_details(&self, cluster: &Cluster, limit: usize) -> Vec<Signature> {
        self.on(cluster)
            .filter(|entry| entry.wants_details())
            .take(limit)
            .map(|entry| entry.signature)
            .collect()
    }

    /// What a node said of a transaction in full: what it holds, or `None`
    /// when it does not have it. Returns whether anything changed.
    pub fn set_landed(
        &mut self,
        signature: &Signature,
        cluster: &Cluster,
        landed: Option<LandedTx>,
    ) -> bool {
        let Some(entry) = self.find_mut(signature, cluster) else {
            return false;
        };
        let Some(landed) = landed else {
            entry.misses = entry.misses.saturating_add(1);
            return true;
        };
        entry.slot = Some(landed.slot);
        entry.block_time = landed.block_time.or(entry.block_time);
        entry.outcome = match &landed.err {
            Some(error) => Outcome::Failed {
                error: error.clone(),
            },
            None => Outcome::Succeeded,
        };
        entry.sent_wire = None;
        entry.landed = Some(landed);
        self.settle();
        true
    }

    fn find_mut(&mut self, signature: &Signature, cluster: &Cluster) -> Option<&mut HistoryEntry> {
        self.entries
            .iter_mut()
            .find(|entry| entry.signature == *signature && entry.cluster == *cluster)
    }

    /// Newest first, and no more than [`MAX_ENTRIES`].
    fn settle(&mut self) {
        self.entries
            .sort_by_key(|entry| std::cmp::Reverse(entry.order_key()));
        while self.entries.len() > MAX_ENTRIES {
            let Some(oldest) = self
                .entries
                .iter()
                .rposition(|entry| entry.outcome != Outcome::Pending)
            else {
                break;
            };
            self.entries.remove(oldest);
            // What was dropped can be listed again.
            self.starts.clear();
        }
    }
}

fn add_addresses(known: &mut Vec<Pubkey>, more: &[Pubkey]) {
    for address in more {
        if !known.contains(address) {
            known.push(*address);
        }
    }
}

/// On-disk wrapper, the same shape as the notices.
pub struct HistoryStore<S: VaultStore> {
    store: S,
    data: History,
    problem: Option<LoadProblem>,
}

impl<S: VaultStore> HistoryStore<S> {
    pub fn open(store: S) -> Self {
        let (data, problem) = load_json(&store, "history");
        Self {
            store,
            data,
            problem,
        }
    }

    pub fn load_problem(&self) -> Option<&LoadProblem> {
        self.problem.as_ref()
    }

    pub fn data(&self) -> &History {
        &self.data
    }

    pub fn data_mut(&mut self) -> &mut History {
        &mut self.data
    }

    pub fn save(&self) -> Result<(), VaultError> {
        check_saveable(&self.problem)?;
        let json = serde_json::to_vec(&self.data).map_err(|e| VaultError::Serde(e.to_string()))?;
        self.store.save(&json)
    }
}

/// A value as its text, such as a signature in base58.
mod text {
    use serde::{Deserialize, Deserializer, Serializer};
    use std::fmt::Display;
    use std::str::FromStr;

    pub fn serialize<T: Display, S: Serializer>(
        value: &T,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.collect_str(value)
    }

    pub fn deserialize<'de, T, D>(deserializer: D) -> Result<T, D::Error>
    where
        T: FromStr,
        T::Err: Display,
        D: Deserializer<'de>,
    {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

/// Addresses, each as its text.
mod texts {
    use serde::{Deserialize, Deserializer, Serializer};
    use solana_pubkey::Pubkey;

    pub fn serialize<S: Serializer>(keys: &[Pubkey], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(keys.iter().map(Pubkey::to_string))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Vec<Pubkey>, D::Error> {
        Vec::<String>::deserialize(deserializer)?
            .iter()
            .map(|text| text.parse().map_err(serde::de::Error::custom))
            .collect()
    }
}

/// Bytes, if any, as base64.
mod maybe_base64 {
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(
        bytes: &Option<Vec<u8>>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        match bytes {
            Some(bytes) => serializer.serialize_some(&STANDARD.encode(bytes)),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<Vec<u8>>, D::Error> {
        Option::<String>::deserialize(deserializer)?
            .map(|text| STANDARD.decode(text).map_err(serde::de::Error::custom))
            .transpose()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sig(byte: u8) -> Signature {
        Signature::from([byte; 64])
    }

    fn key(byte: u8) -> Pubkey {
        Pubkey::new_from_array([byte; 32])
    }

    fn listed(byte: u8, slot: u64, err: Option<&str>) -> AddressSignature {
        AddressSignature {
            signature: sig(byte),
            slot,
            block_time: Some(1_700_000_000 + slot as i64),
            err: err.map(str::to_string),
        }
    }

    fn landed(slot: u64, err: Option<&str>) -> LandedTx {
        LandedTx {
            slot,
            block_time: Some(1_700_000_000),
            wire: vec![9; 4],
            err: err.map(str::to_string),
            fee: 5_000,
            loaded_writable: Vec::new(),
            loaded_readonly: Vec::new(),
            pre_balances: vec![10, 0],
            post_balances: vec![4, 1],
            token_balances: Vec::new(),
            logs: vec!["Program log: hi".into()],
        }
    }

    #[test]
    fn listings_merge_by_signature_newest_first_across_addresses() {
        let mut history = History::default();
        let devnet = Cluster::Devnet;
        assert!(history.merge_listed(
            &devnet,
            key(1),
            &[
                listed(2, 20, None),
                listed(1, 10, Some("custom program error: 0x1"))
            ],
            100,
        ));
        assert!(history.merge_listed(
            &devnet,
            key(2),
            &[listed(3, 30, None), listed(2, 20, None)],
            101
        ));
        // The same listing again changes nothing.
        assert!(!history.merge_listed(
            &devnet,
            key(2),
            &[listed(3, 30, None), listed(2, 20, None)],
            102
        ));

        let order: Vec<_> = history.on(&devnet).map(|entry| entry.signature).collect();
        assert_eq!(order, vec![sig(3), sig(2), sig(1)]);
        assert_eq!(
            history.get(&sig(2)).unwrap().addresses,
            vec![key(1), key(2)]
        );
        assert_eq!(
            history.get(&sig(1)).unwrap().outcome,
            Outcome::Failed {
                error: "custom program error: 0x1".into()
            }
        );
        assert_eq!(history.on(&Cluster::MainnetBeta).count(), 0);
    }

    #[test]
    fn a_sent_transaction_waits_on_top_then_lands_or_goes() {
        let mut history = History::default();
        let devnet = Cluster::Devnet;
        history.merge_listed(&devnet, key(1), &[listed(1, 10, None)], 100);
        history.record_sent(sig(7), &devnet, &[1, 2, 3], vec![key(1)], "Sign", 200);
        history.record_sent(sig(8), &devnet, &[4, 5, 6], vec![key(1)], "Sign", 201);

        let order: Vec<_> = history.on(&devnet).map(|entry| entry.signature).collect();
        assert_eq!(order, vec![sig(8), sig(7), sig(1)]);
        let sent = history.get(&sig(7)).unwrap();
        assert_eq!(sent.outcome, Outcome::Pending);
        assert_eq!(sent.wire(), Some(&[1u8, 2, 3][..]));
        // Not asked in full while it is waiting.
        assert_eq!(history.wanting_details(&devnet, 10), vec![sig(1)]);

        assert!(history.set_status(&sig(7), &TxStatus::Confirmed { slot: 50 }));
        assert_eq!(history.get(&sig(7)).unwrap().outcome, Outcome::Succeeded);
        assert_eq!(history.get(&sig(7)).unwrap().slot, Some(50));
        assert!(history.set_status(&sig(8), &TxStatus::Expired));
        assert!(history.get(&sig(8)).is_none());
        // A final status after it landed changes nothing.
        assert!(!history.set_status(&sig(7), &TxStatus::Unknown));
        assert_eq!(history.wanting_details(&devnet, 10), vec![sig(7), sig(1)]);
    }

    #[test]
    fn a_transaction_read_in_full_is_not_asked_again_and_a_missing_one_only_so_often() {
        let mut history = History::default();
        let devnet = Cluster::Devnet;
        history.merge_listed(
            &devnet,
            key(1),
            &[listed(2, 20, None), listed(1, 10, None)],
            100,
        );

        assert!(history.set_landed(&sig(2), &devnet, Some(landed(20, Some("failed")))));
        let entry = history.get(&sig(2)).unwrap();
        assert_eq!(entry.landed.as_ref().unwrap().fee, 5_000);
        assert_eq!(
            entry.outcome,
            Outcome::Failed {
                error: "failed".into()
            }
        );
        assert_eq!(history.wanting_details(&devnet, 10), vec![sig(1)]);

        for _ in 0..MAX_MISSES {
            assert_eq!(history.wanting_details(&devnet, 10), vec![sig(1)]);
            history.set_landed(&sig(1), &devnet, None);
        }
        assert!(history.wanting_details(&devnet, 10).is_empty());
    }

    #[test]
    fn it_keeps_the_newest_and_never_drops_one_waiting() {
        let mut history = History::default();
        let devnet = Cluster::Devnet;
        history.record_sent(sig(0), &devnet, &[1], vec![key(1)], "Sign", 0);
        let listed: Vec<_> = (1..=MAX_ENTRIES as u64 + 5)
            .rev()
            .map(|slot| AddressSignature {
                signature: Signature::from({
                    let mut bytes = [0u8; 64];
                    bytes[..8].copy_from_slice(&slot.to_le_bytes());
                    bytes[63] = 1;
                    bytes
                }),
                slot,
                block_time: None,
                err: None,
            })
            .collect();
        history.merge_listed(&devnet, key(1), &listed, 1);
        assert_eq!(history.entries().len(), MAX_ENTRIES);
        assert_eq!(history.entries()[0].signature, sig(0));
        assert_eq!(history.entries()[1].slot, Some(MAX_ENTRIES as u64 + 5));
    }

    #[test]
    fn older_pages_go_on_from_the_oldest_listed_until_the_first() {
        let mut history = History::default();
        let devnet = Cluster::Devnet;
        // Asked for 2, the newest page is full: there may be more.
        history.merge_page(
            &devnet,
            key(1),
            &[listed(4, 40, None), listed(3, 30, None)],
            2,
            1,
        );
        history.record_sent(sig(9), &devnet, &[1], vec![key(1)], "Sign", 2);
        assert_eq!(history.oldest_listed(&devnet, &key(1)), Some(sig(3)));
        assert!(!history.reached_start(&devnet, &key(1)));
        // The next page is short: that was the first.
        assert!(history.merge_page(&devnet, key(1), &[listed(2, 20, None)], 2, 3));
        assert_eq!(history.oldest_listed(&devnet, &key(1)), Some(sig(2)));
        assert!(history.reached_start(&devnet, &key(1)));
        assert!(!history.reached_start(&Cluster::MainnetBeta, &key(1)));
        assert!(!history.merge_page(&devnet, key(1), &[], 2, 4));
        let back: History =
            serde_json::from_str(&serde_json::to_string(&history).unwrap()).unwrap();
        assert!(back.reached_start(&devnet, &key(1)));
    }

    #[test]
    fn it_reads_back_what_it_wrote() {
        let mut history = History::default();
        let devnet = Cluster::Devnet;
        history.merge_listed(&devnet, key(1), &[listed(1, 10, None)], 100);
        history.set_landed(&sig(1), &devnet, Some(landed(10, None)));
        history.record_sent(sig(2), &devnet, &[1, 2], vec![key(1)], "Transfer", 200);
        let json = serde_json::to_string(&history).unwrap();
        assert!(json.contains(&sig(1).to_string()));
        assert!(json.contains(&key(1).to_string()));
        let back: History = serde_json::from_str(&json).unwrap();
        assert_eq!(back, history);
    }
}
