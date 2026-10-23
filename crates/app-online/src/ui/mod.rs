//! Online-only UI: Portfolio, Stake, the Proposals and Send modals hosted for
//! the other panes, and the top bar network chip.

mod chips;
mod portfolio;
mod proposals;
mod send;
mod stake;

pub use chips::NetworkChip;

pub use portfolio::PortfolioPanel;
pub use proposals::ProposalsHost;
pub use send::SendHost;
pub use stake::StakePanel;
