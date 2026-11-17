# Programmatic signer fork

How Airlock pins the Ed25519 programmatic signer programs: our own deployment of upstream's code under our own program IDs, so programmatic signers work on mainnet. How the app signs for them is in [signing.md](signing.md).

## Pin

| What | Value |
| --- | --- |
| Upstream | [solana-program/ed25519-programmatic-signer](https://github.com/solana-program/ed25519-programmatic-signer) `main` at `684d220`, deployed on devnet only, under `EdSig…`, `Execx…` and `Nonce…` |
| Fork | [tiago18c/ed25519-programmatic-signer](https://github.com/tiago18c/ed25519-programmatic-signer), branch `mainnet-program-ids` |
| Revision | `8964545091e4018d3a6988b33525305c214b0be4` (upstream `684d220` plus one commit: the program IDs) |
| Ed25519 Signer | `Sig8PHhDifGhptyvrxGRoNNHJSvKYBfSjbxBchTay9E` (`ED25519_SIGNER_PROGRAM_ID`) |
| Message Executor | `ExeXHWU2yrgUrKQg4QcjqnniW6rWYvqBVP36cq5uwTSp` (`MESSAGE_EXECUTOR_PROGRAM_ID`) |
| Nonce | `NoncFMHL3z7njAbTPMQfscuyg2efjVADwfrhffgBv2h` (`EXECUTOR_NONCE_PROGRAM_ID`) |
| Transition signer | `8foN7sp8oHRj51Zq8t2ctrU2SjxYETjsYTnBn2AFVMBF`, the executor's PDA of `transition-signer`, bump 255 as upstream's interface requires (`EXECUTOR_TRANSITION_SIGNER`) |
| IDL | `crates/wallet-core/idls/ed25519-programmatic-signer.json`, the fork's `idl.json` byte for byte (SHA-256 `9f9a17d7…`), read on mainnet and devnet. Its source in `BUILTIN_IDLS` is the repository at the revision above: a file taken unchanged has no on-chain source to compare |
| Binary pins | SHA-256 of each program in `scripts/fetch-programs.mjs`, read from mainnet: signer `bd541ad3…`, executor `762b4465…`, nonce `bdb3d70d…`, the `solana-verify build` of the revision above |

Addresses derive from the program IDs, so a programmatic signer (`["programmatic-signer", authority]` under the signer program) has a different address here than under upstream's deployment. A programmatic signer made under upstream's devnet programs is not one under ours, and the app no longer recognises it.

## The patch on the fork

One commit on top of upstream `684d220`: the three program IDs and the transition signer's address, everywhere upstream writes them (`declare_id!` in each interface, `program-id` in each `Cargo.toml`, the codama attributes and the IDLs, the generated JS client, the README and the CLI's golden output). Nothing else changes: PDAs, owners and CPI targets all read the interface crates' IDs. The transition signer's bump is a constant in upstream's interface (255), and our executor ID was checked to give that bump (`constants_match_derivation`).

## Deployment (mainnet)

The keypairs of the three program IDs are kept outside both repositories. The fork's `MAINNET.md` lists the build, deploy, IDL and verification steps.

| Claim | Result |
| --- | --- |
| Build | `solana-verify build --base-image solanafoundation/solana-verifiable-build:3.1.8` for each library, so the bytes can be reproduced from the fork's commit. Executable hashes (`get-executable-hash`): signer `549e443e…`, executor `b064079f…`, nonce `70764f1e…` |
| Deployed | Not yet |
| Upgrade authority | Not yet chosen |
| IDL | To be written to the signer program's program-metadata account with `@solana-program/program-metadata`, from the fork's `idl.json` with LF line endings |

Once deployed: run `node scripts/fetch-programs.mjs --print` and check the three hashes match the pins (they are the build's; a deploy with `--max-len` above the program's size leaves zeros after it, which changes the hash read back), and fill in the signer's entry in `resources/program-builds.toml` (`authority`, `programHash` as `solana-verify get-program-hash` prints it).

## Rebase

When upstream changes:

1. Rebase `mainnet-program-ids` on upstream `main`. Conflicts are only where upstream wrote an ID.
2. Regenerate what derives from the IDs: the IDLs and JS client (`make generate-clients`), and the CLI golden (`clients/cli/tests/goldens/transaction-approval.txt`, from the failing `approval_display_matches_golden`).
3. Build with `solana-verify build`, upgrade each changed program with its upgrade authority, and write the IDL again.
4. Copy `idl.json` to `crates/wallet-core/idls/ed25519-programmatic-signer.json` (LF), update the commit in `BUILTIN_IDLS`, the hashes in `scripts/fetch-programs.mjs` and the entry in `resources/program-builds.toml`.
