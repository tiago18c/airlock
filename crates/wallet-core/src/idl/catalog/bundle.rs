//! A folder of IDLs with an index, to move a catalog between devices.

use super::*;

impl<S: VaultStore, B: IdlBodies> IdlStore<S, B> {
    /// `index.json` plus one `<program id>.json` per imported program. Embedded
    /// IDLs are left out: the other device already has them.
    pub fn export_files(&self) -> Result<Vec<(String, Vec<u8>)>, IdlError> {
        let records = self.stored_records();
        let index = IdlIndex {
            records: records.clone(),
        };
        let mut files = Vec::with_capacity(records.len() + 1);
        let index_bytes =
            serde_json::to_vec_pretty(&index).map_err(|err| IdlError::Store(err.to_string()))?;
        files.push(("index.json".to_string(), index_bytes));
        for record in &records {
            let body = self.body(&record.program_id).ok_or_else(|| {
                IdlError::Store(format!("missing body for {}", record.program_id))
            })?;
            files.push((format!("{}.json", record.program_id), body.to_vec()));
        }
        Ok(files)
    }

    /// Import every body named by `index.json`. A bad file is skipped. The
    /// others stay stored.
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
        for record in index.records {
            let expected = format!("{}.json", record.program_id);
            let Some((_, bytes)) = files
                .iter()
                .find(|(name, _)| file_name(name).eq_ignore_ascii_case(&expected))
            else {
                report.skipped.push(format!("{expected} is missing"));
                continue;
            };
            match self.import(bytes, Some(record.program_id)) {
                Ok(saved) => report.imported.push(saved),
                Err(err) => report.skipped.push(format!("{expected}: {err}")),
            }
        }
        Ok(report)
    }
}
