//! Programs a Codama IDL carries in `additionalPrograms`, such as the
//! associated-token program inside the Token IDL. Each one gets a record of
//! its own, read from that file, unless a file of its own is stored.

use super::*;
use std::cmp::Reverse;

/// Lower is better: fuller first, then the host's place in [`BUILTIN_IDLS`],
/// then its address so the choice never depends on load order.
type Rank = (Reverse<usize>, usize, Pubkey);

/// One entry of a stored file's `additionalPrograms`.
#[derive(Clone, Debug)]
pub(super) struct Companion {
    program_id: Pubkey,
    name: String,
    /// How much of the program the entry describes. See [`completeness`].
    completeness: usize,
}

impl<S: VaultStore, B: IdlBodies> IdlStore<S, B> {
    /// Work out the companion records again from the files now stored. A
    /// program with a file of its own keeps that file. When several files
    /// carry the same program, the most complete entry wins, and on a tie the
    /// file of the program listed first in [`BUILTIN_IDLS`].
    pub(super) fn refresh_companions(&mut self) {
        let before: Vec<Pubkey> = self.companion_ids().collect();
        self.records.retain(|record| record.host.is_none());
        let mut chosen: BTreeMap<Pubkey, (Rank, IdlRecord)> = BTreeMap::new();
        for host in &self.records {
            let Some(body) = self.loaded.get(&host.program_id) else {
                continue;
            };
            let companions = self
                .peeked
                .entry(host.program_id)
                .and_modify(|(sha, companions)| {
                    if *sha != host.sha256 {
                        *sha = host.sha256;
                        *companions = companions_in(body);
                    }
                })
                .or_insert_with(|| (host.sha256, companions_in(body)));
            for companion in &companions.1 {
                if self
                    .records
                    .iter()
                    .any(|record| record.program_id == companion.program_id)
                {
                    continue;
                }
                let rank = (
                    Reverse(companion.completeness),
                    host_rank(&host.program_id),
                    host.program_id,
                );
                if chosen
                    .get(&companion.program_id)
                    .is_some_and(|(best, _)| *best <= rank)
                {
                    continue;
                }
                let record = IdlRecord {
                    program_id: companion.program_id,
                    name: companion.name.clone(),
                    sha256: host.sha256,
                    spec: IdlSpec::Codama,
                    origin: host.origin.clone(),
                    check: host.check.clone(),
                    host: Some(host.program_id),
                };
                chosen.insert(companion.program_id, (rank, record));
            }
        }
        let hosts: Vec<Pubkey> = self.records.iter().map(|r| r.program_id).collect();
        self.peeked
            .retain(|program_id, _| hosts.contains(program_id));
        self.records
            .extend(chosen.into_values().map(|(_, record)| record));
        self.records.sort_by_key(|a| a.program_id);
        for program_id in before
            .into_iter()
            .chain(self.companion_ids().collect::<Vec<_>>())
        {
            self.forget_client(&program_id);
        }
    }

    fn companion_ids(&self) -> impl Iterator<Item = Pubkey> + '_ {
        self.records
            .iter()
            .filter(|record| record.host.is_some())
            .map(|record| record.program_id)
    }

    /// The program whose file `program_id` is read from, when that is another one.
    pub(super) fn host_of(&self, program_id: &Pubkey) -> Option<Pubkey> {
        self.records
            .iter()
            .find(|record| record.program_id == *program_id)
            .and_then(|record| record.host)
    }

    /// A client for a companion: its host's tree, turned so that the companion
    /// is the program and the host one of the additional ones. Links from one
    /// to another still resolve.
    pub(super) fn companion_client(
        &self,
        program_id: &Pubkey,
        host: &Pubkey,
    ) -> Option<ProgramClient> {
        let host_client = self.program_client(host)?;
        let mut root = host_client.root().clone();
        let wanted = program_id.to_string();
        let index = root
            .additional_programs
            .iter()
            .position(|program| program.public_key == wanted)?;
        let program = root.additional_programs.remove(index);
        let host_program = std::mem::replace(&mut root.program, program);
        root.additional_programs.insert(0, host_program);
        ProgramClient::from_root(root).ok()
    }
}

/// The `additionalPrograms` of a Codama file. Anything else yields none.
fn companions_in(body: &[u8]) -> Vec<Companion> {
    #[derive(Deserialize)]
    struct Root {
        kind: Option<String>,
        #[serde(default, rename = "additionalPrograms")]
        additional_programs: Vec<serde_json::Value>,
    }
    let Ok(root) = serde_json::from_slice::<Root>(body) else {
        return Vec::new();
    };
    if root.kind.as_deref() != Some("rootNode") {
        return Vec::new();
    }
    root.additional_programs
        .iter()
        .filter_map(|program| {
            let program_id = Pubkey::from_str(program.get("publicKey")?.as_str()?).ok()?;
            Some(Companion {
                program_id,
                name: program.get("name")?.as_str()?.to_string(),
                completeness: completeness(program),
            })
        })
        .collect()
}

/// Codama nodes plus lines of docs: every instruction, account, default,
/// PDA, type, error and display node counts once. Empty `docs` arrays and
/// other bare keys add nothing, so two entries that say the same thing tie.
pub(super) fn completeness(value: &serde_json::Value) -> usize {
    match value {
        serde_json::Value::Object(map) => {
            let node = usize::from(map.contains_key("kind"));
            let docs = map
                .get("docs")
                .and_then(serde_json::Value::as_array)
                .map_or(0, |lines| {
                    lines
                        .iter()
                        .filter(|line| line.as_str().is_some_and(|text| !text.trim().is_empty()))
                        .count()
                });
            node + docs
                + map
                    .iter()
                    .filter(|(key, _)| key.as_str() != "docs")
                    .map(|(_, value)| completeness(value))
                    .sum::<usize>()
        }
        serde_json::Value::Array(items) => items.iter().map(completeness).sum(),
        _ => 0,
    }
}

/// Embedded files first, in the order they are listed, then the rest.
fn host_rank(program_id: &Pubkey) -> usize {
    BUILTIN_IDLS
        .iter()
        .position(|idl| idl.program_id == *program_id)
        .unwrap_or(usize::MAX)
}
