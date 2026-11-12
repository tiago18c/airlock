// Downloads the on-chain programs that crates/flow-tests runs transactions against.
//
//   node scripts/fetch-programs.mjs            download what is missing, check every hash
//   node scripts/fetch-programs.mjs --print    download from the cluster and print the hashes found
//
// The binaries are read from devnet, or from mainnet for a program whose
// devnet build differs from the one its IDL describes, and are not kept in
// this repository: the Squads programs are AGPL-3.0, and the signer programs
// are still changing. Each is pinned by SHA-256 below. A program that was
// upgraded on its network fails the check until its hash is updated here on
// purpose.
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const RPC = {
  devnet: process.env.AIRLOCK_PROGRAMS_RPC ?? "https://api.devnet.solana.com",
  mainnet: process.env.AIRLOCK_PROGRAMS_MAINNET_RPC ?? "https://api.mainnet-beta.solana.com",
};
const OUT = join(dirname(fileURLToPath(import.meta.url)), "..", "crates", "flow-tests", "programs");

/** Bytes before the program in an upgradeable loader's program data account. */
const PROGRAM_DATA_HEADER = 45;

export const PROGRAMS = [
  {
    file: "ed25519_signer.so",
    address: "EdSigVfK1DkeMrjFNDMjwfQaJPhPTtX7jW8uPv3oKEgN",
    sha256: "4824c901ee53e6658024a6cc89aec92e8d29608f315f835a640e11039637c80b",
  },
  {
    file: "message_executor.so",
    address: "ExecxgyHYsAXB4c5dZodV1zJZ9hqfsDCYkRDRATrpkFR",
    sha256: "276692a010a2f04aa4d540cf8c0f0f74d64d821d72f826f9964782ec2d190344",
  },
  {
    file: "nonce.so",
    address: "Noncediea1fH12usShuQAz28UhgAeuE5Maf32LsMUQB",
    sha256: "095be5b6bcdf10003db710558a765221dd442a3a13aa7d62747d82871f60d3f7",
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
  {
    // The Squads clear signing program: github.com/tiago18c/squads-cs-poc,
    // the build of 0922251, with verifyProposalCpi. Deployed on devnet only.
    file: "squads_clear_signing.so",
    address: "7xs3LhQjKoCGfXm2qst7LUhismrG1bGxz5Jw6AHij7eu",
    sha256: "1e4f8833eca89476327d32f24a356af1b5ba8a9fa17483d82d0eb055f69f5b39",
  },
  {
    // The program-metadata program, which keeps programs' IDLs:
    // github.com/solana-program/program-metadata.
    file: "program_metadata.so",
    address: "ProgM6JCCvbYkfKqJYHePx4xxSUSqJp7rh8Lyv7nk7S",
    sha256: "879f5bf7653cda05305a013c2e05b2463abfcc0c8e9b823eef238a027b63c05e",
  },
  {
    // OtterSec's verify program, where solana-verify records the repository
    // and commit a program is built from.
    file: "otter_verify.so",
    address: "verifycLy8mB96wd9wqq3WDXQwM4oU6r42Th37Db9fC",
    sha256: "b501789b71bb383556f7c1cc7a01e51ae9bb4111bf14e4dec9f09b18e506307a",
  },
  {
    // Metaplex Token Metadata, frozen since its deploy of 2025-11-17. Devnet
    // runs another build.
    file: "mpl_token_metadata.so",
    address: "metaqbxxUerdq28cj1RbAWkYQm3ybzjb6a8bt518x1s",
    network: "mainnet",
    sha256: "31f0a627dba051a938de650464e55cc5397a4be0fd496929c1f9cf02fe5e9011",
  },
  {
    // Streamflow's vesting program. Devnet runs another build.
    file: "streamflow.so",
    address: "strmRqUCoQUgGUan5YhzUZa6KqdzwX5L6FpUxfmKg5m",
    network: "mainnet",
    sha256: "8d84b7858e7209d390f2b6f8fcda4b1feba30156b7763810be3b3763f46e506a",
  },
  {
    // Marinade's liquid staking program. Devnet runs another build.
    file: "marinade.so",
    address: "MarBmsSgKXdrN1egZf5sqe1TMai9K1rChYNDJgjq7aD",
    network: "mainnet",
    sha256: "296fd550ce78a556ba05dbaff8f8fd6c04fa538bc99f9a0a836e5e430645a3ac",
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

async function accountData(address, network) {
  const rpc = RPC[network];
  const response = await fetch(rpc, {
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
  if (!value) throw new Error(`${address} was not found on ${rpc}: ${JSON.stringify(body.error ?? null)}`);
  return Buffer.from(value.data[0], "base64");
}

/** A program account holds a 4-byte tag and the address of its program data. */
async function download(address, network = "devnet") {
  const program = await accountData(address, network);
  const data = await accountData(base58(program.subarray(4, 36)), network);
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
      bytes = await download(program.address, program.network);
      if (!print) writeFileSync(path, bytes);
    }
    const found = sha256(bytes);
    if (print) {
      console.log(`${program.file} ${found}${found === program.sha256 ? "" : "  (pinned: " + program.sha256 + ")"}`);
    } else if (found !== program.sha256) {
      failed = true;
      console.error(`error: ${program.file} has hash ${found}, not the pinned ${program.sha256}.`);
      console.error(`       Delete ${path} to download it again. If ${program.address} was upgraded on ${program.network ?? "devnet"},`);
      console.error("       review the change and update the hash in scripts/fetch-programs.mjs.");
    } else {
      console.log(`ok ${program.file}`);
    }
  }
  if (failed) process.exit(1);
}
