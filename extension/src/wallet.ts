import type {
  IdentifierArray,
  Wallet,
  WalletAccount,
  WalletIcon,
} from "@wallet-standard/base";
import {
  SolanaSignTransaction,
  type SolanaSignTransactionInput,
  type SolanaSignTransactionOutput,
} from "@solana/wallet-standard-features";
import { ReadonlyWalletAccount } from "@wallet-standard/wallet";
import {
  StandardConnect,
  StandardDisconnect,
  StandardEvents,
  type StandardConnectInput,
  type StandardConnectOutput,
  type StandardEventsListeners,
} from "@wallet-standard/features";
import iconPng from "../../apps/airlock/icons/128x128.png";
import {
  BridgeError,
  bytesToBase64,
  base64ToBytes,
  encodeBase58,
  MAX_TRANSACTIONS,
  type AccountWire,
  type BridgeMethod,
} from "./protocol";

const CHAINS = ["solana:mainnet", "solana:devnet", "solana:testnet"] as const;
// Only what the desktop app can do is advertised, so a dApp never picks a
// feature that is certain to fail. The desktop signs legacy, v0 and v1.
const ACCOUNT_FEATURES = [SolanaSignTransaction] as const;
const VERSIONS = ["legacy", 0, 1] as const;

export const ICON = iconPng as WalletIcon;

type ChangeListener = StandardEventsListeners["change"];

export class AirlockWallet implements Wallet {
  readonly version = "1.0.0" as const;
  readonly name = "Airlock";
  readonly icon = ICON;
  readonly chains = CHAINS;

  #accounts: readonly WalletAccount[] = [];
  #listeners = new Set<ChangeListener>();
  readonly features: Wallet["features"];

  constructor(
    private readonly call: (method: BridgeMethod, params?: unknown) => Promise<unknown>,
  ) {
    this.features = {
      [StandardConnect]: {
        version: "1.0.0",
        connect: (input?: StandardConnectInput): Promise<StandardConnectOutput> => this.#connect(input),
      },
      [StandardDisconnect]: {
        version: "1.0.0",
        disconnect: (): Promise<void> => this.#disconnect(),
      },
      [StandardEvents]: {
        version: "1.0.0",
        on: (_event: "change", listener: ChangeListener): (() => void) => {
          this.#listeners.add(listener);
          return () => {
            this.#listeners.delete(listener);
          };
        },
      },
      [SolanaSignTransaction]: {
        version: "1.0.0",
        supportedTransactionVersions: VERSIONS,
        signTransaction: (
          ...inputs: readonly SolanaSignTransactionInput[]
        ): Promise<readonly SolanaSignTransactionOutput[]> => this.#signTransaction(...inputs),
      },
    };
  }

  get accounts(): readonly WalletAccount[] {
    return this.#accounts;
  }

  setAccounts(wires: readonly AccountWire[]): void {
    this.#accounts = wires.map(toAccount);
    this.#emit();
  }

  async #connect(input?: StandardConnectInput): Promise<StandardConnectOutput> {
    const silent = input?.silent === true;
    let result: unknown;
    try {
      result = await this.call("connect", { silent });
    } catch (err) {
      // A silent connect is a question, not a request: with the app closed the
      // answer is that nothing is connected, and the site shows as disconnected.
      if (silent) return { accounts: this.#accounts };
      throw err;
    }
    this.setAccounts(readAccounts(result));
    return { accounts: this.#accounts };
  }

  async #disconnect(): Promise<void> {
    const accounts = this.#accounts.map((account) => account.address);
    this.setAccounts([]);
    try {
      await this.call("disconnect", { accounts });
    } catch {
      // The site no longer sees accounts even if the desktop app is closed.
    }
  }

  async #signTransaction(
    ...inputs: readonly SolanaSignTransactionInput[]
  ): Promise<readonly SolanaSignTransactionOutput[]> {
    // Several inputs are one request: the desktop reviews them together and
    // answers with all of them signed, or not at all.
    if (inputs.length === 0) return [];
    if (inputs.length > MAX_TRANSACTIONS) {
      throw new BridgeError(
        "not_implemented",
        `Airlock signs at most ${MAX_TRANSACTIONS} transactions per request.`,
      );
    }
    inputs.forEach((input) => this.#requireAccount(input.account));
    const result = await this.call("signTransaction", {
      transactions: inputs.map((input) => ({
        account: input.account.address,
        transaction: bytesToBase64(input.transaction),
        chain: input.chain,
        options: input.options,
      })),
    });
    const signed = stringList(result, "signedTransactions");
    if (signed.length !== inputs.length) {
      throw new BridgeError("internal", "The desktop app returned the wrong number of transactions.");
    }
    return signed.map((item) => ({ signedTransaction: base64ToBytes(item) }));
  }

  #requireAccount(account: WalletAccount): void {
    if (!this.#accounts.some((known) => known.address === account.address)) {
      throw new BridgeError("rejected", "This account is not connected to the site.");
    }
  }

  #emit(): void {
    const properties = { accounts: this.#accounts };
    for (const listener of this.#listeners) listener(properties);
  }
}

function toAccount(wire: AccountWire): WalletAccount {
  const publicKey = base64ToBytes(wire.publicKey);
  if (publicKey.length !== 32) {
    throw new BridgeError("internal", "An account public key was not 32 bytes.");
  }
  return new ReadonlyWalletAccount({
    address: encodeBase58(publicKey),
    publicKey,
    chains: CHAINS as unknown as IdentifierArray,
    features: ACCOUNT_FEATURES as unknown as IdentifierArray,
    label: wire.label,
    icon: ICON,
  });
}

function readAccounts(value: unknown): AccountWire[] {
  const accounts = asRecord(value).accounts;
  if (!Array.isArray(accounts)) {
    throw new BridgeError("internal", "The desktop app did not return accounts.");
  }
  return accounts.map((account) => {
    const record = asRecord(account);
    return {
      publicKey: requiredString(record, "publicKey"),
      label: optionalString(record, "label"),
    };
  });
}

function stringList(value: unknown, field: string): string[] {
  const list = asRecord(value)[field];
  if (!Array.isArray(list) || list.some((item) => typeof item !== "string")) {
    throw new BridgeError("internal", `The desktop app result is missing ${field}.`);
  }
  return list as string[];
}

function asRecord(value: unknown): Record<string, unknown> {
  if (!value || typeof value !== "object") {
    throw new BridgeError("internal", "The desktop app returned an empty result.");
  }
  return value as Record<string, unknown>;
}

function requiredString(record: Record<string, unknown>, field: string): string {
  const value = record[field];
  if (typeof value !== "string" || value.length === 0) {
    throw new BridgeError("internal", `The desktop app result is missing ${field}.`);
  }
  return value;
}

function optionalString(record: Record<string, unknown>, field: string): string | undefined {
  const value = record[field];
  if (value === undefined) return undefined;
  if (typeof value !== "string") {
    throw new BridgeError("internal", `The desktop app result has a bad ${field}.`);
  }
  return value;
}
