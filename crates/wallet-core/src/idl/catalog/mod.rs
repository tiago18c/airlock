//! Plaintext IDL catalog. One record per program, body stored as the exact
//! imported bytes. The checksum is SHA-256 of those bytes, not of a
//! re-serialized Codama tree.

use crate::error::VaultError;
use crate::idl::builtin::{builtin_idl, BUILTIN_IDLS};
use crate::ids::pubkey_string;
use base64::Engine;
use codama_dynamic_client::ProgramClient;
use codama_nodes_from_anchor::{is_anchor_idl, root_node_from_anchor};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use solana_pubkey::Pubkey;
use std::collections::BTreeMap;
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use thiserror::Error;
use wallet_vault::store::{check_saveable, load_json, LoadProblem, VaultStore};

mod bodies;
mod bundle;
mod companions;
mod onchain;
mod outline;
mod types;

#[cfg(test)]
mod tests;

pub use bodies::*;
pub use onchain::*;
pub use outline::*;
pub use types::*;

pub struct IdlStore<S: VaultStore, B: IdlBodies> {
    index_store: S,
    bodies: B,
    records: Vec<IdlRecord>,
    loaded: BTreeMap<Pubkey, Vec<u8>>,
    /// Parsed clients, kept until that program's IDL changes. Parsing a large
    /// IDL on every keystroke of a form is far too slow.
    clients: Mutex<BTreeMap<Pubkey, Arc<ProgramClient>>>,
    /// The `additionalPrograms` of each stored file, by the checksum they were
    /// read from, so a change elsewhere does not read every file again.
    peeked: BTreeMap<Pubkey, ([u8; 32], Vec<companions::Companion>)>,
    /// Whether embedded IDLs fill the slots nothing imported holds.
    builtins: bool,
    problem: Option<LoadProblem>,
}

/// The stored IDLs, as something that reads them needs them: which programs
/// have one, and a client for each. How they are stored is not its business.
pub trait IdlCatalog {
    fn records(&self) -> &[IdlRecord];
    /// A client for one stored IDL.
    fn program_client(&self, program_id: &Pubkey) -> Option<Arc<ProgramClient>>;
}

impl<S: VaultStore, B: IdlBodies> IdlCatalog for IdlStore<S, B> {
    fn records(&self) -> &[IdlRecord] {
        IdlStore::records(self)
    }

    fn program_client(&self, program_id: &Pubkey) -> Option<Arc<ProgramClient>> {
        IdlStore::program_client(self, program_id)
    }
}

impl<S: VaultStore, B: IdlBodies> IdlStore<S, B> {
    pub fn open(index_store: S, bodies: B) -> Self {
        let (index, problem): (IdlIndex, _) = load_json(&index_store, "program list");
        let listed = index.records;
        let mut store = Self {
            index_store,
            bodies,
            records: Vec::new(),
            loaded: BTreeMap::new(),
            clients: Mutex::new(BTreeMap::new()),
            peeked: BTreeMap::new(),
            builtins: false,
            problem,
        };
        let mut changed = false;
        for mut record in listed {
            match store.bodies.load(&record.program_id) {
                // An index written by another build may list an embedded IDL.
                _ if record.origin == IdlOrigin::Builtin => changed = true,
                Ok(Some(body)) => {
                    let actual = sha256(&body);
                    if record.sha256 != actual {
                        // The file changed outside the app. The last online
                        // comparison was about different bytes, so drop it.
                        record.sha256 = actual;
                        record.check = None;
                        changed = true;
                    }
                    store.loaded.insert(record.program_id, body);
                    store.records.push(record);
                }
                _ => changed = true,
            }
        }
        store.records.sort_by_key(|a| a.program_id);
        store.refresh_companions();
        if changed {
            let _ = store.save_index();
        }
        store
    }

    /// Add the IDLs that ship inside the app. An import for the same program
    /// takes its place, and removing that import brings the embedded one back.
    pub fn with_builtins(mut self) -> Self {
        self.builtins = true;
        for builtin in &BUILTIN_IDLS {
            self.restore_builtin(&builtin.program_id);
        }
        self.refresh_companions();
        self
    }

    pub fn records(&self) -> &[IdlRecord] {
        &self.records
    }

    /// Set when the stored program list could not be read at startup.
    pub fn load_problem(&self) -> Option<&LoadProblem> {
        self.problem.as_ref()
    }

    /// The stored file a program is read from. For a program carried in
    /// another file's `additionalPrograms`, that file.
    pub fn body(&self, program_id: &Pubkey) -> Option<&[u8]> {
        let file = self.host_of(program_id).unwrap_or(*program_id);
        self.loaded.get(&file).map(Vec::as_slice)
    }

    /// A client for one stored IDL. Parsed once, then shared until the IDL changes.
    pub fn program_client(&self, program_id: &Pubkey) -> Option<Arc<ProgramClient>> {
        if let Some(client) = self.clients.lock().ok()?.get(program_id) {
            return Some(client.clone());
        }
        let client = match self.host_of(program_id) {
            Some(host) => self.companion_client(program_id, &host)?,
            None => {
                open_client(self.loaded.get(program_id)?, Some(*program_id))
                    .ok()?
                    .0
            }
        };
        let client = Arc::new(client);
        self.clients
            .lock()
            .ok()?
            .insert(*program_id, client.clone());
        Some(client)
    }

    fn forget_client(&self, program_id: &Pubkey) {
        if let Ok(mut clients) = self.clients.lock() {
            clients.remove(program_id);
        }
    }

    /// Put the embedded IDL in a program's slot when nothing imported holds it.
    fn restore_builtin(&mut self, program_id: &Pubkey) {
        let Some(builtin) = builtin_idl(program_id).filter(|_| self.builtins) else {
            return;
        };
        if self
            .records
            .iter()
            .any(|row| row.program_id == *program_id && row.host.is_none())
        {
            return;
        }
        let body = builtin.json.as_bytes().to_vec();
        self.records.retain(|row| row.program_id != *program_id);
        self.records.push(IdlRecord {
            program_id: builtin.program_id,
            name: builtin.name.to_string(),
            sha256: sha256(&body),
            spec: IdlSpec::Codama,
            origin: IdlOrigin::Builtin,
            check: None,
            host: None,
        });
        self.records.sort_by_key(|a| a.program_id);
        self.loaded.insert(builtin.program_id, body);
        self.forget_client(program_id);
    }

    /// Records that live in the index. Embedded IDLs come with the app
    /// instead, and companions are read from their host's file.
    fn stored_records(&self) -> Vec<IdlRecord> {
        self.records
            .iter()
            .filter(|record| record.origin != IdlOrigin::Builtin && record.host.is_none())
            .cloned()
            .collect()
    }

    /// Record a comparison of `remote` against the stored file. The stored body
    /// is not replaced. A missing program is an error and writes nothing. A
    /// companion is checked as the file it is read from: `remote` is that
    /// file's program's IDL.
    pub fn record_check(
        &mut self,
        program_id: &Pubkey,
        remote: &[u8],
    ) -> Result<IdlCheck, IdlError> {
        let program_id = &self.host_of(program_id).unwrap_or(*program_id);
        if !self.records.iter().any(|row| row.program_id == *program_id) {
            return Err(IdlError::Invalid("program is not in the catalog".into()));
        }
        let remote_sha256 = sha256(remote);
        let previous = self.records.clone();
        let check = {
            let slot = self
                .records
                .iter_mut()
                .find(|row| row.program_id == *program_id)
                .expect("record exists");
            let check = IdlCheck {
                remote_sha256: Some(remote_sha256),
                same: remote_sha256 == slot.sha256,
            };
            slot.check = Some(check.clone());
            check
        };
        if let Err(err) = self.save_index() {
            self.records = previous;
            return Err(IdlError::Store(err.to_string()));
        }
        self.refresh_companions();
        Ok(check)
    }

    /// Remove an imported IDL. An embedded IDL cannot be removed, and comes
    /// back when the import that replaced it is removed.
    pub fn remove(&mut self, program_id: &Pubkey) -> Result<(), IdlError> {
        if let Some(host) = self.host_of(program_id) {
            let name = |id: &Pubkey| {
                self.records
                    .iter()
                    .find(|row| row.program_id == *id)
                    .map_or_else(|| id.to_string(), |row| row.name.clone())
            };
            return Err(IdlError::Companion {
                name: name(program_id),
                host: name(&host),
            });
        }
        let Some(previous_body) = self.loaded.get(program_id).cloned() else {
            return Ok(());
        };
        if let Some(record) = self
            .records
            .iter()
            .find(|row| row.program_id == *program_id)
        {
            if record.origin == IdlOrigin::Builtin {
                return Err(IdlError::Builtin(record.name.clone()));
            }
        }
        let previous_records = self.records.clone();
        self.records.retain(|row| row.program_id != *program_id);
        self.loaded.remove(program_id);
        if let Err(err) = self.bodies.remove(program_id) {
            self.records = previous_records;
            self.loaded.insert(*program_id, previous_body);
            return Err(IdlError::Store(err.to_string()));
        }
        if let Err(err) = self.save_index() {
            self.records = previous_records;
            self.loaded.insert(*program_id, previous_body.clone());
            let _ = self.bodies.save(program_id, &previous_body);
            return Err(IdlError::Store(err.to_string()));
        }
        self.forget_client(program_id);
        self.restore_builtin(program_id);
        self.refresh_companions();
        Ok(())
    }

    /// Validate `bytes` and store them under their program id. A failed
    /// validation leaves the previous record and body untouched. Re-importing
    /// the same program replaces that one slot.
    pub fn import(
        &mut self,
        bytes: &[u8],
        program_id: Option<Pubkey>,
    ) -> Result<IdlRecord, IdlError> {
        self.import_with_origin(bytes, program_id, IdlOrigin::File)
    }

    /// Same as [`Self::import`], with the origin recorded on the new slot.
    pub fn import_with_origin(
        &mut self,
        bytes: &[u8],
        program_id: Option<Pubkey>,
        origin: IdlOrigin,
    ) -> Result<IdlRecord, IdlError> {
        let inspected = inspect_idl(bytes, program_id)?;
        let previous_records = self.records.clone();
        let previous_body = self.loaded.get(&inspected.program_id).cloned();
        let record = IdlRecord {
            program_id: inspected.program_id,
            name: inspected.name,
            sha256: inspected.sha256,
            spec: inspected.spec,
            origin,
            check: None,
            host: None,
        };
        self.records
            .retain(|row| row.program_id != record.program_id);
        self.records.push(record.clone());
        self.records.sort_by_key(|a| a.program_id);
        self.loaded.insert(record.program_id, bytes.to_vec());
        self.forget_client(&record.program_id);
        if let Err(err) = self.persist_program(record.program_id, previous_body) {
            self.records = previous_records;
            self.forget_client(&record.program_id);
            self.refresh_companions();
            return Err(err);
        }
        self.refresh_companions();
        Ok(record)
    }

    fn persist_program(
        &mut self,
        program_id: Pubkey,
        previous_body: Option<Vec<u8>>,
    ) -> Result<(), IdlError> {
        let body = self
            .loaded
            .get(&program_id)
            .cloned()
            .ok_or_else(|| IdlError::Store("imported IDL body is missing".into()))?;
        if let Err(err) = self.bodies.save(&program_id, &body) {
            self.restore_body(program_id, previous_body);
            return Err(IdlError::Store(err.to_string()));
        }
        if let Err(err) = self.save_index() {
            self.restore_body(program_id, previous_body.clone());
            match &previous_body {
                Some(old) => {
                    let _ = self.bodies.save(&program_id, old);
                }
                None => {
                    let _ = self.bodies.remove(&program_id);
                }
            }
            return Err(IdlError::Store(err.to_string()));
        }
        Ok(())
    }

    fn restore_body(&mut self, program_id: Pubkey, previous_body: Option<Vec<u8>>) {
        match previous_body {
            Some(body) => {
                self.loaded.insert(program_id, body);
            }
            None => {
                self.loaded.remove(&program_id);
            }
        }
    }

    fn save_index(&self) -> Result<(), VaultError> {
        check_saveable(&self.problem)?;
        let index = IdlIndex {
            records: self.stored_records(),
        };
        let json =
            serde_json::to_vec_pretty(&index).map_err(|err| VaultError::Serde(err.to_string()))?;
        self.index_store.save(&json)
    }
}

struct Inspected {
    program_id: Pubkey,
    name: String,
    sha256: [u8; 32],
    spec: IdlSpec,
}

fn inspect_idl(bytes: &[u8], program_id: Option<Pubkey>) -> Result<Inspected, IdlError> {
    let (client, spec, digest) = open_client(bytes, program_id)?;
    Ok(Inspected {
        program_id: client.program_address(),
        name: client.root().program.name.to_string(),
        sha256: digest,
        spec,
    })
}

fn open_client(
    bytes: &[u8],
    program_id: Option<Pubkey>,
) -> Result<(ProgramClient, IdlSpec, [u8; 32]), IdlError> {
    if bytes.iter().all(u8::is_ascii_whitespace) {
        return Err(IdlError::Empty);
    }
    let text = std::str::from_utf8(bytes).map_err(|_| IdlError::NotJson)?;
    let value: serde_json::Value = serde_json::from_str(text).map_err(|_| IdlError::NotJson)?;
    let digest = sha256(bytes);
    if value.get("kind").and_then(serde_json::Value::as_str) == Some("rootNode") {
        let client =
            ProgramClient::from_json(text).map_err(|err| IdlError::Invalid(err.to_string()))?;
        let declared = client.program_address();
        if let Some(given) = program_id {
            if given != declared {
                return Err(IdlError::ProgramIdMismatch {
                    file: declared,
                    given,
                });
            }
        }
        return Ok((client, IdlSpec::Codama, digest));
    }
    if !is_anchor_idl(&value) {
        return Err(IdlError::Unrecognized);
    }
    let root = root_node_from_anchor(text).map_err(|err| IdlError::Invalid(err.to_string()))?;
    let declared = Pubkey::from_str(&root.program.public_key).ok();
    let resolved = match (declared, program_id) {
        (Some(file), Some(given)) if file != given => {
            return Err(IdlError::ProgramIdMismatch { file, given });
        }
        (Some(file), _) => file,
        (None, Some(given)) => given,
        (None, None) => return Err(IdlError::ProgramIdRequired),
    };
    let client = ProgramClient::from_root_with_program_id(root, resolved)
        .map_err(|err| IdlError::Invalid(err.to_string()))?;
    Ok((client, IdlSpec::Anchor, digest))
}

fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}
