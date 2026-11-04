//! What the device dialog shows: one job on a hardware wallet at a time,
//! and where it stands. The job itself runs in `actions::device`.

use solana_pubkey::Pubkey;
use wallet_core::HardwareVendor;

/// Where a job on the device stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeviceStage {
    /// Looking for the device, or waiting for it to be ready. Says what the
    /// user has to do: connect it, unlock it, open the Solana app.
    Find(String),
    /// Reading a key or checking one.
    Account,
    /// The device shows something, and waits for the user to confirm it.
    Confirm,
    Done,
    /// It stopped, and says why.
    Failed(String),
}

/// What a job asks the device.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeviceAsk {
    /// Sign a transaction, or a message, for `account`. `of` counts the
    /// device's signatures in this press, when there is more than one.
    Sign {
        account: Pubkey,
        path: String,
        of: Option<(usize, usize)>,
        /// What a Ledger shows when it signs blind.
        hash: Option<String>,
        message: bool,
    },
    /// Show `account` on the device, to compare with the screen.
    Show { account: Pubkey, path: String },
    /// Read the keys of a path scheme, for Add wallet.
    Read { scheme: String },
}

/// The job on screen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceRun {
    /// Tells this job from the next, so an old one never writes over it.
    pub id: u64,
    pub vendor: HardwareVendor,
    pub ask: DeviceAsk,
    pub stage: DeviceStage,
    /// The model, once the device answered.
    pub model: Option<String>,
}

impl DeviceRun {
    /// The dialog's title.
    pub fn title(&self) -> String {
        let vendor = self.vendor.label();
        match &self.ask {
            DeviceAsk::Sign { message: true, .. } => format!("Sign the message on your {vendor}"),
            DeviceAsk::Sign {
                of: Some((at, of)), ..
            } => {
                format!("Sign on your {vendor} ({at} of {of})")
            }
            DeviceAsk::Sign { .. } => format!("Sign on your {vendor}"),
            DeviceAsk::Show { .. } => format!("Check the address on your {vendor}"),
            DeviceAsk::Read { .. } => format!("Read the accounts of your {vendor}"),
        }
    }

    /// Whether the dialog stays up for the user to read it.
    pub fn ended(&self) -> bool {
        matches!(self.stage, DeviceStage::Done | DeviceStage::Failed(_))
    }
}
