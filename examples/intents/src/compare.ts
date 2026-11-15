/** Whether a transaction a wallet composed is the one the app sent, changed
 * only as the app's notes allow (docs/intents.md, "Delivery"). The app runs
 * this before it signs anything that came back. */

import type { AccountNote, DataNote } from "../../../extension/src/intents";
import { decodeBase58 } from "../../../extension/src/protocol";
import { isSigner, isWritable, parseTransaction, type ParsedTransaction } from "./wire";

export const COMPUTE_BUDGET = "ComputeBudget111111111111111111111111111111";

export interface CompareInput {
  /** The transaction as the app sent it. */
  sent: Uint8Array;
  /** The transaction the wallet composed. */
  composed: Uint8Array;
  /** Where `sent` was in the request, which is what `data` notes count by. */
  transaction: number;
  /** The account acted as. When it pays the fee of `sent`, who pays is the
   * wallet's to choose, as the blockhash is. */
  account?: string;
  notes?: Record<string, AccountNote>;
  data?: DataNote[];
  /** From the resolution: address sent, address used. */
  replaced?: Record<string, string>;
  /** The composed transaction's role in the resolution. */
  role?: string;
}

export type Verdict =
  | { verdict: "same"; detail: string }
  | { verdict: "different"; detail: string }
  | { verdict: "cannot-check"; detail: string };

interface Meta {
  pubkey: string;
  signer: boolean;
  writable: boolean;
}

interface Instruction {
  program: string;
  accounts: Meta[];
  data: Uint8Array;
  /** Bytes of `data` the wallet may have rewritten to a value this page cannot work out. */
  free: Set<number>;
  /** Where it sat in its own transaction, compute budget included. */
  index: number;
}

export function compareComposed(input: CompareInput): Verdict {
  if (input.role && input.role !== "direct") {
    return {
      verdict: "cannot-check",
      detail: `cannot check: carrier. The wallet runs these instructions inside a program (role "${input.role}"), and this page reads only instructions at the top of a transaction.`,
    };
  }
  let sent: ParsedTransaction;
  let composed: ParsedTransaction;
  try {
    sent = parseTransaction(input.sent);
  } catch (err) {
    return { verdict: "cannot-check", detail: `cannot check: the transaction sent does not read (${message(err)}).` };
  }
  try {
    composed = parseTransaction(input.composed);
  } catch (err) {
    return { verdict: "different", detail: `The composed transaction does not read as a transaction: ${message(err)}.` };
  }
  if (usesLookups(sent) || usesLookups(composed)) {
    return {
      verdict: "cannot-check",
      detail: "cannot check: lookup tables. An account comes from a lookup table, which this page does not read from the chain.",
    };
  }

  const notes = input.notes ?? {};
  const replaced = input.replaced ?? {};
  for (const [from, to] of Object.entries(replaced)) {
    if (from === to) continue;
    const note = notes[from];
    const allowed =
      note !== undefined &&
      ((note.kind === "new" && note.replaceable) || (note.kind === "feePayer" && note.replaceable) || note.kind === "derived");
    if (!allowed) {
      return { verdict: "different", detail: `${from} became ${to}, but no note lets the wallet replace it.` };
    }
  }
  const use = (address: string) => replaced[address] ?? address;

  const sentPayer = sent.staticKeys[0];
  const payerNote = notes[sentPayer];
  const payerReplaceable = payerNote?.kind === "feePayer" && payerNote.replaceable;
  // The account acted as carries no note. Paying its own fee, it may have the
  // wallet pay from another key, and every instruction stays as written.
  const accountPays = input.account !== undefined && sentPayer === input.account;
  const composedPayer = composed.staticKeys[0];
  const payerChanged = composedPayer !== use(sentPayer);
  if (payerChanged && !payerReplaceable && !accountPays) {
    return {
      verdict: "different",
      detail: `The fee payer changed from ${sentPayer} to ${composedPayer}, and no note allows that.`,
    };
  }
  // Paying makes an account a writable signer whatever the instructions say of
  // it, so the old payer's and the new payer's flags are not compared. The
  // account acted as still signs: only its writable flag may follow the fee.
  // A payer replaced through its note counts as moved too.
  const payerMoved = composedPayer !== sentPayer;
  const looseWritable = new Set(payerMoved ? [composedPayer, use(sentPayer)] : []);
  const looseSigner = new Set(payerMoved && payerReplaceable ? [composedPayer, use(sentPayer)] : []);

  const expected = instructionsOf(sent)
    .map((instruction) => rewrite(instruction, input, use))
    .filter((instruction) => instruction.program !== COMPUTE_BUDGET);
  const actual = instructionsOf(composed).filter((instruction) => instruction.program !== COMPUTE_BUDGET);

  if (expected.length !== actual.length) {
    return {
      verdict: "different",
      detail: `The app sent ${expected.length} instructions (compute budget aside) and the wallet composed ${actual.length}.`,
    };
  }
  for (let i = 0; i < expected.length; i += 1) {
    const difference = differs(expected[i], actual[i], looseWritable, looseSigner);
    if (difference) {
      return { verdict: "different", detail: `Instruction ${expected[i].index} of the transaction sent: ${difference}` };
    }
  }
  const swaps = Object.entries(replaced).filter(([from, to]) => from !== to).length;
  return {
    verdict: "same",
    detail:
      `Same ${expected.length === 1 ? "instruction" : `${expected.length} instructions`}: programs, accounts with their signer and writable flags, and data` +
      (swaps ? `, after ${swaps === 1 ? "the 1 declared replacement" : `the ${swaps} declared replacements`}` : "") +
      (payerMoved
        ? `. The wallet pays the fee from ${composedPayer}; the blockhash and the compute budget were left to it too.`
        : payerReplaceable || accountPays
          ? ". The blockhash, the compute budget and the fee payer were left to the wallet."
          : ". The blockhash and the compute budget were left to the wallet."),
  };
}

function usesLookups(tx: ParsedTransaction): boolean {
  return tx.lookups.some((lookup) => lookup.writable.length + lookup.readonly.length > 0);
}

function instructionsOf(tx: ParsedTransaction): Instruction[] {
  return tx.instructions.map((instruction, index) => ({
    program: tx.staticKeys[instruction.programIndex],
    accounts: instruction.accounts.map((account) => ({
      pubkey: tx.staticKeys[account],
      signer: isSigner(tx, account),
      writable: isWritable(tx, account),
    })),
    data: instruction.data,
    free: new Set<number>(),
    index,
  }));
}

/** The instruction as the app would expect it back: each declared replacement
 * applied, in the accounts and at each `data` entry. */
function rewrite(instruction: Instruction, input: CompareInput, use: (address: string) => string): Instruction {
  const data = instruction.data.slice();
  const free = new Set<number>();
  for (const note of input.data ?? []) {
    if (note.transaction !== input.transaction || note.instruction !== instruction.index) continue;
    if ("account" in note) {
      const to = use(note.account);
      if (to === note.account) continue;
      const bytes = decodeBase58(to);
      if (bytes && bytes.length === 32 && note.offset + 32 <= data.length) data.set(bytes, note.offset);
    } else if (use(note.bumpOf) !== note.bumpOf && note.offset < data.length) {
      // A new address has a new bump, which this page leaves to the wallet.
      free.add(note.offset);
    }
  }
  return {
    ...instruction,
    program: use(instruction.program),
    accounts: instruction.accounts.map((meta) => ({ ...meta, pubkey: use(meta.pubkey) })),
    data,
    free,
  };
}

function differs(expected: Instruction, actual: Instruction, looseWritable: Set<string>, looseSigner: Set<string>): string | null {
  if (expected.program !== actual.program) {
    return `the program is ${actual.program}, not ${expected.program}.`;
  }
  if (expected.accounts.length !== actual.accounts.length) {
    return `it has ${actual.accounts.length} accounts, not ${expected.accounts.length}.`;
  }
  for (let i = 0; i < expected.accounts.length; i += 1) {
    const want = expected.accounts[i];
    const got = actual.accounts[i];
    if (want.pubkey !== got.pubkey) return `account ${i} is ${got.pubkey}, not ${want.pubkey}.`;
    if (want.signer !== got.signer && !looseSigner.has(got.pubkey)) {
      return `account ${i} (${got.pubkey}) ${got.signer ? "now signs" : "no longer signs"}.`;
    }
    if (want.writable !== got.writable && !looseWritable.has(got.pubkey)) {
      return `account ${i} (${got.pubkey}) is ${got.writable ? "writable" : "read-only"} now.`;
    }
  }
  if (expected.data.length !== actual.data.length) {
    return `its data is ${actual.data.length} bytes, not ${expected.data.length}.`;
  }
  for (let i = 0; i < expected.data.length; i += 1) {
    if (expected.free.has(i)) continue;
    if (expected.data[i] !== actual.data[i]) return `its data differs at byte ${i}.`;
  }
  return null;
}

function message(err: unknown): string {
  return err instanceof Error ? err.message.replace(/\.$/, "") : String(err);
}
