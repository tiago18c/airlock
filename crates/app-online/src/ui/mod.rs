//! Online-only UI: Portfolio, Stake, the Proposals, Send, Manage tokens and
//! Migrate wallet modals hosted for the other panes, and the top bar network chip.

mod chips;
mod cleanup;
mod history;
mod migrate;
mod portfolio;
mod proposals;
mod send;
mod stake;

pub use chips::NetworkChip;
pub use cleanup::CleanupHost;
pub use history::HistoryPanel;
pub use migrate::MigrateHost;

pub use portfolio::PortfolioPanel;
pub use proposals::ProposalsHost;
pub use send::SendHost;
pub use stake::StakePanel;
