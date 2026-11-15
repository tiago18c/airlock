import {
  ComputeBudgetProgram,
  Keypair,
  PublicKey,
  Transaction,
  TransactionInstruction,
} from "@solana/web3.js";
import { parseTransaction } from "../src/wire";
import { decodeBase58 } from "../../../extension/src/protocol";

export const BLOCKHASH_A = new PublicKey(new Uint8Array(32).fill(1)).toBase58();
export const BLOCKHASH_B = new PublicKey(new Uint8Array(32).fill(2)).toBase58();

export function key(): PublicKey {
  return Keypair.generate().publicKey;
}

export function legacy(payer: PublicKey, instructions: TransactionInstruction[], blockhash = BLOCKHASH_A): Uint8Array {
  const tx = new Transaction({ feePayer: payer, recentBlockhash: blockhash }).add(...instructions);
  return new Uint8Array(tx.serialize({ requireAllSignatures: false, verifySignatures: false }));
}

export function withComputeBudget(instructions: TransactionInstruction[]): TransactionInstruction[] {
  return [ComputeBudgetProgram.setComputeUnitLimit({ units: 50_000 }), ...instructions];
}

/** The same transaction as `wire`, written as v1 (SIMD-0385), with a compute
 * unit limit in its config and no signatures yet. */
export function asV1(wire: Uint8Array, blockhash = BLOCKHASH_B): Uint8Array {
  const tx = parseTransaction(wire);
  const out: number[] = [0x81, tx.numRequiredSignatures, tx.numReadonlySigned, tx.numReadonlyUnsigned];
  const mask = 0b100;
  out.push(mask & 0xff, (mask >> 8) & 0xff, 0, 0);
  out.push(...decodeBase58(blockhash)!);
  out.push(tx.instructions.length, tx.staticKeys.length);
  for (const k of tx.staticKeys) out.push(...decodeBase58(k)!);
  out.push(0x50, 0xc3, 0, 0);
  for (const ix of tx.instructions) out.push(ix.programIndex, ix.accounts.length, ix.data.length & 0xff, ix.data.length >> 8);
  for (const ix of tx.instructions) out.push(...ix.accounts, ...ix.data);
  for (let i = 0; i < tx.numRequiredSignatures; i += 1) out.push(...new Uint8Array(64));
  return Uint8Array.from(out);
}
