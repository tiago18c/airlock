//! Checks every program whose IDL ships with the app against the build it was
//! reviewed for, in `resources/program-builds.toml`, and against the IDL it
//! was made from. Run daily by `.github/workflows/program-watch.yml`.
//!
//! An entry in that file is a person's statement: this embedded IDL was read
//! against the program running this executable hash. Nothing on chain ties an
//! IDL to a build. The watcher only notices when either side moves:
//!
//! - the program is redeployed (its executable hash changes) or its upgrade
//!   authority changes,
//! - the embedded IDL file changes after the review,
//! - the on-chain IDL our file was made from changes or goes away,
//! - OtterSec starts or stops verifying the running build (mainnet only).
//!
//! ```text
//! program-watch [--root DIR] [--networks mainnet,devnet] [--markdown FILE]
//!               [--json FILE] [--date YYYY-MM-DD] [--no-osec]
//! ```
//!
//! Node URLs come from `MAINNET_RPC_URL`, `DEVNET_RPC_URL` and
//! `TESTNET_RPC_URL`, or the public endpoints. It exits 1 when something
//! could not be read, and 0 otherwise, findings or not.

mod builds;
mod observe;
mod osec;

use builds::Build;
use observe::{Code, Observed};
use serde::Serialize;
use solana_pubkey::Pubkey;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use wallet_core::{
    idls_from_pack, sha256_bytes, EmbeddedIdl, IdlNetwork, IdlOrigin, IdlSource, BUILTIN_IDLS,
};

const BUILDS_FILE: &str = "resources/program-builds.toml";

/// An embedded IDL and where it ships from.
struct Shipped {
    idl: EmbeddedIdl,
    /// `core` (compiled in) or `pack` (`resources/idls`).
    home: &'static str,
    sha256: [u8; 32],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "camelCase")]
enum Level {
    /// Something moved since the review. Someone should look.
    Change,
    /// Nothing is reviewed yet, or the review no longer covers the file.
    Todo,
    /// Could not be read.
    Error,
    /// Worth knowing, nothing to do.
    Info,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Finding {
    level: Level,
    program: String,
    program_id: String,
    network: &'static str,
    text: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Row {
    program: String,
    program_id: String,
    network: &'static str,
    ships_in: &'static str,
    code: String,
    authority: String,
    verified: String,
    metadata_idl: String,
    anchor_idl: String,
    reviewed: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Report {
    date: String,
    networks: Vec<&'static str>,
    /// Change or todo findings: what the scheduled job opens an issue for.
    needs_action: usize,
    errors: usize,
    findings: Vec<Finding>,
    programs: Vec<Row>,
    /// `[[builds]]` entries with what the chain holds now, to be reviewed
    /// and pasted into the builds file.
    suggestions: String,
}

struct Args {
    root: PathBuf,
    networks: Vec<IdlNetwork>,
    markdown: Option<PathBuf>,
    json: Option<PathBuf>,
    date: Option<String>,
    osec: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        root: Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."),
        networks: vec![IdlNetwork::Mainnet, IdlNetwork::Devnet],
        markdown: None,
        json: None,
        date: None,
        osec: true,
    };
    let mut given = std::env::args().skip(1);
    while let Some(flag) = given.next() {
        let mut value = || given.next().ok_or_else(|| format!("{flag} needs a value"));
        match flag.as_str() {
            "--root" => args.root = value()?.into(),
            "--networks" => {
                args.networks = value()?
                    .split(',')
                    .map(|key| match IdlNetwork::from_key(key.trim()) {
                        Some(IdlNetwork::Localnet) | None => Err(format!("unknown network {key}")),
                        Some(network) => Ok(network),
                    })
                    .collect::<Result<_, _>>()?
            }
            "--markdown" => args.markdown = Some(value()?.into()),
            "--json" => args.json = Some(value()?.into()),
            "--date" => args.date = Some(value()?),
            "--no-osec" => args.osec = false,
            other => return Err(format!("unknown argument {other}")),
        }
    }
    Ok(args)
}

fn rpc_url(network: IdlNetwork) -> String {
    let (var, default) = match network {
        IdlNetwork::Mainnet => ("MAINNET_RPC_URL", "https://api.mainnet-beta.solana.com"),
        IdlNetwork::Devnet => ("DEVNET_RPC_URL", "https://api.devnet.solana.com"),
        _ => ("TESTNET_RPC_URL", "https://api.testnet.solana.com"),
    };
    std::env::var(var)
        .ok()
        .filter(|url| !url.is_empty())
        .unwrap_or_else(|| default.to_string())
}

fn shipped(root: &Path) -> Result<Vec<Shipped>, String> {
    let mut idls: Vec<Shipped> = BUILTIN_IDLS
        .iter()
        .map(|idl| (EmbeddedIdl::from(idl), "core"))
        .chain({
            let packed = idl_pack::pack_dir(&root.join("resources/idls"))?;
            idls_from_pack(&packed.bytes, &packed.sha256)
                .map_err(|err| err.to_string())?
                .into_iter()
                .map(|idl| (idl, "pack"))
        })
        .map(|(idl, home)| Shipped {
            sha256: sha256_bytes(&idl.json),
            idl,
            home,
        })
        .collect();
    idls.sort_by(|a, b| a.idl.name.cmp(&b.idl.name));
    Ok(idls)
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let args = match parse_args() {
        Ok(args) => args,
        Err(err) => {
            eprintln!("program-watch: {err}");
            std::process::exit(2);
        }
    };
    match run(&args).await {
        Ok(report) => {
            print!("{}", markdown(&report));
            if report.errors > 0 {
                std::process::exit(1);
            }
        }
        Err(err) => {
            eprintln!("program-watch: {err}");
            std::process::exit(1);
        }
    }
}

async fn run(args: &Args) -> Result<Report, String> {
    let shipped = shipped(&args.root)?;
    let ties = builds::read(&args.root.join(BUILDS_FILE))?;
    let osec = if args.osec {
        Some(osec::Client::new()?)
    } else {
        None
    };
    let date = args.date.clone().unwrap_or_else(today);

    let mut findings = Vec::new();
    let mut programs = Vec::new();
    let mut suggestions = String::new();
    for &network in &args.networks {
        let here: Vec<&Shipped> = shipped
            .iter()
            .filter(|item| item.idl.runs_on(network))
            .collect();
        let ids: Vec<Pubkey> = here.iter().map(|item| item.idl.program_id).collect();
        let rpc = wallet_rpc::Rpc::connect(&rpc_url(network));
        let observed = match observe::observe(&rpc, osec.as_ref(), network, &ids).await {
            Ok(observed) => observed,
            Err(err) => {
                findings.push(Finding {
                    level: Level::Error,
                    program: "every program".into(),
                    program_id: String::new(),
                    network: network.key(),
                    text: err,
                });
                continue;
            }
        };
        for (item, seen) in here.into_iter().zip(&observed) {
            let tie = ties
                .iter()
                .find(|tie| tie.program_id == item.idl.program_id && tie.network == network);
            let judged = judge(item, network, seen, tie);
            let stale = judged
                .iter()
                .any(|finding| matches!(finding.level, Level::Change | Level::Todo));
            if stale {
                suggestions.push_str(&suggestion(item, network, seen, tie, &date));
            }
            findings.extend(judged);
            programs.push(row(item, network, seen, tie));
        }
    }
    // Every entry should name an IDL that still ships on that network.
    for tie in &ties {
        let known = shipped
            .iter()
            .any(|item| item.idl.program_id == tie.program_id && item.idl.runs_on(tie.network));
        if !known && args.networks.contains(&tie.network) {
            findings.push(Finding {
                level: Level::Todo,
                program: tie.program_id.to_string(),
                program_id: tie.program_id.to_string(),
                network: tie.network.key(),
                text: format!(
                    "{BUILDS_FILE} has an entry for a program no embedded IDL reads on this network. Remove it."
                ),
            });
        }
    }
    findings.sort_by_key(|finding| finding.level);
    let report = Report {
        date,
        networks: args.networks.iter().map(|network| network.key()).collect(),
        needs_action: findings
            .iter()
            .filter(|finding| matches!(finding.level, Level::Change | Level::Todo))
            .count(),
        errors: findings
            .iter()
            .filter(|finding| finding.level == Level::Error)
            .count(),
        findings,
        programs,
        suggestions,
    };
    if let Some(path) = &args.markdown {
        std::fs::write(path, markdown(&report))
            .map_err(|err| format!("{}: {err}", path.display()))?;
    }
    if let Some(path) = &args.json {
        let json = serde_json::to_string_pretty(&report).map_err(|err| err.to_string())?;
        std::fs::write(path, json).map_err(|err| format!("{}: {err}", path.display()))?;
    }
    Ok(report)
}

fn judge(
    item: &Shipped,
    network: IdlNetwork,
    seen: &Observed,
    tie: Option<&Build>,
) -> Vec<Finding> {
    let mut out = Vec::new();
    let mut say = |level, text: String| {
        out.push(Finding {
            level,
            program: item.idl.name.clone(),
            program_id: item.idl.program_id.to_string(),
            network: network.key(),
            text,
        })
    };

    match (&seen.code, tie) {
        (Err(err), _) => say(Level::Error, format!("Could not read the program: {err}")),
        (Ok(_), None) => say(
            Level::Todo,
            format!("No reviewed build. Check the IDL against the running program, then add the suggested entry to {BUILDS_FILE}."),
        ),
        (Ok(code), Some(tie)) => {
            match (code, tie.program_hash) {
                (Code::Native, None) => {}
                (Code::Program(now), Some(was)) if now.hash != was => say(
                    Level::Change,
                    format!(
                        "Redeployed: the executable hash was {}, now {}.{} Review the new build against the IDL, then update the entry.",
                        hex::encode(was),
                        hex::encode(now.hash),
                        match seen.verified() {
                            Some(build) => format!(" OtterSec verifies it as {} at {}.", build.repo, build.commit),
                            None => String::new(),
                        },
                    ),
                ),
                (Code::Program(_), Some(_)) => {}
                _ => say(Level::Change, "The program moved between a native and a deployed program.".into()),
            }
            if let Code::Program(now) = code {
                if now.authority != tie.authority {
                    say(
                        Level::Change,
                        format!(
                            "The upgrade authority was {}, now {}.",
                            authority(tie.authority.as_ref()),
                            authority(now.authority.as_ref())
                        ),
                    );
                }
            }
            if tie.idl_sha256 != item.sha256 {
                say(
                    Level::Todo,
                    format!(
                        "The embedded IDL changed after the review (reviewed {}, ships {}). Check it against the build and update idlSha256.",
                        short(&tie.idl_sha256),
                        short(&item.sha256)
                    ),
                );
            }
            match (&seen.builds, tie.verified, seen.verified()) {
                (Some(Ok(_)), Some(_), None) => say(
                    Level::Change,
                    "OtterSec no longer verifies the running build. The entry says it did.".into(),
                ),
                (Some(Ok(_)), None, Some(build)) => say(
                    Level::Info,
                    format!(
                        "OtterSec verifies the running build from {} at {}{}. The entry could say so.",
                        build.repo,
                        build.commit,
                        match build.signer {
                            Some(signer) => format!(", uploaded by {signer}"),
                            None => String::new(),
                        }
                    ),
                ),
                _ => {}
            }
        }
    }

    if let Some(Err(err)) = &seen.builds {
        say(Level::Info, format!("OtterSec could not be asked: {err}"));
    }

    // The on-chain IDL a file was made from, on the network it was read on.
    let source = item
        .idl
        .source
        .chain()
        .filter(|source| source.network == network);
    if let Some(source) = source {
        let (name, now) = match source.account {
            IdlOrigin::ProgramMetadata => ("program-metadata", &seen.metadata_idl),
            _ => ("Anchor", &seen.anchor_idl),
        };
        match now {
            Ok(Some(now)) if *now == source.sha256 => {}
            Ok(Some(now)) => say(
                Level::Change,
                format!(
                    "The {name} IDL our file was made from changed on chain: {} then, {} now. Compare them and update the file and its source sha256.",
                    short(&source.sha256),
                    short(now)
                ),
            ),
            Ok(None) => say(
                Level::Change,
                format!("The {name} IDL our file was made from is no longer on chain."),
            ),
            Err(err) => say(Level::Error, format!("The {name} IDL could not be read: {err}")),
        }
    } else {
        for (name, now) in [
            ("program-metadata", &seen.metadata_idl),
            ("Anchor", &seen.anchor_idl),
        ] {
            if let Ok(Some(now)) = now {
                say(
                    Level::Info,
                    format!(
                        "Publishes an IDL in its {name} account ({}). Our file is made from {}.",
                        short(now),
                        source_text(&item.idl.source)
                    ),
                );
            }
        }
    }
    out
}

fn row(item: &Shipped, network: IdlNetwork, seen: &Observed, tie: Option<&Build>) -> Row {
    let idl = |found: &Result<Option<[u8; 32]>, String>| match found {
        Ok(Some(hash)) => short(hash),
        Ok(None) => "none".into(),
        Err(_) => "unreadable".into(),
    };
    Row {
        program: item.idl.name.clone(),
        program_id: item.idl.program_id.to_string(),
        network: network.key(),
        ships_in: item.home,
        code: match &seen.code {
            Ok(Code::Native) => "native".into(),
            Ok(Code::Program(code)) => short(&code.hash),
            Err(_) => "unreadable".into(),
        },
        authority: match &seen.code {
            Ok(Code::Program(code)) => authority(code.authority.as_ref()),
            _ => "-".into(),
        },
        verified: match (&seen.builds, seen.verified()) {
            (None, _) => "-".into(),
            (Some(Err(_)), _) => "unreachable".into(),
            (Some(Ok(_)), Some(build)) => format!("{}@{}", build.repo, short_commit(&build.commit)),
            (Some(Ok(_)), None) => "no".into(),
        },
        metadata_idl: idl(&seen.metadata_idl),
        anchor_idl: idl(&seen.anchor_idl),
        reviewed: tie.map_or_else(|| "never".into(), |tie| tie.reviewed.clone()),
    }
}

fn suggestion(
    item: &Shipped,
    network: IdlNetwork,
    seen: &Observed,
    tie: Option<&Build>,
    date: &str,
) -> String {
    let Ok(code) = &seen.code else {
        return String::new();
    };
    let mut out = String::new();
    let _ = writeln!(out, "[[builds]]");
    let _ = writeln!(
        out,
        "# {} ({}). Check that the IDL describes this build before committing.",
        item.idl.name, item.home
    );
    let _ = writeln!(out, "programId = \"{}\"", item.idl.program_id);
    let _ = writeln!(out, "network = \"{}\"", network.key());
    match code {
        Code::Native => {
            let _ = writeln!(out, "programHash = \"{}\"", builds::NATIVE);
        }
        Code::Program(code) => {
            let _ = writeln!(out, "programHash = \"{}\"", hex::encode(code.hash));
            if let Some(key) = code.authority {
                let _ = writeln!(out, "authority = \"{key}\"");
            }
        }
    }
    let _ = writeln!(out, "idlSha256 = \"{}\"", hex::encode(item.sha256));
    let verified = seen.verified();
    let unchanged = match (code, tie.and_then(|tie| tie.program_hash)) {
        (Code::Program(code), Some(was)) => code.hash == was,
        _ => false,
    };
    match (verified, tie.and_then(|tie| tie.source.as_ref())) {
        (Some(build), _) => {
            let _ = writeln!(
                out,
                "source = {{ repo = \"{}\", commit = \"{}\" }}",
                build.repo, build.commit
            );
            if let Some(signer) = build.signer {
                let _ = writeln!(out, "verified = \"{signer}\"");
            }
        }
        (None, Some((repo, commit))) if unchanged => {
            let _ = writeln!(
                out,
                "source = {{ repo = \"{repo}\", commit = \"{commit}\" }}"
            );
        }
        _ => {
            let _ = writeln!(
                out,
                "# source = {{ repo = \"\", commit = \"\" }}  # the commit you read, if known"
            );
        }
    }
    let _ = writeln!(out, "reviewed = \"{date}\"");
    out.push('\n');
    out
}

fn markdown(report: &Report) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "# Program watch\n");
    let _ = writeln!(
        out,
        "Checked {} on {}. {} need action, {} could not be read.\n",
        report.date,
        report.networks.join(", "),
        report.needs_action,
        report.errors
    );
    for (level, title) in [
        (Level::Change, "Changed since the review"),
        (Level::Todo, "To review"),
        (Level::Error, "Could not read"),
        (Level::Info, "Notes"),
    ] {
        let found: Vec<&Finding> = report
            .findings
            .iter()
            .filter(|finding| finding.level == level)
            .collect();
        if found.is_empty() {
            continue;
        }
        let _ = writeln!(out, "## {title}\n");
        for finding in found {
            let _ = writeln!(
                out,
                "- **{}** on {} (`{}`): {}",
                finding.program, finding.network, finding.program_id, finding.text
            );
        }
        out.push('\n');
    }
    let _ = writeln!(out, "## Programs\n");
    let _ = writeln!(
        out,
        "| Program | Network | Ships in | Executable | Authority | Verified build | program-metadata IDL | Anchor IDL | Reviewed |"
    );
    let _ = writeln!(
        out,
        "| --- | --- | --- | --- | --- | --- | --- | --- | --- |"
    );
    for row in &report.programs {
        let _ = writeln!(
            out,
            "| {} | {} | {} | `{}` | {} | {} | `{}` | `{}` | {} |",
            row.program,
            row.network,
            row.ships_in,
            row.code,
            row.authority,
            row.verified,
            row.metadata_idl,
            row.anchor_idl,
            row.reviewed
        );
    }
    if !report.suggestions.is_empty() {
        let _ = writeln!(
            out,
            "\n## Suggested entries for `{BUILDS_FILE}`\n\nWhat the chain holds now. Paste an entry only after checking its IDL against that build.\n"
        );
        let _ = writeln!(out, "```toml\n{}```", report.suggestions);
    }
    out
}

fn source_text(source: &IdlSource) -> String {
    match source {
        IdlSource::Chain(source) => format!(
            "the {} IDL on {}",
            match source.account {
                IdlOrigin::ProgramMetadata => "program-metadata",
                _ => "Anchor",
            },
            source.network.key()
        ),
        IdlSource::Repository { url } => url.to_string(),
        IdlSource::Hand => "transactions, by hand".into(),
    }
}

fn authority(key: Option<&Pubkey>) -> String {
    key.map_or_else(|| "none (frozen)".into(), Pubkey::to_string)
}

fn short(hash: &[u8; 32]) -> String {
    hex::encode(&hash[..6])
}

fn short_commit(commit: &str) -> &str {
    commit.get(..10).unwrap_or(commit)
}

/// Today in UTC, as `YYYY-MM-DD`.
fn today() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    // Howard Hinnant's days-to-civil.
    let z = (secs / 86_400) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}
