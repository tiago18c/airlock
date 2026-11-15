// What the intents end-to-end drivers share once the page is done: the
// Squads v4 addresses another member acts on, every transaction a resolution
// made or ran read back from the node, and the checks of each wallet kind.
// scripts/drive-intents-e2e.mjs (desktop) and scripts/drive-intents-android.mjs
// (a phone) use these.
import { createHash } from "node:crypto";
import { b58decode, b58encode, findProgramAddress, legacyTx, rpcCall, sleep } from "./drive-lib.mjs";
import { selectRuns } from "../examples/intents/drive-page.mjs";

export const KINDS = ["hot", "multisig-alone", "multisig-waits", "offline", "programmatic"];

/** Squads v4 on the node at `rpc`: the addresses of a multisig's proposals,
 * and a vote sent from another wallet. */
export const squads = (rpc) => {
  /** An instruction of Anchor's `name`: its discriminator. */
  const anchor = (name) => createHash("sha256").update(`global:${name}`).digest().subarray(0, 8);
  const SQUADS = b58decode("SQDS4ep65T869zMMBKyuUq6aD6EgTu8psMjkvj52pCf");
  const u64 = (n) => {
    const b = Buffer.alloc(8);
    b.writeBigUInt64LE(BigInt(n));
    return b;
  };
  const proposalOf = (multisig, index) => findProgramAddress([Buffer.from("multisig"), b58decode(multisig), Buffer.from("transaction"), u64(index), Buffer.from("proposal")], SQUADS)[0];
  const transactionOf = (multisig, index) => findProgramAddress([Buffer.from("multisig"), b58decode(multisig), Buffer.from("transaction"), u64(index)], SQUADS)[0];
  /** Ephemeral signer `signer` of transaction `index`: the address the program
   * signs for when it runs that transaction. */
  const ephemeralOf = (multisig, index, signer) =>
    b58encode(findProgramAddress([Buffer.from("multisig"), transactionOf(multisig, index), Buffer.from("ephemeral_signer"), Buffer.from([signer])], SQUADS)[0]);
  /** The multisig's newest transaction number, read from its account. */
  const newestIndex = async (multisig) => {
    const info = await rpcCall(rpc, "getAccountInfo", [multisig, { encoding: "base64" }]);
    return Number(Buffer.from(info.value.data[0], "base64").readBigUInt64LE(8 + 32 + 32 + 2 + 4));
  };
  /** A member votes on proposal `index` from another wallet, as from Squads'
   * own app: `proposal_reject`, `proposal_approve` or `proposal_cancel`,
   * signed by `key`, sent straight to the node. */
  const voteAs = async (key, vote, multisig, index) => {
    const { value } = await rpcCall(rpc, "getLatestBlockhash", [{ commitment: "confirmed" }]);
    const member = Buffer.from(key.publicKey);
    const wire = legacyTx({
      // ProposalVote: the multisig, the member (signs, pays), the proposal.
      keys: [member, proposalOf(multisig, index), b58decode(multisig), SQUADS],
      signers: 1, readonly: 2, blockhash: b58decode(value.blockhash),
      instructions: [[3, [2, 0, 1], Buffer.concat([anchor(vote), Buffer.from([0])])]],
    });
    const message = wire.subarray(65);
    const signed = Buffer.concat([wire.subarray(0, 1), key.sign(message), message]);
    const signature = await rpcCall(rpc, "sendTransaction", [signed.toString("base64"), { encoding: "base64" }]);
    for (let waited = 0; waited < 20_000; waited += 500) {
      const status = (await rpcCall(rpc, "getSignatureStatuses", [[signature]])).value[0];
      if (status?.confirmationStatus === "confirmed" || status?.confirmationStatus === "finalized") {
        if (status.err) throw new Error(`${vote} failed: ${JSON.stringify(status.err)}`);
        return signature;
      }
      await sleep(500);
    }
    throw new Error(`${vote} never confirmed`);
  };
  return { SQUADS, proposalOf, transactionOf, ephemeralOf, newestIndex, voteAs };
};

/** What the node at `rpc` shows of transaction `signature`: whether its
 * status is known and succeeded, and whether `getTransaction` returns it in
 * any encoding, with its logs. */
export const readFromNode = async (rpc, signature) => {
  const read = { signature, status: null, err: null, fetched: null, encodings: {}, logs: [] };
  for (let attempt = 0; attempt < 20 && !read.status; attempt += 1) {
    const status = (await rpcCall(rpc, "getSignatureStatuses", [[signature], { searchTransactionHistory: true }])).value[0];
    if (status) {
      read.status = status.confirmationStatus ?? "processed";
      read.err = status.err ?? null;
    } else await sleep(500);
  }
  // Each encoding, asked for version 0 at most and then for version 1.
  for (const encoding of ["json", "base64", "jsonParsed"]) {
    for (const max of [0, 1]) {
      const key = `${encoding}, max ${max}`;
      try {
        const tx = await rpcCall(rpc, "getTransaction", [signature, { encoding, commitment: "confirmed", maxSupportedTransactionVersion: max }]);
        read.encodings[key] = tx ? "returned" : "null";
        if (tx && !read.fetched) {
          read.fetched = key;
          read.slot = tx.slot;
          read.version = tx.version ?? "legacy";
          read.logs = tx.meta?.logMessages ?? [];
        }
      } catch (err) {
        read.encodings[key] = err.message.replace(/^getTransaction: /, "");
      }
    }
  }
  return read;
};

/** Every resolution seen (`entries`: `{ run, phase, resolution, states }`):
 * each transaction it made and each it says ran, read back from the node.
 * Checks, through `check`, that what a `done` resolution made and ran is on
 * the node and succeeded. */
export const readChain = async ({ rpc, entries, say, check }) => {
  const chain = [];
  for (const { run, phase: phaseName, resolution, states } of entries) {
    const entry = { phase: phaseName, run, id: resolution.id, states, state: resolution.state, timing: resolution.timing, delivery: resolution.delivery, replaced: resolution.replaced ?? {}, error: resolution.error ?? null, transactions: [], ran: [] };
    chain.push(entry);
    for (const made of resolution.transactions ?? []) {
      if (!made.signature) continue;
      entry.transactions.push({ role: made.role, state: made.state, ...(await readFromNode(rpc, made.signature)) });
    }
    for (const ran of resolution.ran ?? []) {
      const read = await readFromNode(rpc, ran.signature);
      const memo = resolution.message?.memo;
      read.memoInLogs = memo === undefined ? undefined : read.logs.some((line) => line.includes(JSON.stringify(memo).slice(1, -1).slice(0, 60)));
      entry.ran.push({ transaction: ran.transaction, ...read });
      say(`node: ${phaseName} ${run} ${resolution.id} ran ${ran.transaction} in ${ran.signature.slice(0, 12)}… ${read.status ? `${read.status}, ${read.err === null ? "succeeded" : `FAILED ${JSON.stringify(read.err)}`}` : "NOT FOUND"}${read.fetched ? `, getTransaction (${read.fetched}) v${read.version}` : ", getTransaction refused"}${memo === undefined ? "" : read.memoInLogs ? ", memo in its logs" : ""}`);
    }
  }
  for (const entry of chain) {
    if (entry.state !== "done") continue;
    for (const ran of entry.ran) check(ran.status && ran.err === null, `${entry.run} ${entry.id}: what ran (${ran.signature.slice(0, 12)}…) is on the node and succeeded`);
    for (const made of entry.transactions) check(made.status && (made.err === null || made.role === "vote"), `${entry.run} ${entry.id}: its ${made.role} transaction is on the node and succeeded`);
  }
  return chain;
};

const TOKEN_PROGRAMS = ["TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA", "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb"];

/** What kind `kind` expects of what happened: the profile the app reported
 * first, and for each kind what its resolutions went through on the node.
 * `profiles` are `{ run, profile }` in order; `setup` is what the kind set
 * up (`profile`, `multisig`); `only` is --only. */
export const checkKind = async ({ kind, rpc, chain, profiles, setup, only, check, say }) => {
  const { ephemeralOf, newestIndex } = squads(rpc);
  const firstProfile = profiles[0]?.profile;
  if (setup.profile) {
    check(firstProfile?.timing === setup.profile.timing && firstProfile?.execution === setup.profile.execution,
      `the profile is ${setup.profile.timing}, ${setup.profile.execution} (the app said ${firstProfile ? `${firstProfile.timing}, ${firstProfile.execution}` : "nothing"})`);
  }
  const ofRun = (phaseName, run) => chain.filter((entry) => entry.phase === phaseName && entry.run === run);
  /** The new mint of a resolution: the address `replaced` gave it, which has to
   * be ephemeral signer 0 of one of the multisig's proposals, and whether the
   * node holds an account there owned by a token program. */
  const carriedMint = async (entry) => {
    const newest = await newestIndex(setup.multisig);
    for (const [sent, used] of Object.entries(entry.replaced)) {
      for (let index = 1; index <= newest; index += 1) {
        if (ephemeralOf(setup.multisig, index, 0) !== used) continue;
        const info = await rpcCall(rpc, "getAccountInfo", [used, { encoding: "base64" }]);
        return { sent, used, proposal: index, owner: info.value?.owner ?? null };
      }
    }
    return null;
  };
  const checkCarriedMint = async (entry, run) => {
    const mint = await carriedMint(entry);
    check(mint, `${run}: replaced gives the new mint an ephemeral signer of the multisig's proposal (${JSON.stringify(entry.replaced)})`);
    if (!mint) return;
    say(`node: ${run} mint ${mint.sent} became ${mint.used}, ephemeral signer 0 of proposal ${mint.proposal}, owned by ${mint.owner}`);
    check(TOKEN_PROGRAMS.includes(mint.owner), `${run}: the node holds the mint at ${mint.used}, owned by a token program (${mint.owner ?? "no account"})`);
  };
  if (kind === "multisig-alone") {
    for (const run of ["mint:classic", "mint:token-2022"]) {
      for (const entry of ofRun("alone", run)) {
        check(entry.state === "done" && !entry.states.includes("awaiting-signatures"), `${run}: done without waiting (${entry.states.join(" → ")})`);
        await checkCarriedMint(entry, run);
      }
    }
    for (const run of ["memo", "mint:classic", "message:memo"]) {
      for (const entry of ofRun("alone", run)) {
        check(entry.state === "done" && !entry.states.includes("awaiting-signatures"), `${run}: done without waiting (${entry.states.join(" → ")})`);
        check(entry.transactions.some((made) => made.role === "carrier"), `${run}: one carrier transaction created, approved and executed it (${entry.transactions.map((made) => made.role).join(", ")})`);
        check(entry.ran[0] && entry.transactions.some((made) => made.signature === entry.ran[0].signature), `${run}: ran names that transaction`);
      }
    }
  }
  if (kind === "multisig-waits") {
    for (const run of ["memo", "message:memo", "transfer"]) {
      for (const entry of ofRun("waits", run)) {
        check(entry.states.includes("awaiting-signatures") && entry.state === "done", `${run}: waited, then done once Bob executed (${entry.states.join(" → ")})`);
        check(entry.transactions.some((made) => made.role === "proposal"), `${run}: its transaction is listed as the proposal`);
        check(entry.ran[0] && !entry.transactions.some((made) => made.signature === entry.ran[0].signature), `${run}: ran names the executing transaction, not the proposal`);
      }
    }
    for (const run of ["mint:classic", "mint:token-2022"]) {
      const entries = ofRun("waits", run);
      check(entries.length > 0 || (only && !selectRuns(only).some((each) => each.id === run)), `${run}: the multisig took the request (${entries.length} resolution)`);
      for (const entry of entries) {
        check(entry.states.includes("awaiting-signatures") && entry.state === "done", `${run}: waited, then done once Bob executed (${entry.states.join(" → ")})`);
        await checkCarriedMint(entry, run);
      }
    }
    for (const entry of ofRun("together", "memo")) {
      check(entry.state === "done" && !entry.states.includes("awaiting-signatures"), `together, memo: done at once (${entry.states.join(" → ")})`);
    }
    check(profiles.some((item) => item.profile.timing === "sync"), "with Bob ticked the profile turns sync");
    for (const [phaseName, words] of [["rejected", "rejected"], ["cancelled", "cancelled"]]) {
      const entries = ofRun(phaseName, "memo");
      check(entries.length === 1 && entries[0].state === "failed" && entries[0].states.includes("awaiting-signatures"), `${phaseName}: the resolution waited, then failed (${entries.map((entry) => entry.states.join(" → ")).join("; ")})`);
      const error = entries[0]?.error;
      check(error?.message?.includes(words), `${phaseName}: its error says the proposal was ${words} (${error?.message ?? "no error"})`);
    }
  }
  if (kind === "offline") {
    for (const entry of ofRun("offline", "memo")) {
      check(entry.states.includes("awaiting-signatures") && entry.state === "done", `memo: waited for the other device, then done (${entry.states.join(" → ")})`);
    }
  }
};
