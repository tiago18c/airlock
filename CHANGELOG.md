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
- The IDLs of third-party programs (Raydium, Orca) ship as one compressed file beside the app instead of inside it, about 96 KB in place of 1.8 MB of JSON. Desktop reads it while it starts; the web app fetches it once and caches it. The app checks it against the checksum its build pinned and refuses a file that does not match, and Review and Programs say so. The IDLs of the programs the wallet itself uses stay compiled in.
- IDLs are kept per network: mainnet, devnet, testnet, and localnet for a node whose genesis is none of those. The Programs screen, review and Build read the IDLs of the cluster in use. A custom RPC reads those of the public cluster its genesis matched. An IDL imported or read from the chain on one network does not show on another, and the export carries every network. Files stored before this update are kept for the cluster in use when the app first opens.
- Raydium's CLMM and CP swap programs come with the app on mainnet too. Their files are the devnet ones with mainnet's program, admin and fee receiver addresses, which is all the two networks' published IDLs differ in. The Raydium router, Vector and the Ed25519 signer stay devnet only.
- Checking an embedded IDL online compares the chain's bytes with the document the file was made from, as well as with the file. A converted or annotated file whose source the chain still publishes shows "matches its source" instead of "differs". The Programs screen says where each embedded file came from and what was changed in it.
- A converted browser or paste request discards the site's compute unit and loaded-accounts limits when online. The rewrite is simulated at the highest of each (1,400,000 units and 64 MiB), and both limits are what it used plus 10%. The site's price per compute unit is still kept. Offline, or when the simulation fails, limits the site set are kept. Build and rebuilds that fill blank limits from a simulation use the same 10% headroom.
- A transaction rebuilt for a programmatic signer, Vector wallet or Squads vault is probed at the highest compute limit, then both the compute and loaded-accounts limits are set from that simulation plus 10%. Sign does the same again once an authorization is signed, so the inner instructions are measured and the limits on screen (and on the wire that is signed) match what the node used. Once a signature over the transaction itself is in place, those limits are left alone so fitting cannot void it.
- A Squads rebuild no longer drops execute because a static compute estimate (the site's limit plus wrap overhead) was above 1,400,000. The wrap caps at the network maximum and the real use is taken from simulation, so a stacked estimate like 1,500,000 no longer stops a proposal that would fit.
- Signing an authorization (a programmatic signer's or a Vector account's) is a press of its own. Sign then simulates the authorized transaction and fits its limits before the next press signs the transaction, so an authorization made on this device no longer lands out of compute. The browser's quick Sign is not offered when this device signs an authorization: Review does it. A simulation that fails, as it does before an authorization is signed, keeps compute at the highest limit instead of the little the failed run used.
- Share opens on Everything, now its first tab. Its Signatures tab has tabs of its own: Local, which it opens on, puts the signatures made by keys on this device in one transfer; All puts every signature in one transfer; Individual shows a code for each.

### Fixed
- The embedded IDL files have LF line endings on every checkout, so their checksums no longer depend on the OS the app was built on. A Windows build and a Linux or web build pinned different checksums for the same embedded IDL.
- A browser or paste request whose programs create accounts from new keypairs in their own calls, like Orca's pool creation creating its two vaults, no longer converts into signer slots nobody can sign. When the transaction's own instructions do not explain an unsigned outside signer, it is simulated and the accounts its programs create count as one-time keys, left for the site to sign or replaced by keys made here. Without a node, or when the simulation fails, only the transaction's own instructions count, as before.

### Security
- Secp256k1 and EIP-191 Vector keys sign with k256 0.14, off crypto-bigint 0.5 and its timing advisory (RUSTSEC-2026-0387). crypto-bigint 0.7 is at 0.7.9, off the yanked 0.7.7.

### Added
- Initial release.
