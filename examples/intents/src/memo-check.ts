/** The check docs/intents.md asks of a message signed as a memo: the
 * transaction succeeded, and holds a Memo instruction, at the top or inner,
 * whose data is the message and whose accounts are exactly the account. */

import { base64ToBytes, decodeBase58 } from "../../../extension/src/protocol";
import { parseTransaction } from "./wire";

export const MEMO = "MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr";

/** What `getTransaction` answers, as far as this check reads it. */
export interface FetchedTransaction {
  version?: number | "legacy";
  transaction: [string, string] | string[];
  meta: {
    err: unknown;
    innerInstructions?: { index: number; instructions: { programIdIndex: number; accounts: number[]; data: string }[] }[] | null;
    loadedAddresses?: { writable: string[]; readonly: string[] } | null;
  } | null;
}

export interface MemoVerdict {
  holds: boolean;
  detail: string;
}

export function checkMemo(fetched: FetchedTransaction, account: string, message: Uint8Array): MemoVerdict {
  if (!fetched.meta) return { holds: false, detail: "The node gave the transaction without its outcome, so it proves nothing." };
  if (fetched.meta.err !== null && fetched.meta.err !== undefined) {
    return { holds: false, detail: `The transaction failed (${JSON.stringify(fetched.meta.err)}), so it proves nothing.` };
  }
  const tx = parseTransaction(base64ToBytes(fetched.transaction[0]));
  const keys = [
    ...tx.staticKeys,
    ...(fetched.meta.loadedAddresses?.writable ?? []),
    ...(fetched.meta.loadedAddresses?.readonly ?? []),
  ];
  const candidates: { where: string; program: string; accounts: string[]; data: Uint8Array }[] = tx.instructions.map(
    (instruction, index) => ({
      where: `instruction ${index}`,
      program: keys[instruction.programIndex],
      accounts: instruction.accounts.map((i) => keys[i]),
      data: instruction.data,
    }),
  );
  for (const group of fetched.meta.innerInstructions ?? []) {
    group.instructions.forEach((instruction, position) => {
      candidates.push({
        where: `inner instruction ${position} of instruction ${group.index}`,
        program: keys[instruction.programIdIndex],
        accounts: instruction.accounts.map((i) => keys[i]),
        data: decodeBase58(instruction.data) ?? new Uint8Array(),
      });
    });
  }
  const memos = candidates.filter((candidate) => candidate.program === MEMO);
  if (memos.length === 0) return { holds: false, detail: "The transaction holds no Memo instruction." };
  const match = memos.find(
    (candidate) => candidate.accounts.length === 1 && candidate.accounts[0] === account && sameBytes(candidate.data, message),
  );
  if (match) {
    return {
      holds: true,
      detail: `It holds: ${match.where} is a Memo with exactly the account and the message, and the transaction succeeded.`,
    };
  }
  const near = memos[0];
  if (!sameBytes(near.data, message)) {
    return { holds: false, detail: `The Memo at ${near.where} carries other text than the message.` };
  }
  return {
    holds: false,
    detail: `The Memo at ${near.where} names ${near.accounts.length} accounts (${near.accounts.join(", ")}), not exactly the account.`,
  };
}

/** What the page could establish about a memo's proof:
 * - `holds`: the node showed the transaction, it succeeded, and it holds the
 *   Memo with exactly the account and the message;
 * - `fails`: the node showed it, and it failed or does not hold that;
 * - `unchecked`: the node would not show its contents. `succeeded` then
 *   says what its status alone said: true, false, or null when it gave none.
 *   That a transaction succeeded proves nothing about what it held, so the
 *   proof is neither passed nor failed. */
export type MemoProof =
  | { verdict: "holds" | "fails"; detail: string; version: number | "legacy" | null }
  | { verdict: "unchecked"; detail: string; succeeded: boolean | null; refusal: string };

/** One JSON-RPC call; throws `RpcRefusal` when the node answers an error. */
export type Rpc = (method: string, params: unknown[]) => Promise<unknown>;

export class RpcRefusal extends Error {
  constructor(
    message: string,
    readonly data: unknown,
  ) {
    super(message);
  }

  /** The node's words, with what it put in `data`, where nodes say why. */
  get words(): string {
    return typeof this.data === "string" && this.data ? `${this.message}: ${this.data}` : this.message;
  }
}

/** Reads transaction `signature` and checks the memo in it. A node refuses
 * a version 1 transaction unless it is asked for that version
 * (`maxSupportedTransactionVersion: 1`), and one that does not know version
 * 1 refuses it then too: the page then reads only its status, and says the
 * proof was not checked. */
export async function proveMemo(rpc: Rpc, signature: string, account: string, message: Uint8Array): Promise<MemoProof | null> {
  const ask = async (version: number): Promise<{ fetched: FetchedTransaction | null } | { refusal: RpcRefusal }> => {
    try {
      const fetched = await rpc("getTransaction", [signature, { encoding: "base64", commitment: "confirmed", maxSupportedTransactionVersion: version }]);
      return { fetched: (fetched as FetchedTransaction | null) ?? null };
    } catch (err) {
      if (err instanceof RpcRefusal) return { refusal: err };
      throw err;
    }
  };
  let answer = await ask(0);
  // Only a refusal over the version is worth asking again.
  if ("refusal" in answer && /version/i.test(answer.refusal.words)) answer = await ask(1);
  const refusal = "refusal" in answer ? answer.refusal : null;
  const fetched = "fetched" in answer ? answer.fetched : null;
  if (fetched) {
    const verdict = checkMemo(fetched, account, message);
    return { verdict: verdict.holds ? "holds" : "fails", detail: verdict.detail, version: fetched.version ?? null };
  }
  if (!refusal) return null;
  const statuses = (await rpc("getSignatureStatuses", [[signature], { searchTransactionHistory: true }]).catch(() => null)) as {
    value?: ({ err: unknown; confirmationStatus?: string } | null)[];
  } | null;
  const status = statuses?.value?.[0] ?? null;
  const succeeded = status ? status.err === null || status.err === undefined : null;
  const said =
    succeeded === true
      ? "Its status says it succeeded"
      : succeeded === false
        ? `Its status says it failed (${JSON.stringify(status?.err)})`
        : "The node gave no status for it either";
  return {
    verdict: "unchecked",
    succeeded,
    refusal: refusal.words,
    detail: `The proof was not checked: this node would not show the transaction's contents (${refusal.words}). ${said}, but that says nothing of what it held, so the page can neither pass nor fail it.`,
  };
}

function sameBytes(left: Uint8Array, right: Uint8Array): boolean {
  return left.length === right.length && left.every((byte, i) => byte === right[i]);
}
