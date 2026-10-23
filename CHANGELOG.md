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

### Added
- Initial release.
