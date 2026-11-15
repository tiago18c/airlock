import assert from "node:assert/strict";
import { test } from "node:test";
import {
  AddressLookupTableAccount,
  PublicKey,
  SystemProgram,
  TransactionInstruction,
  TransactionMessage,
  VersionedTransaction,
} from "@solana/web3.js";
import type { AccountNote, DataNote } from "../../../extension/src/intents";
import { compareComposed } from "../src/compare";
import {
  ATA_PROGRAM,
  TOKEN_PROGRAM,
  associatedTokenAddress,
  createAssociatedTokenAccount,
  createMintAccount,
  initializeMint2,
  memo,
  mintTo,
} from "../src/instructions";
import { BLOCKHASH_B, asV1, key, legacy, withComputeBudget } from "./helpers";

const account = key();
const app = key();

function mintTransaction(mint: PublicKey, owner = account, payer = account): Uint8Array {
  const [ata] = associatedTokenAddress(owner, mint);
  return legacy(
    payer,
    [
      createMintAccount(payer, mint, 1_461_600),
      initializeMint2(mint, 0, owner),
      createAssociatedTokenAccount(payer, ata, owner, mint),
      mintTo(mint, ata, owner, 1000n),
    ],
  );
}

function mintNotes(mint: PublicKey): Record<string, AccountNote> {
  const [ata] = associatedTokenAddress(account, mint);
  // The account acted as carries no note.
  return {
    [mint.toBase58()]: { kind: "new", replaceable: true },
    [ata.toBase58()]: {
      kind: "derived",
      pda: { program: ATA_PROGRAM.toBase58(), seeds: [{ account: account.toBase58() }, { account: TOKEN_PROGRAM.toBase58() }, { account: mint.toBase58() }] },
    },
  };
}

test("the same instructions on a new blockhash, with a compute budget added, are the same", () => {
  const sent = legacy(account, [memo("hi", [account])]);
  const composed = legacy(account, withComputeBudget([memo("hi", [account])]), BLOCKHASH_B);
  const verdict = compareComposed({ sent, composed, transaction: 0 });
  assert.equal(verdict.verdict, "same", verdict.detail);
});

test("a mint the wallet swapped for its own key is the same, once the declared replacements apply", () => {
  const mint = key();
  const theirs = key();
  const [ata] = associatedTokenAddress(account, mint);
  const [theirAta] = associatedTokenAddress(account, theirs);
  const verdict = compareComposed({
    sent: mintTransaction(mint),
    composed: mintTransaction(theirs),
    transaction: 0,
    notes: mintNotes(mint),
    replaced: { [mint.toBase58()]: theirs.toBase58(), [ata.toBase58()]: theirAta.toBase58() },
  });
  assert.equal(verdict.verdict, "same", verdict.detail);
  assert.match(verdict.detail, /2 declared replacements/);
});

test("a replacement nobody declared is different", () => {
  const mint = key();
  const theirs = key();
  const [ata] = associatedTokenAddress(account, mint);
  const [theirAta] = associatedTokenAddress(account, theirs);
  // replaced says nothing of the mint.
  const verdict = compareComposed({
    sent: mintTransaction(mint),
    composed: mintTransaction(theirs),
    transaction: 0,
    notes: mintNotes(mint),
    replaced: { [ata.toBase58()]: theirAta.toBase58() },
  });
  assert.equal(verdict.verdict, "different");

  // And replaced cannot name an account whose note does not allow it.
  const kept = compareComposed({
    sent: mintTransaction(mint),
    composed: mintTransaction(theirs),
    transaction: 0,
    notes: { ...mintNotes(mint), [mint.toBase58()]: { kind: "new", replaceable: false } },
    replaced: { [mint.toBase58()]: theirs.toBase58(), [ata.toBase58()]: theirAta.toBase58() },
  });
  assert.equal(kept.verdict, "different");
  assert.match(kept.detail, /no note lets the wallet replace it/);
});

test("other data, another program or another account is different", () => {
  const sent = legacy(account, [memo("hi", [account])]);
  assert.equal(compareComposed({ sent, composed: legacy(account, [memo("ho", [account])]), transaction: 0 }).verdict, "different");
  const transfer = legacy(account, [SystemProgram.transfer({ fromPubkey: account, toPubkey: app, lamports: 5 })]);
  const elsewhere = legacy(account, [SystemProgram.transfer({ fromPubkey: account, toPubkey: key(), lamports: 5 })]);
  const verdict = compareComposed({ sent: transfer, composed: elsewhere, transaction: 0 });
  assert.equal(verdict.verdict, "different");
  assert.match(verdict.detail, /account 1/);
  const extra = legacy(account, [memo("hi", [account]), memo("more", [account])]);
  assert.match(compareComposed({ sent, composed: extra, transaction: 0 }).detail, /sent 1 instructions/);
});

test("a flag that changed is different", () => {
  const other = key();
  const sent = legacy(account, [new TransactionInstruction({ programId: TOKEN_PROGRAM, keys: [{ pubkey: other, isSigner: false, isWritable: false }], data: Buffer.from([1]) })]);
  const composed = legacy(account, [new TransactionInstruction({ programId: TOKEN_PROGRAM, keys: [{ pubkey: other, isSigner: false, isWritable: true }], data: Buffer.from([1]) })]);
  const verdict = compareComposed({ sent, composed, transaction: 0 });
  assert.equal(verdict.verdict, "different");
  assert.match(verdict.detail, /writable now/);
});

test("a sponsor the wallet may replace can become the account, flags and all", () => {
  const sent = legacy(app, [memo("hi", [account])]);
  const composed = legacy(account, [memo("hi", [account])]);
  const notes: Record<string, AccountNote> = { [app.toBase58()]: { kind: "feePayer", replaceable: true, signs: "app" } };
  const verdict = compareComposed({ sent, composed, transaction: 0, notes, replaced: { [app.toBase58()]: account.toBase58() } });
  assert.equal(verdict.verdict, "same", verdict.detail);
  assert.match(verdict.detail, /pays the fee from/);

  // Not replaceable, it must stay.
  const kept: Record<string, AccountNote> = { [app.toBase58()]: { kind: "feePayer", replaceable: false, signs: "app" } };
  assert.equal(compareComposed({ sent, composed, transaction: 0, notes: kept }).verdict, "different");
});

test("the account acted as may have the wallet pay its fee from another key, with every instruction as written", () => {
  const wallet = key();
  const transfer = (payer: PublicKey) => legacy(payer, [SystemProgram.transfer({ fromPubkey: account, toPubkey: app, lamports: 5 }), memo("hi", [account])]);
  const sent = transfer(account);
  const composed = transfer(wallet);
  const verdict = compareComposed({ sent, composed, transaction: 0, account: account.toBase58() });
  assert.equal(verdict.verdict, "same", verdict.detail);
  assert.match(verdict.detail, new RegExp(`pays the fee from ${wallet.toBase58()}`));

  // Without knowing the account acted as, and with no note, the payer must stay.
  const unknown = compareComposed({ sent, composed, transaction: 0 });
  assert.equal(unknown.verdict, "different");
  assert.match(unknown.detail, /no note allows that/);

  // Another account paying its own fee is not the account acted as.
  const other = key();
  const theirs = legacy(other, [memo("hi", [other])]);
  assert.equal(compareComposed({ sent: theirs, composed: legacy(wallet, [memo("hi", [other])]), transaction: 0, account: account.toBase58() }).verdict, "different");
});

test("a new fee payer changes only the payer's place: instructions, signers and data stay", () => {
  const wallet = key();
  const sent = legacy(account, [SystemProgram.transfer({ fromPubkey: account, toPubkey: app, lamports: 5 })]);
  const at = { transaction: 0, account: account.toBase58() };

  // The wallet's payer written into an instruction.
  const into = legacy(wallet, [SystemProgram.transfer({ fromPubkey: wallet, toPubkey: app, lamports: 5 })]);
  assert.match(compareComposed({ ...at, sent, composed: into }).detail, /account 0 is/);

  // Another amount.
  const more = legacy(wallet, [SystemProgram.transfer({ fromPubkey: account, toPubkey: app, lamports: 6 })]);
  assert.match(compareComposed({ ...at, sent, composed: more }).detail, /data differs/);

  // The account no longer signs: it is still the one that gives.
  const unsigned = legacy(wallet, [
    new TransactionInstruction({
      programId: SystemProgram.programId,
      keys: [
        { pubkey: account, isSigner: false, isWritable: true },
        { pubkey: app, isSigner: false, isWritable: true },
      ],
      data: SystemProgram.transfer({ fromPubkey: account, toPubkey: app, lamports: 5 }).data,
    }),
  ]);
  assert.match(compareComposed({ ...at, sent, composed: unsigned }).detail, /no longer signs/);
});

test("a feePayer note that is replaceable lets the payer change too, and one that is not keeps it", () => {
  const sponsor = key();
  const wallet = key();
  const sent = legacy(sponsor, [memo("hi", [account])]);
  const composed = legacy(wallet, [memo("hi", [account])]);
  const replaceable: Record<string, AccountNote> = { [sponsor.toBase58()]: { kind: "feePayer", replaceable: true, signs: "app" } };
  const kept: Record<string, AccountNote> = { [sponsor.toBase58()]: { kind: "feePayer", replaceable: false, signs: "app" } };
  const replaced = { [sponsor.toBase58()]: wallet.toBase58() };
  assert.equal(compareComposed({ sent, composed, transaction: 0, account: account.toBase58(), notes: replaceable, replaced }).verdict, "same");
  assert.equal(compareComposed({ sent, composed, transaction: 0, account: account.toBase58(), notes: kept }).verdict, "different");
});

test("an address in a data entry is written over before comparing, and nowhere else", () => {
  const authority = key();
  const theirs = key();
  const mint = key();
  const notes: Record<string, AccountNote> = { [authority.toBase58()]: { kind: "new", replaceable: true } };
  const data: DataNote[] = [{ transaction: 0, instruction: 0, offset: 2, account: authority.toBase58() }];
  const sent = legacy(account, [initializeMint2(mint, 0, authority), memo("x", [authority])]);
  const composed = legacy(account, [initializeMint2(mint, 0, theirs), memo("x", [theirs])]);
  const replaced = { [authority.toBase58()]: theirs.toBase58() };
  assert.equal(compareComposed({ sent, composed, transaction: 0, notes, data, replaced }).verdict, "same");
  // Without the entry, the bytes in the data no longer match.
  assert.equal(compareComposed({ sent, composed, transaction: 0, notes, replaced }).verdict, "different");
  // An entry for another transaction of the request does not apply here.
  assert.equal(compareComposed({ sent, composed, transaction: 1, notes, data, replaced }).verdict, "different");
});

test("a data entry counts instructions as sent, compute budget included", () => {
  const authority = key();
  const theirs = key();
  const mint = key();
  const notes: Record<string, AccountNote> = { [authority.toBase58()]: { kind: "new", replaceable: true } };
  const data: DataNote[] = [{ transaction: 0, instruction: 1, offset: 2, account: authority.toBase58() }];
  const sent = legacy(account, withComputeBudget([initializeMint2(mint, 0, authority), memo("x", [authority])]));
  const composed = legacy(account, [initializeMint2(mint, 0, theirs), memo("x", [theirs])]);
  const verdict = compareComposed({ sent, composed, transaction: 0, notes, data, replaced: { [authority.toBase58()]: theirs.toBase58() } });
  assert.equal(verdict.verdict, "same", verdict.detail);
});

test("a bump of a re-derived account may change, at its entry only", () => {
  const seedOwner = key();
  const theirs = key();
  const program = key();
  const [pda] = PublicKey.findProgramAddressSync([seedOwner.toBytes()], program);
  const [theirPda] = PublicKey.findProgramAddressSync([theirs.toBytes()], program);
  const notes: Record<string, AccountNote> = {
    [seedOwner.toBase58()]: { kind: "new", replaceable: true },
    [pda.toBase58()]: { kind: "derived", pda: { program: program.toBase58(), seeds: [{ account: seedOwner.toBase58() }] } },
  };
  const ix = (owner: PublicKey, address: PublicKey, bump: number) =>
    new TransactionInstruction({
      programId: program,
      keys: [
        { pubkey: owner, isSigner: true, isWritable: true },
        { pubkey: address, isSigner: false, isWritable: true },
      ],
      data: Buffer.from([9, bump, 7]),
    });
  const sent = legacy(account, [ix(seedOwner, pda, 250)]);
  const composed = legacy(account, [ix(theirs, theirPda, 244)]);
  const replaced = { [seedOwner.toBase58()]: theirs.toBase58(), [pda.toBase58()]: theirPda.toBase58() };
  const data: DataNote[] = [{ transaction: 0, instruction: 0, offset: 1, bumpOf: pda.toBase58() }];
  assert.equal(compareComposed({ sent, composed, transaction: 0, notes, data, replaced }).verdict, "same");
  assert.equal(compareComposed({ sent, composed, transaction: 0, notes, replaced }).verdict, "different");
});

test("a v1 transaction composed from a legacy one is read and compared", () => {
  const sent = legacy(account, withComputeBudget([memo("hi", [account, app])]));
  const composed = asV1(legacy(account, [memo("hi", [account, app])]));
  const verdict = compareComposed({ sent, composed, transaction: 0 });
  assert.equal(verdict.verdict, "same", verdict.detail);
});

test("what this page cannot read it says it cannot check", () => {
  const sent = legacy(account, [memo("hi", [account])]);
  const carrier = compareComposed({ sent, composed: sent, transaction: 0, role: "carrier" });
  assert.equal(carrier.verdict, "cannot-check");
  assert.match(carrier.detail, /^cannot check: carrier/);
  assert.equal(compareComposed({ sent, composed: sent, transaction: 0, role: "proposal" }).verdict, "cannot-check");

  const table = new AddressLookupTableAccount({
    key: key(),
    state: { deactivationSlot: BigInt("18446744073709551615"), lastExtendedSlot: 0, lastExtendedSlotStartIndex: 0, addresses: [app] },
  });
  const message = new TransactionMessage({
    payerKey: account,
    recentBlockhash: BLOCKHASH_B,
    instructions: [SystemProgram.transfer({ fromPubkey: account, toPubkey: app, lamports: 1 })],
  }).compileToV0Message([table]);
  const v0 = new VersionedTransaction(message).serialize();
  assert.match(compareComposed({ sent, composed: v0, transaction: 0 }).detail, /lookup tables/);

  assert.equal(compareComposed({ sent, composed: new Uint8Array([1, 2]), transaction: 0 }).verdict, "different");
});
