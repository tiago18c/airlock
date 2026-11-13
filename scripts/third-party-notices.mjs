// Writes THIRD-PARTY-NOTICES.md: every IDL file the app ships, the program it
// describes, where it was taken from, and the licence of that source.
//
//   node scripts/third-party-notices.mjs           write the file
//   node scripts/third-party-notices.mjs --check   fail if the file is out of date,
//                                                   or a shipped IDL has no row here
//
// The shipped files and their sources come from resources/idls/manifest.toml
// (the IDL pack) and BUILTIN_IDLS in crates/wallet-core/src/idl/builtin.rs (the
// core). What is established by hand is in the two tables below: FILES, one row
// per shipped file, and LICENCES, one row per repository named anywhere. Adding
// an IDL means adding its row to FILES, and its repository to LICENCES if new.
import { readdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const read = (path) => readFileSync(join(root, path), "utf8");

export const OUTPUT = "THIRD-PARTY-NOTICES.md";

const NONE = "None stated";
const BUSL = "Business Source License 1.1";
const METAPLEX = "Metaplex NFT Open Source License 1.0";

/**
 * The licence each repository states, as read from its licence file on GitHub
 * (`gh api repos/<repo> --jq .license`, and the file itself where GitHub
 * reports none or "other"). "None stated": no licence file in the repository,
 * and none named in its README.
 */
export const LICENCES = {
  "0dotxyz/marginfi-v2": "Apache-2.0",
  "Bonasa-Tech/manifest": "GPL-3.0",
  "Kamino-Finance/kfarms": BUSL,
  "Kamino-Finance/klend": BUSL,
  "Kamino-Finance/kvault": BUSL,
  "LayerZero-Labs/LayerZero-v2":
    "LayerZero Business License 1.2 and MIT, both at the repository's root; the Solana programs' `package.json` says BUSL-1.1",
  "MeteoraAg/damm-v2": "Meteora Noncommercial Licence",
  "MeteoraAg/dynamic-bonding-curve-sdk": "MIT",
  "Mythic-Project/governance-sdk": "Apache-2.0",
  "QuarryProtocol/quarry": "AGPL-3.0",
  "Squads-Protocol/smart-account-program": "AGPL-3.0",
  "Squads-Protocol/v4": "AGPL-3.0",
  "byreal-git/byreal-clmm": "Apache-2.0",
  "firedancer-io/market-tick": NONE,
  "fractal-protocol/solana-upshift-vault-programs": BUSL,
  "gmsol-labs/gmx-solana": BUSL,
  "hylo-so/sdk": "MPL-2.0",
  "igneous-labs/inf-1.5": "Apache-2.0",
  "igneous-labs/stakedex-sdk": NONE,
  "jito-foundation/jito-programs": NONE,
  "jito-foundation/restaking": "Apache-2.0",
  "jup-ag/jupiter-lend": NONE,
  "marinade-finance/liquid-staking-program": "All rights reserved, with a limited licence, in its `LICENSE.md`",
  "me-foundation/mmm": "Apache-2.0",
  "metaDAOproject/programs": BUSL,
  "metaplex-foundation/mpl-bubblegum": METAPLEX,
  "metaplex-foundation/mpl-candy-machine": METAPLEX,
  "metaplex-foundation/mpl-core": METAPLEX,
  "metaplex-foundation/mpl-token-auth-rules": METAPLEX,
  "metaplex-foundation/mpl-token-metadata": METAPLEX,
  "onre-finance/onre-sol": "MIT",
  "orca-so/whirlpools": "Orca License (Apache-2.0 until 2025-02-26)",
  "otter-sec/otter-verify": NONE,
  "pump-fun/pump-public-docs": NONE,
  "raydium-io/raydium-amm": "Apache-2.0",
  "raydium-io/raydium-clmm": "Apache-2.0",
  "raydium-io/raydium-cp-swap": "Apache-2.0",
  "raydium-io/raydium-idl": NONE,
  "smartcontractkit/chainlink-ccip": `${BUSL} for some folders; none stated for \`chains/solana\``,
  "solana-foundation/subscriptions": "MIT",
  "solana-program/compute-budget": "Apache-2.0",
  "solana-program/ed25519-programmatic-signer": "Apache-2.0",
  "solana-program/loader-v3": "Apache-2.0",
  "solana-program/memo": "Apache-2.0",
  "solana-program/program-metadata": "Apache-2.0",
  "solana-program/stake": "Apache-2.0",
  "solana-program/stake-pool": "Apache-2.0",
  "solana-program/system": "Apache-2.0",
  "solana-program/token": "Apache-2.0",
  "solana-program/token-2022": "Apache-2.0",
  "solendprotocol/solana-program-library": "Apache-2.0",
  "streamflow-finance/js-sdk": "GPL-3.0",
  "tiago18c/squads-cs-poc": NONE,
  "tiago18c/vector": "MIT",
  "velocity-exchange/velocity-v1": "Apache-2.0",
};

const STAKE_POOL_SOURCE = {
  text: "`program/src/instruction.rs` in [solana-program/stake-pool](https://github.com/solana-program/stake-pool/blob/864ba3c1c564cc270ca62b6e6b558f57538ae092/program/src/instruction.rs) at program@v2.0.3 (`864ba3c`)",
  repo: "solana-program/stake-pool",
};
const TRANSACTIONS = { text: "transactions the program executed" };

/**
 * One row per shipped file.
 *   program  what the file describes, as people know it
 *   project  for a file read from the chain or written by hand: the program's
 *            own public repository, or null when none was found
 *   from     for a file written by hand: what it was written from, and the
 *            repository whose licence applies, if any
 *   also     a second source some of the file was taken from
 */
export const FILES = {
  // The core, compiled into the app.
  "compute-budget.json": { program: "Compute budget" },
  "ed25519-programmatic-signer.json": {
    program: "Ed25519 programmatic signer",
    project: "solana-program/ed25519-programmatic-signer",
    also: { text: "then brought to that repository's IDL at `2b2cc7f`" },
  },
  "squads-clear-signing.json": { program: "Squads clear signing" },
  "squads-multisig.json": { program: "Squads v4 multisig", project: "Squads-Protocol/v4" },
  "squads-smart-account.json": { program: "Squads smart account" },
  "stake.json": { program: "Stake" },
  "system.json": { program: "System" },
  "token-2022.json": { program: "Token-2022 (and the associated token account program)" },
  "token.json": { program: "Token (and the associated token account program)" },
  "vector.json": { program: "Vector", project: "tiago18c/vector" },

  // The IDL pack.
  "byreal-clmm.json": { program: "Byreal CLMM", project: "byreal-git/byreal-clmm" },
  "ccip-lockrelease-pool.json": {
    program: "Chainlink CCIP Lock-Release Pool",
    project: "smartcontractkit/chainlink-ccip",
    also: {
      text: "eight types added from `base_token_pool.json` in [smartcontractkit/chainlink-ccip](https://github.com/smartcontractkit/chainlink-ccip) at `9546a59`",
      repo: "smartcontractkit/chainlink-ccip",
    },
  },
  "ccip-router.json": { program: "Chainlink CCIP Router", project: "smartcontractkit/chainlink-ccip" },
  "coinflow.json": { program: "Coinflow", project: null },
  "dflow.json": { program: "DFlow", project: null },
  "gmx-solana.json": { program: "GMX Solana store", project: "gmsol-labs/gmx-solana" },
  "hadron.json": { program: "Hadron", project: null },
  "huma-permissionless.json": { program: "Huma Permissionless", project: null },
  "huma.json": { program: "Huma", project: null },
  "hylo-earn-pool.json": {
    program: "Hylo Earn Pool",
    project: null,
    also: { text: "the same document is in [hylo-so/sdk](https://github.com/hylo-so/sdk)", repo: "hylo-so/sdk" },
  },
  "hylo-router.json": { program: "Hylo Router" },
  "hylo.json": {
    program: "Hylo Exchange",
    project: null,
    also: { text: "the same document is in [hylo-so/sdk](https://github.com/hylo-so/sdk)", repo: "hylo-so/sdk" },
  },
  "jito-restaking.json": { program: "Jito Restaking" },
  "jito-tip-distribution.json": { program: "Jito Tip Distribution", project: "jito-foundation/jito-programs" },
  "jito-vault.json": { program: "Jito Vault" },
  "jupiter-lend-borrow.json": { program: "Jupiter Lend Borrow" },
  "jupiter-lend-dex.json": { program: "Jupiter Lend DEX" },
  "jupiter-lend-earn.json": { program: "Jupiter Lend Earn" },
  "jupiter-lend-liquidity.json": { program: "Jupiter Lend Liquidity" },
  "jupiter-perps.json": { program: "Jupiter Perpetuals", project: null },
  "jupiter.json": { program: "Jupiter v6", project: null },
  "kamino-farms.json": { program: "Kamino Farms", project: "Kamino-Finance/kfarms" },
  "kamino-lend.json": { program: "Kamino Lend", project: "Kamino-Finance/klend" },
  "kamino-liquidity.json": { program: "Kamino Liquidity", project: null },
  "kamino-vault.json": { program: "Kamino Vault", project: "Kamino-Finance/kvault" },
  "layerzero-endpoint.json": { program: "LayerZero Endpoint", project: "LayerZero-Labs/LayerZero-v2" },
  "loader-v3.json": { program: "Upgradeable BPF loader (loader-v3)" },
  "loopscale.json": { program: "Loopscale", project: null },
  "magic-eden-mmm.json": { program: "Magic Eden AMM", project: "me-foundation/mmm" },
  "manifest-exchange.json": { program: "Manifest" },
  "marginfi.json": { program: "marginfi", project: "0dotxyz/marginfi-v2" },
  "marinade.json": { program: "Marinade Liquid Staking", project: "marinade-finance/liquid-staking-program" },
  "market-tick.json": { program: "Market Tick", project: "firedancer-io/market-tick" },
  "memo-v1.json": { program: "Memo v1" },
  "memo-v3.json": { program: "Memo v3" },
  "memo.json": { program: "Memo v4" },
  "metadao-futarchy.json": { program: "MetaDAO Futarchy", project: "metaDAOproject/programs" },
  "meteora-damm-v1.json": { program: "Meteora DAMM v1", project: null },
  "meteora-damm-v2.json": { program: "Meteora DAMM v2", project: "MeteoraAg/damm-v2" },
  "meteora-dbc.json": { program: "Meteora Dynamic Bonding Curve" },
  "meteora-dlmm.json": { program: "Meteora DLMM", project: null },
  "meteora-vault.json": { program: "Meteora Dynamic Vault", project: null },
  "mpl-bubblegum.json": { program: "Metaplex Bubblegum" },
  "mpl-candy-guard.json": { program: "Metaplex Candy Guard", project: "metaplex-foundation/mpl-candy-machine" },
  "mpl-candy-machine.json": { program: "Metaplex Candy Machine", project: "metaplex-foundation/mpl-candy-machine" },
  "mpl-core.json": { program: "Metaplex Core" },
  "mpl-token-auth-rules.json": { program: "Metaplex Token Auth Rules" },
  "mpl-token-metadata.json": { program: "Metaplex Token Metadata" },
  "okx-dex-router.json": { program: "OKX DEX Router", project: null },
  "onre.json": { program: "OnRe", project: "onre-finance/onre-sol" },
  "orca-whirlpool.json": { program: "Orca Whirlpool", project: "orca-so/whirlpools" },
  "otter-verify.json": { program: "OtterSec verify", project: "otter-sec/otter-verify" },
  "pancakeswap-v3.json": { program: "PancakeSwap v3", project: null },
  "perena.json": { program: "Perena Vault", project: null },
  "phoenix-eternal-beta.json": { program: "Phoenix Perpetuals (beta)", project: null },
  "phoenix-eternal.json": { program: "Phoenix Perpetuals (Eternal)", project: null },
  "phoenix-ember.json": { program: "Phoenix Ember", project: null },
  "program-metadata.json": { program: "Program metadata", project: "solana-program/program-metadata" },
  "pump-amm.json": { program: "PumpSwap AMM" },
  "pump-fees.json": { program: "Pump Fees" },
  "pump.json": { program: "Pump.fun" },
  "quarry-mine.json": { program: "Quarry Mine", project: "QuarryProtocol/quarry" },
  "raydium-amm-v4.json": {
    program: "Raydium AMM v4",
    also: {
      text: "brought up to the program's source, [raydium-io/raydium-amm](https://github.com/raydium-io/raydium-amm) at `d26944b`",
      repo: "raydium-io/raydium-amm",
    },
  },
  "raydium-clmm-mainnet.json": { program: "Raydium CLMM (mainnet)", project: "raydium-io/raydium-clmm" },
  "raydium-clmm.json": { program: "Raydium CLMM (devnet)", project: "raydium-io/raydium-clmm" },
  "raydium-cp-swap-mainnet.json": { program: "Raydium CP swap (mainnet)", project: "raydium-io/raydium-cp-swap" },
  "raydium-cp-swap.json": { program: "Raydium CP swap (devnet)", project: "raydium-io/raydium-cp-swap" },
  "raydium-launchlab.json": { program: "Raydium LaunchLab", project: null },
  "raydium-router-mainnet.json": { program: "Raydium router (mainnet)", from: TRANSACTIONS },
  "raydium-router.json": { program: "Raydium router (devnet)", from: TRANSACTIONS },
  "realms-governance.json": { program: "Realms governance" },
  "riptide.json": { program: "Riptide AMM", project: null },
  "sanctum-infinity.json": { program: "Sanctum Infinity" },
  "sanctum-router.json": { program: "Sanctum Router" },
  "sanctum-spl-multi-stake-pool.json": {
    program: "Sanctum Multi-Validator Stake Pool",
    from: { ...STAKE_POOL_SOURCE, text: `${STAKE_POOL_SOURCE.text}, limited to the instructions this program runs` },
  },
  "sanctum-spl-stake-pool.json": {
    program: "Sanctum SPL Stake Pool",
    from: { ...STAKE_POOL_SOURCE, text: `${STAKE_POOL_SOURCE.text}, limited to the instructions this program runs` },
  },
  "save.json": {
    program: "Save (Solend) lending",
    from: {
      text: "token-lending's `instruction.rs` and `processor.rs` in [solendprotocol/solana-program-library](https://github.com/solendprotocol/solana-program-library/blob/d04ce00bbf4356c4fd32b3be38eb9760b696bb3e/token-lending/sdk/src/instruction.rs) at `d04ce00`",
      repo: "solendprotocol/solana-program-library",
    },
  },
  "spl-stake-pool.json": { program: "SPL Stake Pool", from: STAKE_POOL_SOURCE },
  "star-atlas-cargo.json": { program: "Star Atlas Cargo", project: null },
  "star-atlas-sage.json": { program: "Star Atlas SAGE", project: null },
  "streamflow-staking.json": { program: "Streamflow Staking", project: null },
  "streamflow.json": { program: "Streamflow" },
  "subscriptions-devnet.json": { program: "Subscriptions (devnet)", project: "solana-foundation/subscriptions" },
  "subscriptions.json": { program: "Subscriptions (mainnet)", project: "solana-foundation/subscriptions" },
  "upshift.json": {
    program: "Upshift Vault",
    project: "fractal-protocol/solana-upshift-vault-programs",
    also: {
      text: "`depositChecked` and `redeemChecked` added from the program's verified source in [fractal-protocol/solana-upshift-vault-programs](https://github.com/fractal-protocol/solana-upshift-vault-programs) at `87c0f2d`",
      repo: "fractal-protocol/solana-upshift-vault-programs",
    },
  },
  "velocity.json": { program: "Velocity (formerly Drift)", project: "velocity-exchange/velocity-v1" },
};

// ---------------------------------------------------------------------------
// Reading the two lists of shipped files.

/** `key = value` of a TOML string, array of strings or inline table of strings. */
function tomlValue(text) {
  text = text.trim();
  if (text.startsWith('"')) return JSON.parse(text);
  if (text.startsWith("[")) return [...text.matchAll(/"([^"]*)"/g)].map((m) => m[1]);
  if (text.startsWith("{")) {
    return Object.fromEntries([...text.matchAll(/(\w+)\s*=\s*"([^"]*)"/g)].map((m) => [m[1], m[2]]));
  }
  throw new Error(`cannot read the TOML value ${text}`);
}

/**
 * The [[idls]] entries of resources/idls/manifest.toml: file, programId, name,
 * networks and source. Multi-line strings (`changes`) are skipped.
 */
export function manifestEntries(toml) {
  return toml
    .split(/^\[\[idls\]\]\s*$/m)
    .slice(1)
    .map((block) => {
      const entry = {};
      const lines = block.split(/\r?\n/);
      for (let i = 0; i < lines.length; i++) {
        const match = lines[i].match(/^(\w+)\s*=\s*(.*)$/);
        if (!match) continue;
        const [, key, value] = match;
        if (value.startsWith('"""')) {
          // Skip to the line that closes it, unless it closes on this one.
          if (!value.slice(3).includes('"""')) while (++i < lines.length && !lines[i].includes('"""'));
          continue;
        }
        entry[key] = tomlValue(value);
      }
      return entry;
    });
}

const NETWORK = { Mainnet: "mainnet", Devnet: "devnet", Testnet: "testnet", Localnet: "localnet" };
const ACCOUNT = { ProgramMetadata: "programMetadata", Anchor: "anchor" };

/** The entries of BUILTIN_IDLS, in the same shape as the manifest's. */
export function builtinEntries(rust) {
  const start = rust.indexOf("pub static BUILTIN_IDLS");
  if (start < 0) throw new Error("no BUILTIN_IDLS in builtin.rs");
  const body = rust.slice(start, rust.indexOf("\n];", start));
  return body
    .split(/^\s*BuiltinIdl \{\s*$/m)
    .slice(1)
    .map((block) => {
      const file = block.match(/include_str!\("\.\.\/\.\.\/idls\/([^"]+)"\)/)?.[1];
      const name = block.match(/^\s*name: "([^"]+)"/m)?.[1];
      const repository = block.match(/source: repository\(\s*"([^"]+)"/);
      const chain = block.match(/source: on_chain\(\s*IdlNetwork::(\w+),\s*IdlOrigin::(\w+)/);
      let source;
      if (repository) source = { kind: "repository", url: repository[1] };
      else if (chain) source = { kind: "chain", network: NETWORK[chain[1]], account: ACCOUNT[chain[2]] };
      else if (/source: IdlSource::Hand/.test(block)) source = { kind: "hand" };
      if (!file || !name || !source) throw new Error(`cannot read a BUILTIN_IDLS entry:\n${block}`);
      return { file, name, source };
    });
}

// ---------------------------------------------------------------------------
// Writing the notices.

/** `owner/repo`, and the commit and path, of a GitHub URL. */
export function githubSource(url) {
  const match = url.match(/^https:\/\/github\.com\/([^/]+\/[^/]+?)(?:\/blob\/([0-9a-f]{7,40})\/(.+))?\/?$/);
  if (!match) throw new Error(`not a GitHub repository or file URL: ${url}`);
  return { repo: match[1], commit: match[2], path: match[3] };
}

const capitalise = (text) => text.charAt(0).toUpperCase() + text.slice(1);
const repoLink = (repo) => `[${repo}](https://github.com/${repo})`;
// A backslash first, so one already in the text is not read as escaping the bar after it.
const cell = (text) => text.replace(/\\/g, "\\\\").replace(/\|/g, "\\|").replace(/\r?\n/g, " ");

function takenFrom(entry, row) {
  const { source } = entry;
  let text;
  if (source.kind === "repository") {
    const { repo, commit, path } = githubSource(source.url);
    text = commit ? `\`${path}\` in [${repo}](${source.url}) at \`${commit.slice(0, 7)}\`` : `${repoLink(repo)} (no commit recorded)`;
  } else if (source.kind === "chain") {
    const account = source.account === "anchor" ? "Anchor IDL account" : "program-metadata account";
    text = `published on chain by the program's author, in its ${account} on ${source.network}`;
  } else {
    text = `written by hand from ${row.from.text}`;
  }
  if (row.also) text += `; ${row.also.text}`;
  return capitalise(text);
}

function licence(entry, row) {
  const { source } = entry;
  const parts = [];
  if (source.kind === "repository") {
    parts.push(LICENCES[githubSource(source.url).repo]);
  } else if (source.kind === "chain") {
    parts.push(row.project ? `on chain: none stated. Program's repository, ${repoLink(row.project)}: ${LICENCES[row.project]}` : `on chain: none stated. No public repository found for the program`);
  } else {
    parts.push(row.from.repo ? LICENCES[row.from.repo] : `${NONE}: no source document`);
  }
  if (row.also?.repo && row.also.repo !== row.project) parts.push(`${repoLink(row.also.repo)}: ${LICENCES[row.also.repo]}`);
  return capitalise(parts.join("; "));
}

function table(entries) {
  const lines = ["| File | Program | Taken from | Licence of the source |", "| --- | --- | --- | --- |"];
  for (const entry of entries) {
    const row = FILES[entry.file];
    lines.push(`| \`${entry.file}\` | ${cell(row.program)} | ${cell(takenFrom(entry, row))} | ${cell(licence(entry, row))} |`);
  }
  return lines.join("\n");
}

export function render(core, pack) {
  return `# Third-party notices

<!-- Generated by scripts/third-party-notices.mjs from resources/idls/manifest.toml and BUILTIN_IDLS. Edit the tables in the script, then run it. -->

Airlock is under the MIT licence ([LICENSE](LICENSE)). This file lists what ships with it that is other people's work, and is not under that licence.

## IDL files

An IDL is a program's interface description: the names and byte layouts of its instructions, accounts and types, and often their doc comments. Airlock ships the IDLs of other projects' Solana programs as data files, so it can say what a transaction does before it is signed: ${core.length} are compiled into the app from \`crates/wallet-core/idls/\`, and ${pack.length} ship beside it in the IDL pack, from \`resources/idls/\`.

Each was taken from the source named below and then modified: converted from Anchor to Codama where it was an Anchor IDL, given display nodes, and corrected where it did not match the deployed program. What was changed in each file is recorded beside its source: in its \`changes\` in [\`resources/idls/manifest.toml\`](resources/idls/manifest.toml) for the pack, and in \`BUILTIN_IDLS\` in [\`crates/wallet-core/src/idl/builtin.rs\`](crates/wallet-core/src/idl/builtin.rs) for the core.

These files are not under Airlock's MIT licence. Each remains under its source's own terms.

**Licence of the source** is the licence the source repository states in its licence file, as read when the file's row was written (2026-11-13 for the first ones). "None stated" means the repository has no licence file and its README names none. A file read from the chain was published there by the program's author, with no licence attached; for those, the column gives the licence of the program's own repository where the program has a public one.

### Compiled into the app

${table(core)}

### The IDL pack

${table(pack)}

## Other code in this repository

- \`vendor/solana-client-wasm\` is a vendored copy of the WebAssembly RPC client from regolith-labs/solana-playground, under Apache-2.0. Its origin and changes are in [\`vendor/ORIGIN.md\`](vendor/ORIGIN.md).
- The Rust crates and npm packages the apps are built from are under their own licences, which their packages declare. They are not listed here.
`;
}

// ---------------------------------------------------------------------------

/** Every problem with the tables above, given the shipped files. */
export function problems(core, pack, packDir, coreDir) {
  const found = [];
  const listedPack = new Set(pack.map((e) => e.file));
  const listedCore = new Set(core.map((e) => e.file));
  for (const file of packDir) if (!listedPack.has(file)) found.push(`resources/idls/${file} is not in manifest.toml.`);
  for (const file of listedPack) if (!packDir.includes(file)) found.push(`manifest.toml lists ${file}, which is not in resources/idls.`);
  for (const file of coreDir) if (!listedCore.has(file)) found.push(`crates/wallet-core/idls/${file} is not in BUILTIN_IDLS.`);
  const shipped = [...core, ...pack];
  const used = new Set();
  for (const entry of shipped) {
    const row = FILES[entry.file];
    if (!row) {
      found.push(`${entry.file} has no row in FILES in scripts/third-party-notices.mjs.`);
      continue;
    }
    const repos = [];
    if (entry.source.kind === "repository") repos.push(githubSource(entry.source.url).repo);
    if (entry.source.kind === "chain" && row.project === undefined) {
      found.push(`${entry.file} is read from the chain: give its row a project (its repository, or null).`);
    }
    if (entry.source.kind === "hand" && !row.from) found.push(`${entry.file} is written by hand: give its row a from.`);
    if (entry.source.kind !== "hand" && row.from) found.push(`${entry.file} is not written by hand, but its row has a from.`);
    if (row.project) repos.push(row.project);
    if (row.from?.repo) repos.push(row.from.repo);
    if (row.also?.repo) repos.push(row.also.repo);
    for (const repo of repos) {
      used.add(repo);
      if (!LICENCES[repo]) found.push(`${repo} (for ${entry.file}) has no row in LICENCES in scripts/third-party-notices.mjs.`);
    }
  }
  const names = new Set(shipped.map((e) => e.file));
  for (const file of Object.keys(FILES)) if (!names.has(file)) found.push(`FILES has a row for ${file}, which is not shipped.`);
  for (const repo of Object.keys(LICENCES)) if (!used.has(repo)) found.push(`LICENCES has a row for ${repo}, which no file names.`);
  return found;
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const check = process.argv.includes("--check");
  const pack = manifestEntries(read("resources/idls/manifest.toml"));
  const core = builtinEntries(read("crates/wallet-core/src/idl/builtin.rs"));
  const jsonIn = (dir) => readdirSync(join(root, dir)).filter((f) => f.endsWith(".json"));
  const found = problems(core, pack, jsonIn("resources/idls"), jsonIn("crates/wallet-core/idls"));
  if (found.length > 0) {
    for (const problem of found) console.error(`error: ${problem}`);
    process.exit(1);
  }
  const byFile = (a, b) => a.file.localeCompare(b.file);
  const text = render([...core].sort(byFile), [...pack].sort(byFile));
  if (check) {
    let committed = "";
    try {
      committed = read(OUTPUT).replace(/\r\n/g, "\n");
    } catch {
      // Missing: reported below.
    }
    if (committed !== text) {
      console.error(`error: ${OUTPUT} is out of date. Run: node scripts/third-party-notices.mjs`);
      process.exit(1);
    }
    console.log(`${OUTPUT} lists all ${core.length + pack.length} shipped IDL files.`);
  } else {
    writeFileSync(join(root, OUTPUT), text);
    console.log(`Wrote ${OUTPUT}: ${core.length + pack.length} IDL files.`);
  }
}
