/** The mint row's Token-2022 request, apart from the page so it can be
 * tested: a mint whose metadata pointer names the mint itself, its token
 * account, and the notes that let the wallet swap the mint's key. */

import type { PublicKey, TransactionInstruction } from "@solana/web3.js";
import type { AccountNote, DataNote } from "../../../extension/src/intents";
import {
  ATA_PROGRAM,
  METADATA_ADDRESS_OFFSET,
  MINT_WITH_POINTER_SIZE,
  TOKEN_2022_PROGRAM,
  associatedTokenAddress,
  createAssociatedTokenAccount,
  createMintAccount,
  initializeMetadataPointer,
  initializeMint2,
  mintTo,
} from "./instructions";

export interface Token2022Request {
  instructions: TransactionInstruction[];
  ata: PublicKey;
  accounts: Record<string, AccountNote>;
  data: DataNote[];
}

/** Where InitializeMetadataPointer sits in the transaction, which is what its
 * `data` entry counts by. */
export const POINTER_INSTRUCTION = 1;

export function token2022MintRequest(owner: PublicKey, mint: PublicKey, lamports: number): Token2022Request {
  const [ata] = associatedTokenAddress(owner, mint, TOKEN_2022_PROGRAM);
  const instructions = [
    createMintAccount(owner, mint, lamports, MINT_WITH_POINTER_SIZE, TOKEN_2022_PROGRAM),
    initializeMetadataPointer(mint, owner, mint),
    initializeMint2(mint, 0, owner, TOKEN_2022_PROGRAM),
    createAssociatedTokenAccount(owner, ata, owner, mint, TOKEN_2022_PROGRAM),
    mintTo(mint, ata, owner, 1000n, TOKEN_2022_PROGRAM),
  ];
  const mintAddress = mint.toBase58();
  return {
    instructions,
    ata,
    accounts: {
      [mintAddress]: { kind: "new", replaceable: true },
      [ata.toBase58()]: {
        kind: "derived",
        pda: {
          program: ATA_PROGRAM.toBase58(),
          seeds: [{ account: owner.toBase58() }, { bytes: TOKEN_2022_PROGRAM.toBytes() }, { account: mintAddress }],
        },
      },
    },
    // The mint's own address is in the pointer's data, so a wallet that puts
    // its key in the mint's place must write it there too.
    data: [{ transaction: 0, instruction: POINTER_INSTRUCTION, offset: METADATA_ADDRESS_OFFSET, account: mintAddress }],
  };
}
