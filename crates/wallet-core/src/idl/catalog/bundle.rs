//! A folder of IDLs with an index, to move a catalog between devices.

use super::*;

impl<S: VaultStore, B: IdlBodies> IdlStore<S, B> {
    /// `index.json` plus one `<network>.<program id>.json` per imported
    /// program, for every network. Embedded IDLs are left out: the other
    /// device already has them.
    pub fn export_files(&self) -> Result<Vec<(String, Vec<u8>)>, IdlError> {
        let records = self.indexed_records();
        let mut files = Vec::with_capacity(records.len() + 1);
        for entry in &records {
            let program_id = &entry.record.program_id;
            let body = match entry.network {
                Some(network) if network == self.network => {
                    self.body(program_id).map(<[u8]>::to_vec)
                }
                Some(network) => self.bodies.load(network, program_id).ok().flatten(),
                None => self.bodies.load_unscoped(program_id).ok().flatten(),
            }
            .ok_or_else(|| IdlError::Store(format!("missing body for {program_id}")))?;
            files.push((bundle_file_name(entry.network, program_id), body));
        }
        let index = IdlIndex { records };
        let index_bytes =
            serde_json::to_vec_pretty(&index).map_err(|err| IdlError::Store(err.to_string()))?;
        files.insert(0, ("index.json".to_string(), index_bytes));
        Ok(files)
    }

    /// Import every body named by `index.json`, each for the network it is
    /// listed under. A bundle from before IDLs were kept per network is
    /// imported for the network shown. A bad file is skipped. The others
    /// stay stored.
    pub fn import_bundle(&mut self, files: &[(&str, &[u8])]) -> Result<IdlBundleReport, IdlError> {
        let index_bytes = files
            .iter()
            .find(|(name, _)| file_name(name).eq_ignore_ascii_case("index.json"))
            .map(|(_, bytes)| *bytes)
            .ok_or(IdlError::BundleMissingIndex)?;
        let index: IdlIndex = serde_json::from_slice(index_bytes).map_err(|_| IdlError::NotJson)?;
        let mut report = IdlBundleReport {
            imported: Vec::new(),
            skipped: Vec::new(),
        };
        for entry in index.records {
            let program_id = entry.record.program_id;
            let expected = bundle_file_name(entry.network, &program_id);
            let Some((_, bytes)) = files
                .iter()
                .find(|(name, _)| file_name(name).eq_ignore_ascii_case(&expected))
            else {
                report.skipped.push(format!("{expected} is missing"));
                continue;
            };
            let network = entry.network.unwrap_or(self.network);
            match self.import_for(network, bytes, Some(program_id)) {
                Ok(saved) => report.imported.push(saved),
                Err(err) => report.skipped.push(format!("{expected}: {err}")),
            }
        }
        Ok(report)
    }
}

fn bundle_file_name(network: Option<IdlNetwork>, program_id: &Pubkey) -> String {
    match network {
        Some(network) => format!("{network}.{program_id}.json"),
        None => format!("{program_id}.json"),
    }
}
