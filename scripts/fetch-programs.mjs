// Downloads the on-chain programs that crates/flow-tests runs transactions against.
//
//   node scripts/fetch-programs.mjs            download what is missing, check every hash
//   node scripts/fetch-programs.mjs --print    download from the cluster and print the hashes found
//
// The binaries are read from devnet and are not kept in this repository: the
// Squads programs are AGPL-3.0, and the signer programs are still changing.
// Each is pinned by SHA-256 below. A program that was upgraded on devnet fails
// the check until its hash is updated here on purpose.
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const RPC = process.env.AIRLOCK_PROGRAMS_RPC ?? "https://api.devnet.solana.com";
const OUT = join(dirname(fileURLToPath(import.meta.url)), "..", "crates", "flow-tests", "programs");

/** Bytes before the program in an upgradeable loader's program data account. */
const PROGRAM_DATA_HEADER = 45;

export const PROGRAMS = [
  {
    file: "ed25519_signer.so",
    address: "EdSigVfK1DkeMrjFNDMjwfQaJPhPTtX7jW8uPv3oKEgN",
    sha256: "26a86681ca9b6d2527c0185931692c9cd540b91659e4fb4c457a628d90626f98",
  },
  {
    file: "message_executor.so",
    address: "ExecxgyHYsAXB4c5dZodV1zJZ9hqfsDCYkRDRATrpkFR",
    sha256: "27560290bd0744f3650895bb73d2faf39a4a9d2c5a4cf487877ad26d551355b2",
  },
  {
    file: "nonce.so",
    address: "Noncediea1fH12usShuQAz28UhgAeuE5Maf32LsMUQB",
    sha256: "08df5f60557edf0a717b1cdc91a1ff78dc5d8ba105e00a404e312333eab43c1a",
  },
  {
    file: "squads_multisig.so",
    address: "SQDS4ep65T869zMMBKyuUq6aD6EgTu8psMjkvj52pCf",
    sha256: "97fb2a5c08e5df5e862d8354ac5b97d3587117f98c4457754ca12e30bef0664d",
  },
  {
    file: "squads_smart_account.so",
    address: "SMRTzfY6DfH5ik3TKiyLFfXexV8uSG3d2UksSCYdunG",
    sha256: "456e8deeddccabde0d143cf18111fbb17bf00db03074569ade1997f46a25db2c",
  },
  {
    // Our deployment of the Vector program: github.com/tiago18c/vector,
    // branch feat/single-program, at e0566fc. See docs/vector-fork.md.
    file: "vector.so",
    address: "74mXFW7BSAAwc6zQC2uYKuDcWnz17HHXMWqLnEeg9BJv",
    sha256: "3eeea62b1393fd656293044f07d6e473645df7c10bda25e4ebf66043b820dc9d",
  },
];

const ALPHABET = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";

function base58(bytes) {
  let value = 0n;
  for (const byte of bytes) value = value * 256n + BigInt(byte);
  let out = "";
  for (; value > 0n; value /= 58n) out = ALPHABET[Number(value % 58n)] + out;
  for (const byte of bytes) {
    if (byte !== 0) break;
    out = "1" + out;
  }
  return out;
}

export const sha256 = (bytes) => createHash("sha256").update(bytes).digest("hex");

async function accountData(address) {
  const response = await fetch(RPC, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({
      jsonrpc: "2.0",
      id: 1,
      method: "getAccountInfo",
      params: [address, { encoding: "base64" }],
    }),
  });
  const body = await response.json();
  const value = body.result?.value;
  if (!value) throw new Error(`${address} was not found on ${RPC}: ${JSON.stringify(body.error ?? null)}`);
  return Buffer.from(value.data[0], "base64");
}

/** A program account holds a 4-byte tag and the address of its program data. */
async function download(address) {
  const program = await accountData(address);
  const data = await accountData(base58(program.subarray(4, 36)));
  return data.subarray(PROGRAM_DATA_HEADER);
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const print = process.argv.includes("--print");
  mkdirSync(OUT, { recursive: true });
  let failed = false;
  for (const program of PROGRAMS) {
    const path = join(OUT, program.file);
    let bytes;
    if (!print && existsSync(path)) {
      bytes = readFileSync(path);
    } else {
      bytes = await download(program.address);
      if (!print) writeFileSync(path, bytes);
    }
    const found = sha256(bytes);
    if (print) {
      console.log(`${program.file} ${found}${found === program.sha256 ? "" : "  (pinned: " + program.sha256 + ")"}`);
    } else if (found !== program.sha256) {
      failed = true;
      console.error(`error: ${program.file} has hash ${found}, not the pinned ${program.sha256}.`);
      console.error(`       Delete ${path} to download it again. If ${program.address} was upgraded,`);
      console.error("       review the change and update the hash in scripts/fetch-programs.mjs.");
    } else {
      console.log(`ok ${program.file}`);
    }
  }
  if (failed) process.exit(1);
}
