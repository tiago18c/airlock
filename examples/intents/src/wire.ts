/** Reads a transaction off the wire, in any of the three formats a wallet may
 * hand back: legacy, v0 and v1. web3.js 1.x reads only the first two, and
 * Airlock converts to v1 whenever it rebuilds. */

import { encodeBase58 } from "../../../extension/src/protocol";

export type Format = "legacy" | "v0" | "v1";

export interface CompiledInstruction {
  programIndex: number;
  accounts: number[];
  data: Uint8Array;
}

export interface Lookup {
  table: string;
  writable: number[];
  readonly: number[];
}

export interface ParsedTransaction {
  format: Format;
  signatures: Uint8Array[];
  /** Where each signature starts in the wire. */
  signatureOffsets: number[];
  /** The bytes a signer signs. */
  message: Uint8Array;
  numRequiredSignatures: number;
  numReadonlySigned: number;
  numReadonlyUnsigned: number;
  staticKeys: string[];
  blockhash: string;
  instructions: CompiledInstruction[];
  lookups: Lookup[];
}

const SIGNATURE = 64;
const KEY = 32;
const V0 = 0x80;
const V1 = 0x81;

class Reader {
  offset = 0;
  constructor(readonly bytes: Uint8Array) {}

  u8(): number {
    if (this.offset >= this.bytes.length) throw new Error("The transaction ends early.");
    return this.bytes[this.offset++];
  }

  take(length: number): Uint8Array {
    if (this.offset + length > this.bytes.length) throw new Error("The transaction ends early.");
    const out = this.bytes.subarray(this.offset, this.offset + length);
    this.offset += length;
    return out;
  }

  /** A compact-u16, as Solana writes lengths. */
  shortvec(): number {
    let value = 0;
    for (let shift = 0; shift < 21; shift += 7) {
      const byte = this.u8();
      value |= (byte & 0x7f) << shift;
      if ((byte & 0x80) === 0) return value;
    }
    throw new Error("A length in the transaction is too long.");
  }

  u16le(): number {
    return this.u8() | (this.u8() << 8);
  }

  u32le(): number {
    return (this.u8() | (this.u8() << 8) | (this.u8() << 16) | (this.u8() << 24)) >>> 0;
  }
}

export function parseTransaction(bytes: Uint8Array): ParsedTransaction {
  if (bytes.length === 0) throw new Error("The transaction is empty.");
  // A v1 transaction starts with its message; the others with their signatures.
  if (bytes[0] === V1) return parseV1(bytes);
  const reader = new Reader(bytes);
  const count = reader.shortvec();
  const signatureOffsets: number[] = [];
  const signatures: Uint8Array[] = [];
  for (let i = 0; i < count; i += 1) {
    signatureOffsets.push(reader.offset);
    signatures.push(reader.take(SIGNATURE));
  }
  const messageStart = reader.offset;
  const versioned = (bytes[messageStart] & 0x80) !== 0;
  if (versioned) {
    const version = reader.u8() & 0x7f;
    if (version !== 0) throw new Error(`Transaction version ${version} is not one this page reads.`);
  }
  const numRequiredSignatures = reader.u8();
  const numReadonlySigned = reader.u8();
  const numReadonlyUnsigned = reader.u8();
  const staticKeys = readKeys(reader, reader.shortvec());
  const blockhash = encodeBase58(reader.take(KEY));
  const instructions: CompiledInstruction[] = [];
  const instructionCount = reader.shortvec();
  for (let i = 0; i < instructionCount; i += 1) {
    const programIndex = reader.u8();
    const accounts = [...reader.take(reader.shortvec())];
    const data = reader.take(reader.shortvec()).slice();
    instructions.push({ programIndex, accounts, data });
  }
  const lookups: Lookup[] = [];
  if (versioned) {
    const lookupCount = reader.shortvec();
    for (let i = 0; i < lookupCount; i += 1) {
      const table = encodeBase58(reader.take(KEY));
      const writable = [...reader.take(reader.shortvec())];
      const readonly = [...reader.take(reader.shortvec())];
      lookups.push({ table, writable, readonly });
    }
  }
  if (count !== numRequiredSignatures) {
    throw new Error("The transaction has a different number of signatures than its message asks for.");
  }
  return {
    format: versioned ? "v0" : "legacy",
    signatures,
    signatureOffsets,
    message: bytes.slice(messageStart, reader.offset),
    numRequiredSignatures,
    numReadonlySigned,
    numReadonlyUnsigned,
    staticKeys,
    blockhash,
    instructions,
    lookups,
  };
}

/** SIMD-0385: the message first, with its compute budget in a config mask
 * rather than in instructions, and the signatures at the end. */
function parseV1(bytes: Uint8Array): ParsedTransaction {
  const reader = new Reader(bytes);
  reader.u8();
  const numRequiredSignatures = reader.u8();
  const numReadonlySigned = reader.u8();
  const numReadonlyUnsigned = reader.u8();
  const mask = reader.u32le();
  const blockhash = encodeBase58(reader.take(KEY));
  const instructionCount = reader.u8();
  const staticKeys = readKeys(reader, reader.u8());
  // Each bit of the mask stands for four bytes of config.
  let configBytes = 0;
  for (let bits = mask; bits !== 0; bits >>>= 1) configBytes += (bits & 1) * 4;
  reader.take(configBytes);
  const headers: { programIndex: number; accounts: number; data: number }[] = [];
  for (let i = 0; i < instructionCount; i += 1) {
    headers.push({ programIndex: reader.u8(), accounts: reader.u8(), data: reader.u16le() });
  }
  const instructions = headers.map((header) => ({
    programIndex: header.programIndex,
    accounts: [...reader.take(header.accounts)],
    data: reader.take(header.data).slice(),
  }));
  const message = bytes.slice(0, reader.offset);
  const signatureOffsets: number[] = [];
  const signatures: Uint8Array[] = [];
  for (let i = 0; i < numRequiredSignatures; i += 1) {
    signatureOffsets.push(reader.offset);
    signatures.push(reader.take(SIGNATURE));
  }
  return {
    format: "v1",
    signatures,
    signatureOffsets,
    message,
    numRequiredSignatures,
    numReadonlySigned,
    numReadonlyUnsigned,
    staticKeys,
    blockhash,
    instructions,
    lookups: [],
  };
}

function readKeys(reader: Reader, count: number): string[] {
  const keys: string[] = [];
  for (let i = 0; i < count; i += 1) keys.push(encodeBase58(reader.take(KEY)));
  return keys;
}

export function isSigner(tx: ParsedTransaction, index: number): boolean {
  return index < tx.numRequiredSignatures;
}

export function isWritable(tx: ParsedTransaction, index: number): boolean {
  if (index < tx.numRequiredSignatures) return index < tx.numRequiredSignatures - tx.numReadonlySigned;
  if (index < tx.staticKeys.length) return index < tx.staticKeys.length - tx.numReadonlyUnsigned;
  // An address from a lookup table: writable ones come first.
  const loaded = index - tx.staticKeys.length;
  const writable = tx.lookups.reduce((sum, lookup) => sum + lookup.writable.length, 0);
  return loaded < writable;
}

/** The signers a transaction asks for whose signature is still all zeros. */
export function missingSigners(tx: ParsedTransaction): string[] {
  return tx.signatures
    .map((signature, index) => (signature.every((byte) => byte === 0) ? tx.staticKeys[index] : null))
    .filter((key): key is string => key !== null);
}

/** `wire` with `signature` written into `signer`'s slot. */
export function withSignature(wire: Uint8Array, signer: string, signature: Uint8Array): Uint8Array {
  const tx = parseTransaction(wire);
  const index = tx.staticKeys.slice(0, tx.numRequiredSignatures).indexOf(signer);
  if (index < 0) throw new Error(`${signer} is not a signer of this transaction.`);
  if (signature.length !== SIGNATURE) throw new Error("A signature is 64 bytes.");
  const out = wire.slice();
  out.set(signature, tx.signatureOffsets[index]);
  return out;
}
