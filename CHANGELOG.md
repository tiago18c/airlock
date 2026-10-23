# Changelog

What changed for people who use Airlock, newest first. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and versions follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Every pull request that changes behavior adds a line under **Unreleased**.
A release moves those lines under a version heading. See `docs/releasing.md`.

## [Unreleased]

### Changed
- A converted browser or paste request discards the site's compute unit and loaded-accounts limits when online. The rewrite is simulated at the highest of each (1,400,000 units and 64 MiB), and both limits are what it used plus 10%. The site's price per compute unit is still kept. Offline, or when the simulation fails, limits the site set are kept. Build and rebuilds that fill blank limits from a simulation use the same 10% headroom.

### Added
- Initial release.
