/** The few instructions the page builds, written by hand so it needs no
 * token or memo package. */

import { PublicKey, SystemProgram, TransactionInstruction } from "@solana/web3.js";
// The Buffer web3.js itself bundles: a browser has no global one.
import { Buffer } from "buffer";

export const MEMO_PROGRAM = new PublicKey("MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr");
export const TOKEN_PROGRAM = new PublicKey("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA");
export const TOKEN_2022_PROGRAM = new PublicKey("TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb");
export const ATA_PROGRAM = new PublicKey("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL");
/** What a mint account takes, and so what it must hold to be rent exempt. */
export const MINT_SIZE = 82;

/* Token-2022's layout, as spl-token-2022-interface 3.1 has it. A mint with
 * extensions is padded to the size of a token account (165), then one byte
 * says it is a mint, then each extension as a u16 type, a u16 length and its
 * bytes. */
const TOKEN_2022_BASE = 165;
const ACCOUNT_TYPE_MINT = 1;
/** `ExtensionType::MetadataPointer`. */
export const EXTENSION_METADATA_POINTER = 18;
/** A mint whose one extension is a metadata pointer: 165 + 1 + 4 + 64. */
export const MINT_WITH_POINTER_SIZE = TOKEN_2022_BASE + 1 + 4 + 64;
/** `TokenInstruction::MetadataPointerExtension`, then `MetadataPointerInstruction::Initialize`. */
const METADATA_POINTER_EXTENSION = 39;
const METADATA_POINTER_INITIALIZE = 0;
/** Where the metadata address sits in InitializeMetadataPointer's data: after
 * the two tags and the authority. */
export const METADATA_ADDRESS_OFFSET = 2 + 32;

/** A memo that every account in `signers` signs. The Memo program fails
 * unless each of them did. */
export function memo(text: string | Uint8Array, signers: PublicKey[]): TransactionInstruction {
  return new TransactionInstruction({
    programId: MEMO_PROGRAM,
    keys: signers.map((pubkey) => ({ pubkey, isSigner: true, isWritable: false })),
    data: toBuffer(typeof text === "string" ? new TextEncoder().encode(text) : text),
  });
}

export function createMintAccount(
  payer: PublicKey,
  mint: PublicKey,
  lamports: number,
  space = MINT_SIZE,
  tokenProgram = TOKEN_PROGRAM,
): TransactionInstruction {
  return SystemProgram.createAccount({
    fromPubkey: payer,
    newAccountPubkey: mint,
    lamports,
    space,
    programId: tokenProgram,
  });
}

/** Offset of the mint authority in InitializeMint2's data: after the tag and
 * the decimals. The mint's own address is not in its data. */
export const MINT_AUTHORITY_OFFSET = 2;

/** SPL Token InitializeMint2: tag 20, decimals, the mint authority, and no
 * freeze authority. Token-2022 takes the same bytes. */
export function initializeMint2(mint: PublicKey, decimals: number, authority: PublicKey, tokenProgram = TOKEN_PROGRAM): TransactionInstruction {
  const data = new Uint8Array(1 + 1 + 32 + 1);
  data[0] = 20;
  data[1] = decimals;
  data.set(authority.toBytes(), MINT_AUTHORITY_OFFSET);
  data[34] = 0;
  return new TransactionInstruction({
    programId: tokenProgram,
    keys: [{ pubkey: mint, isSigner: false, isWritable: true }],
    data: toBuffer(data),
  });
}

/** Token-2022 InitializeMetadataPointer: tags 39 and 0, the authority that may
 * change the pointer, then the account that holds the metadata. It has to
 * come before InitializeMint2. Unlike that instruction, this one writes an
 * address the wallet may replace (the mint's own) into its data. */
export function initializeMetadataPointer(mint: PublicKey, authority: PublicKey, metadata: PublicKey): TransactionInstruction {
  const data = new Uint8Array(2 + 32 + 32);
  data[0] = METADATA_POINTER_EXTENSION;
  data[1] = METADATA_POINTER_INITIALIZE;
  data.set(authority.toBytes(), 2);
  data.set(metadata.toBytes(), METADATA_ADDRESS_OFFSET);
  return new TransactionInstruction({
    programId: TOKEN_2022_PROGRAM,
    keys: [{ pubkey: mint, isSigner: false, isWritable: true }],
    data: toBuffer(data),
  });
}

/** The associated token account of `owner` for `mint`, and its bump. */
export function associatedTokenAddress(owner: PublicKey, mint: PublicKey, tokenProgram = TOKEN_PROGRAM): [PublicKey, number] {
  return PublicKey.findProgramAddressSync([owner.toBytes(), tokenProgram.toBytes(), mint.toBytes()], ATA_PROGRAM);
}

/** Associated Token Account Create (tag 0). Its data carries no bump: the
 * program derives the address itself. */
export function createAssociatedTokenAccount(
  payer: PublicKey,
  ata: PublicKey,
  owner: PublicKey,
  mint: PublicKey,
  tokenProgram = TOKEN_PROGRAM,
): TransactionInstruction {
  return new TransactionInstruction({
    programId: ATA_PROGRAM,
    keys: [
      { pubkey: payer, isSigner: true, isWritable: true },
      { pubkey: ata, isSigner: false, isWritable: true },
      { pubkey: owner, isSigner: false, isWritable: false },
      { pubkey: mint, isSigner: false, isWritable: false },
      { pubkey: SystemProgram.programId, isSigner: false, isWritable: false },
      { pubkey: tokenProgram, isSigner: false, isWritable: false },
    ],
    data: toBuffer(new Uint8Array([0])),
  });
}

/** SPL Token MintTo: tag 7, then the amount as a little-endian u64. */
export function mintTo(mint: PublicKey, destination: PublicKey, authority: PublicKey, amount: bigint, tokenProgram = TOKEN_PROGRAM): TransactionInstruction {
  const data = new Uint8Array(9);
  data[0] = 7;
  new DataView(data.buffer).setBigUint64(1, amount, true);
  return new TransactionInstruction({
    programId: tokenProgram,
    keys: [
      { pubkey: mint, isSigner: false, isWritable: true },
      { pubkey: destination, isSigner: false, isWritable: true },
      { pubkey: authority, isSigner: true, isWritable: false },
    ],
    data: toBuffer(data),
  });
}

function toBuffer(bytes: Uint8Array): Buffer {
  return Buffer.from(bytes);
}

/** A mint account as SPL Token lays it out, read enough to show it exists. */
export function readMint(data: Uint8Array): { supply: bigint; decimals: number; initialized: boolean; authority: string | null } {
  const view = new DataView(data.buffer, data.byteOffset, data.byteLength);
  const hasAuthority = view.getUint32(0, true) === 1;
  return {
    authority: hasAuthority ? new PublicKey(data.subarray(4, 36)).toBase58() : null,
    supply: view.getBigUint64(36, true),
    decimals: data[44],
    initialized: data[45] === 1,
  };
}

/** The metadata pointer of a Token-2022 mint, or null when it has none. Walks
 * the extensions after the account type byte. */
export function readMetadataPointer(data: Uint8Array): { authority: string | null; metadata: string | null } | null {
  if (data.length <= TOKEN_2022_BASE || data[TOKEN_2022_BASE] !== ACCOUNT_TYPE_MINT) return null;
  const view = new DataView(data.buffer, data.byteOffset, data.byteLength);
  const orNull = (bytes: Uint8Array) => (bytes.every((b) => b === 0) ? null : new PublicKey(bytes).toBase58());
  let at = TOKEN_2022_BASE + 1;
  while (at + 4 <= data.length) {
    const type = view.getUint16(at, true);
    const length = view.getUint16(at + 2, true);
    const body = data.subarray(at + 4, at + 4 + length);
    if (type === EXTENSION_METADATA_POINTER && body.length === 64) {
      return { authority: orNull(body.subarray(0, 32)), metadata: orNull(body.subarray(32, 64)) };
    }
    if (type === 0) break;
    at += 4 + length;
  }
  return null;
}

/** A token account: its mint, owner and amount. */
export function readTokenAccount(data: Uint8Array): { mint: string; owner: string; amount: bigint } {
  const view = new DataView(data.buffer, data.byteOffset, data.byteLength);
  return {
    mint: new PublicKey(data.subarray(0, 32)).toBase58(),
    owner: new PublicKey(data.subarray(32, 64)).toBase58(),
    amount: view.getBigUint64(64, true),
  };
}
