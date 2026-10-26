//! The IDL catalog.

use super::*;

impl Session {
    pub fn idl_records(&self) -> Vec<IdlRecord> {
        self.idls.read().records().to_vec()
    }

    /// The network whose IDLs the catalog shows: the cluster in use.
    pub fn idl_network(&self) -> wallet_core::IdlNetwork {
        self.idls.read().network()
    }

    /// One program's outline, a program read from another one's file included.
    pub fn idl_outline(&self, program_id: &Pubkey) -> Result<wallet_core::IdlOutline, IdlError> {
        self.idls.read().outline(program_id)
    }

    pub fn idl_body(&self, program_id: &Pubkey) -> Option<Vec<u8>> {
        self.idls
            .read()
            .body(program_id)
            .map(|bytes| bytes.to_vec())
    }

    /// Import one IDL file. A rejected file leaves the catalog unchanged.
    pub fn import_idl(
        &self,
        bytes: &[u8],
        program_id: Option<Pubkey>,
    ) -> Result<IdlRecord, IdlError> {
        self.import_idl_with_origin(bytes, program_id, wallet_core::IdlOrigin::File)
    }

    pub fn import_idl_with_origin(
        &self,
        bytes: &[u8],
        program_id: Option<Pubkey>,
        origin: wallet_core::IdlOrigin,
    ) -> Result<IdlRecord, IdlError> {
        let imported = {
            let mut idls = self.idls;
            let imported = idls.write().import_with_origin(bytes, program_id, origin);
            imported
        };
        self.note_idl(imported)
    }

    /// Import every program named by `index.json`. Missing bodies are reported
    /// and skipped.
    pub fn import_idl_bundle(&self, files: &[(&str, &[u8])]) -> Result<IdlBundleReport, IdlError> {
        let report = {
            let mut idls = self.idls;
            let report = idls.write().import_bundle(files);
            report
        };
        self.note_idl(report)
    }

    /// Store a checksum comparison. The imported file bytes stay as they are.
    pub fn record_idl_check(
        &self,
        program_id: &Pubkey,
        remote: &[u8],
    ) -> Result<IdlCheck, IdlError> {
        let checked = {
            let mut idls = self.idls;
            let checked = idls.write().record_check(program_id, remote);
            checked
        };
        self.note_idl(checked)
    }

    pub fn remove_idl(&self, program_id: &Pubkey) -> Result<(), IdlError> {
        let removed = {
            let mut idls = self.idls;
            let removed = idls.write().remove(program_id);
            removed
        };
        self.note_idl(removed)
    }

    pub fn export_idl_files(&self) -> Result<Vec<(String, Vec<u8>)>, IdlError> {
        self.note_idl(self.idls.read().export_files())
    }

    pub(super) fn note_idl<T>(&self, result: Result<T, IdlError>) -> Result<T, IdlError> {
        let mut shown = self.save_error;
        match &result {
            Ok(_) => shown.set(None),
            Err(err) => shown.set(Some(format!("Could not save: {err}"))),
        }
        result
    }
}
