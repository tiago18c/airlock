# Changelog

What changed for people who use Airlock, newest first. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and versions follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Every pull request that changes behavior adds a line under **Unreleased**.
A release moves those lines under a version heading. See `docs/releasing.md`.

## [Unreleased]

### Added
- Review and Sign shows the steps the transaction on screen goes through, in a row above it: Rebuild, Sign authorization, Collect authorizations, Simulate, Sign transaction, Collect signatures, then Submit, Return to site, or Share to submit when offline. Only the steps this transaction needs appear. Those behind it are green, the one it is on is marked, and an expired blockhash marks it amber. Simulate sits where the simulation can first run, after the rebuild and every authorization. A failed simulation shows red there and does not hold signing back.

### Changed
- Sign fetches the blockhash when it is pressed. There is no Refresh blockhash button anymore. A transaction this app made or relays, that nothing has signed yet, is moved to a fresh blockhash on the press that signs it. Once a blockhash has expired, Sign moves any transaction off it instead of being disabled. Rebuild on a fresh blockhash is left only for an expired transaction this device has nothing left to sign. The Sign screen drops its "What you are signing" box: the steps at the top say what comes next, and they no longer show signing as held up by a blockhash Sign moves off.
- Summary no longer lists ComputeBudget instructions. A line above Signatures states the most the transaction costs in fees and who pays it. Inspect still lists those instructions and the limits they set. The instructions left keep their numbers in the transaction, the ones a failed simulation names.
- A converted browser or paste request discards the site's compute unit and loaded-accounts limits when online. The rewrite is simulated at the highest of each (1,400,000 units and 64 MiB), and both limits are what it used plus 10%. The site's price per compute unit is still kept. Offline, or when the simulation fails, limits the site set are kept. Build and rebuilds that fill blank limits from a simulation use the same 10% headroom.
- A transaction rebuilt for a programmatic signer, Vector wallet or Squads vault is probed at the highest compute limit, then both the compute and loaded-accounts limits are set from that simulation plus 10%. Sign does the same again once an authorization is signed, so the inner instructions are measured and the limits on screen (and on the wire that is signed) match what the node used. Once a signature over the transaction itself is in place, those limits are left alone so fitting cannot void it.
- A Squads rebuild no longer drops execute because a static compute estimate (the site's limit plus wrap overhead) was above 1,400,000. The wrap caps at the network maximum and the real use is taken from simulation, so a stacked estimate like 1,500,000 no longer stops a proposal that would fit.
- Signing an authorization (a programmatic signer's or a Vector account's) is a press of its own. Sign then simulates the authorized transaction and fits its limits before the next press signs the transaction, so an authorization made on this device no longer lands out of compute. The browser's quick Sign is not offered when this device signs an authorization: Review does it. A simulation that fails, as it does before an authorization is signed, keeps compute at the highest limit instead of the little the failed run used.

### Fixed
- A browser or paste request whose programs create accounts from new keypairs in their own calls, like Orca's pool creation creating its two vaults, no longer converts into signer slots nobody can sign. When the transaction's own instructions do not explain an unsigned outside signer, it is simulated and the accounts its programs create count as one-time keys, left for the site to sign or replaced by keys made here. Without a node, or when the simulation fails, only the transaction's own instructions count, as before.

### Security
- Secp256k1 and EIP-191 Vector keys sign with k256 0.14, off crypto-bigint 0.5 and its timing advisory (RUSTSEC-2026-0387). crypto-bigint 0.7 is at 0.7.9, off the yanked 0.7.7.

### Added
- Initial release.
