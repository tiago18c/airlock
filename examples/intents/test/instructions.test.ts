import assert from "node:assert/strict";
import { test } from "node:test";
import { PublicKey, SystemProgram } from "@solana/web3.js";
import type { AccountNote, DataNote } from "../../../extension/src/intents";
import { compareComposed } from "../src/compare";
import {
  ATA_PROGRAM,
  EXTENSION_METADATA_POINTER,
  METADATA_ADDRESS_OFFSET,
  MINT_WITH_POINTER_SIZE,
  TOKEN_2022_PROGRAM,
  associatedTokenAddress,
  createAssociatedTokenAccount,
  createMintAccount,
  initializeMetadataPointer,
  initializeMint2,
  mintTo,
  readMetadataPointer,
} from "../src/instructions";
import { token2022MintRequest } from "../src/token-2022";
import { key, legacy } from "./helpers";

test("InitializeMetadataPointer is tags 39 and 0, the authority, then the metadata address at byte 34", () => {
  const mint = key();
  const authority = key();
  const ix = initializeMetadataPointer(mint, authority, mint);
  assert.equal(ix.programId.toBase58(), "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb");
  assert.equal(ix.data.length, 66);
  assert.deepEqual([...ix.data.subarray(0, 2)], [39, 0]);
  assert.deepEqual(new Uint8Array(ix.data.subarray(2, 34)), authority.toBytes());
  assert.equal(METADATA_ADDRESS_OFFSET, 34);
  assert.deepEqual(new Uint8Array(ix.data.subarray(34, 66)), mint.toBytes());
  assert.deepEqual(
    ix.keys.map((meta) => [meta.pubkey.toBase58(), meta.isSigner, meta.isWritable]),
    [[mint.toBase58(), false, true]],
  );
});

test("a mint with a metadata pointer takes 234 bytes, and its pointer reads back", () => {
  assert.equal(MINT_WITH_POINTER_SIZE, 234);
  const authority = key();
  const metadata = key();
  // Base mint, padding to 165, the account type, then one extension.
  const data = new Uint8Array(MINT_WITH_POINTER_SIZE);
  data[45] = 1;
  data[165] = 1;
  new DataView(data.buffer).setUint16(166, EXTENSION_METADATA_POINTER, true);
  new DataView(data.buffer).setUint16(168, 64, true);
  data.set(authority.toBytes(), 170);
  data.set(metadata.toBytes(), 202);
  assert.deepEqual(readMetadataPointer(data), { authority: authority.toBase58(), metadata: metadata.toBase58() });
  // A classic mint has no extensions.
  assert.equal(readMetadataPointer(new Uint8Array(82)), null);
});

test("the Token-2022 request declares the mint, its token account and the address in the pointer's data", () => {
  const owner = key();
  const mint = key();
  const request = token2022MintRequest(owner, mint, 2_074_080);
  const [ata] = associatedTokenAddress(owner, mint, TOKEN_2022_PROGRAM);
  assert.equal(request.ata.toBase58(), ata.toBase58());
  assert.deepEqual(
    request.instructions.map((ix) => ix.programId.toBase58()),
    [SystemProgram.programId, TOKEN_2022_PROGRAM, TOKEN_2022_PROGRAM, ATA_PROGRAM, TOKEN_2022_PROGRAM].map((p) => p.toBase58()),
  );
  // CreateAccount asks for the 234 bytes, owned by Token-2022.
  const create = request.instructions[0].data;
  assert.equal(new DataView(create.buffer, create.byteOffset).getBigUint64(12, true), 234n);
  assert.deepEqual(new Uint8Array(create.subarray(20, 52)), TOKEN_2022_PROGRAM.toBytes());

  const ataNote = request.accounts[ata.toBase58()];
  assert.equal(ataNote.kind, "derived");
  assert.ok(ataNote.kind === "derived" && "pda" in ataNote);
  if (ataNote.kind === "derived" && "pda" in ataNote) {
    const seeds = ataNote.pda.seeds.map((seed) => ("account" in seed ? new PublicKey(seed.account).toBytes() : (seed.bytes as Uint8Array)));
    const [derived] = PublicKey.findProgramAddressSync(seeds, new PublicKey(ataNote.pda.program));
    assert.equal(derived.toBase58(), ata.toBase58());
    assert.deepEqual(seeds[1], TOKEN_2022_PROGRAM.toBytes());
  }
  assert.deepEqual(request.accounts[mint.toBase58()], { kind: "new", replaceable: true });
  assert.deepEqual(request.data, [{ transaction: 0, instruction: 1, offset: 34, account: mint.toBase58() }]);
  const pointer = request.instructions[1].data;
  assert.deepEqual(new Uint8Array(pointer.subarray(34, 66)), mint.toBytes());
});

test("a Token-2022 mint the wallet swapped is the same once its address is written at the data entry", () => {
  const owner = key();
  const mint = key();
  const theirs = key();
  const sent = token2022MintRequest(owner, mint, 1);
  const composed = token2022MintRequest(owner, theirs, 1);
  const replaced = { [mint.toBase58()]: theirs.toBase58(), [sent.ata.toBase58()]: composed.ata.toBase58() };
  const input = {
    sent: legacy(owner, sent.instructions),
    composed: legacy(owner, composed.instructions),
    transaction: 0,
    account: owner.toBase58(),
    notes: sent.accounts as Record<string, AccountNote>,
    replaced,
  };
  assert.equal(compareComposed({ ...input, data: sent.data as DataNote[] }).verdict, "same");
  // Without the entry the old address would have had to stay in the data.
  const without = compareComposed(input);
  assert.equal(without.verdict, "different");
  assert.match(without.detail, /Instruction 1 .*data differs at byte 34/);
});

test("the instructions take Token-2022 as their program where asked", () => {
  const owner = key();
  const mint = key();
  const [ata] = associatedTokenAddress(owner, mint, TOKEN_2022_PROGRAM);
  assert.equal(createMintAccount(owner, mint, 1, 234, TOKEN_2022_PROGRAM).keys.length, 2);
  assert.equal(initializeMint2(mint, 0, owner, TOKEN_2022_PROGRAM).programId.toBase58(), TOKEN_2022_PROGRAM.toBase58());
  const create = createAssociatedTokenAccount(owner, ata, owner, mint, TOKEN_2022_PROGRAM);
  assert.equal(create.keys[5].pubkey.toBase58(), TOKEN_2022_PROGRAM.toBase58());
  assert.equal(mintTo(mint, ata, owner, 5n, TOKEN_2022_PROGRAM).programId.toBase58(), TOKEN_2022_PROGRAM.toBase58());
});
