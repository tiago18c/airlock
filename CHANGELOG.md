# Changelog

What changed for people who use Airlock, newest first. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and versions follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Every pull request that changes behavior adds a line under **Unreleased**.
A release moves those lines under a version heading. See `docs/releasing.md`.

## [Unreleased]

### Changed
- A converted browser or paste request discards the site's compute unit and loaded-accounts limits when online. The rewrite is simulated at the highest of each (1,400,000 units and 64 MiB), and both limits are what it used plus 10%. The site's price per compute unit is still kept. Offline, or when the simulation fails, limits the site set are kept. Build and rebuilds that fill blank limits from a simulation use the same 10% headroom.
- A transaction rebuilt for a programmatic signer, Vector wallet or Squads vault is probed at the highest compute limit, then both the compute and loaded-accounts limits are set from that simulation plus 10%. Sign does the same again once an authorization is signed, so the inner instructions are measured and the limits on screen (and on the wire that is signed) match what the node used. Once a signature over the transaction itself is in place, those limits are left alone so fitting cannot void it.
- A Squads rebuild no longer drops execute because a static compute estimate (the site's limit plus wrap overhead) was above 1,400,000. The wrap caps at the network maximum and the real use is taken from simulation, so a stacked estimate like 1,500,000 no longer stops a proposal that would fit.
- A rebuild whose simulation fails, as it does before an authorization is signed, keeps compute at the highest limit instead of the little the failed run used. A Vector or programmatic authorization signed on this device in the same press as the transaction no longer lands out of compute.

### Security
- Secp256k1 and EIP-191 Vector keys sign with k256 0.14, off crypto-bigint 0.5 and its timing advisory (RUSTSEC-2026-0387). crypto-bigint 0.7 is at 0.7.7.

### Added
- Initial release.
