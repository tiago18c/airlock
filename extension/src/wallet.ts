import type {
  IdentifierArray,
  Wallet,
  WalletAccount,
  WalletIcon,
} from "@wallet-standard/base";
import {
  SignAndSendAllTransactions,
  SolanaSignAndSendTransaction,
  SolanaSignIn,
  SolanaSignMessage,
  SolanaSignTransaction,
  type SolanaSignAndSendAllTransactionsOptions,
  type SolanaSignAndSendTransactionInput,
  type SolanaSignAndSendTransactionOutput,
  type SolanaSignInInput,
  type SolanaSignInOutput,
  type SolanaSignMessageInput,
  type SolanaSignMessageOutput,
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
  CANCEL_RESOLUTION,
  GET_RESOLUTION,
  GET_SIGNING_PROFILE,
  INTENT_FEATURES,
  INTENT_FEATURE_VERSION,
  PROVIDE_SIGNATURES,
  RESOLVE,
  SIGN_MESSAGE,
  type AccountNote,
  type ProvideSignaturesInput,
  type Resolution,
  type ResolutionListener,
  type ResolveInput,
  type SignMessageInput,
  type SignMessageOutput,
  type SigningProfile,
} from "./intents";
import {
  BridgeError,
  bytesToBase64,
  base64ToBytes,
  decodeBase58,
  encodeBase58,
  MAX_TRANSACTIONS,
  type AccountWire,
  type BridgeMethod,
} from "./protocol";

const CHAINS = ["solana:mainnet", "solana:devnet", "solana:testnet"] as const;
// Only what the desktop app can do is advertised, so a dApp never picks a
// feature that is certain to fail. The desktop signs legacy, v0 and v1.
const ACCOUNT_FEATURES = [
  SolanaSignTransaction,
  SolanaSignAndSendTransaction,
  SignAndSendAllTransactions,
] as const;
// No key signs a message as a programmatic signer or a Squads vault. Every
// account takes the intent features: one with no key signs a message as a memo.
const KEY_ACCOUNT_FEATURES = [...ACCOUNT_FEATURES, SolanaSignMessage, ...INTENT_FEATURES] as const;
const KEYLESS_ACCOUNT_FEATURES = [...ACCOUNT_FEATURES, ...INTENT_FEATURES] as const;
const VERSIONS = ["legacy", 0, 1] as const;

/** How often a resolution waiting for the person is read again, in case its
 * push was missed. One id is read a turn, so a page waiting on several stays
 * well inside the extension's rate limit. */
export const POLL_MS = 2000;
/** How many resolutions are remembered, to tell an old reply from a newer push. */
const SEEN_LIMIT = 200;

export const ICON = iconPng as WalletIcon;

type ChangeListener = StandardEventsListeners["change"];

/** A refusal that came from a resolution's state: declined, expired or cancelled. */
export class ResolutionError extends BridgeError {
  readonly resolution: Resolution;

  constructor(code: string, message: string, resolution: Resolution) {
    super(code, message);
    this.resolution = resolution;
  }
}

interface Decision {
  resolve: (resolution: Resolution) => void;
  reject: (error: BridgeError) => void;
}

export class AirlockWallet implements Wallet {
  readonly version = "1.0.0" as const;
  readonly name = "Airlock";
  readonly icon = ICON;
  readonly chains = CHAINS;

  #accounts: readonly WalletAccount[] = [];
  #listeners = new Set<ChangeListener>();
  #resolutionListeners = new Set<ResolutionListener>();
  /** The latest of each resolution this page heard of, by id. */
  #seen = new Map<string, Resolution>();
  /** Who waits for the person to decide, by resolution id. */
  #deciding = new Map<string, Decision[]>();
  #pollTimer: ReturnType<typeof setTimeout> | null = null;
  #pollTurn = 0;
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
      [SolanaSignAndSendTransaction]: {
        version: "1.0.0",
        supportedTransactionVersions: VERSIONS,
        signAndSendTransaction: (
          ...inputs: readonly SolanaSignAndSendTransactionInput[]
        ): Promise<readonly SolanaSignAndSendTransactionOutput[]> => this.#signAndSend(...inputs),
      },
      [SignAndSendAllTransactions]: {
        version: "1.0.0",
        supportedTransactionVersions: VERSIONS,
        signAndSendAllTransactions: (
          inputs: readonly SolanaSignAndSendTransactionInput[],
          options?: SolanaSignAndSendAllTransactionsOptions,
        ): Promise<readonly PromiseSettledResult<SolanaSignAndSendTransactionOutput>[]> =>
          this.#signAndSendAll(inputs, options),
      },
      [SolanaSignMessage]: {
        version: "1.0.0",
        signMessage: (
          ...inputs: readonly SolanaSignMessageInput[]
        ): Promise<readonly SolanaSignMessageOutput[]> => this.#signMessage(...inputs),
      },
      // 1.0.0: version 1.1.0 promises offchain messages, which Airlock does not sign yet.
      [SolanaSignIn]: {
        version: "1.0.0",
        signIn: (...inputs: readonly SolanaSignInInput[]): Promise<readonly SolanaSignInOutput[]> =>
          this.#signIn(...inputs),
      },
      // The intents prototype (docs/intents.md), under Airlock's own names.
      [GET_SIGNING_PROFILE]: {
        version: INTENT_FEATURE_VERSION,
        getSigningProfile: (input: { account: WalletAccount }): Promise<SigningProfile> =>
          this.#getSigningProfile(input),
      },
      [RESOLVE]: {
        version: INTENT_FEATURE_VERSION,
        resolve: (input: ResolveInput): Promise<Resolution> => this.#resolve(input),
        onResolution: (listener: ResolutionListener): (() => void) => {
          this.#resolutionListeners.add(listener);
          return () => {
            this.#resolutionListeners.delete(listener);
          };
        },
      },
      [GET_RESOLUTION]: {
        version: INTENT_FEATURE_VERSION,
        getResolution: (input: { id: string }): Promise<Resolution> => this.#byId("getResolution", input),
      },
      [PROVIDE_SIGNATURES]: {
        version: INTENT_FEATURE_VERSION,
        provideSignatures: (input: ProvideSignaturesInput): Promise<Resolution> => this.#provideSignatures(input),
      },
      [CANCEL_RESOLUTION]: {
        version: INTENT_FEATURE_VERSION,
        cancelResolution: (input: { id: string }): Promise<Resolution> => this.#byId("cancelResolution", input),
      },
      [SIGN_MESSAGE]: {
        version: INTENT_FEATURE_VERSION,
        signMessage: (input: SignMessageInput): Promise<SignMessageOutput> => this.#resolveMessage(input),
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

  /** A message the app sent with no id, as the content script hands it on. */
  receivePush(push: unknown): void {
    if (!push || typeof push !== "object") return;
    const { method, params } = push as { method?: unknown; params?: Record<string, unknown> | null };
    if (method === "accountsChanged" && Array.isArray(params?.accounts)) {
      this.setAccounts(params.accounts as AccountWire[]);
      return;
    }
    if (method === "resolutionChanged" && isResolution(params?.resolution)) {
      const resolution = params.resolution;
      this.#learn(resolution);
      for (const listener of [...this.#resolutionListeners]) {
        try {
          listener(resolution);
        } catch {
          // One page listener that throws must not keep the others from hearing.
        }
      }
    }
  }

  async #getSigningProfile(input: { account: WalletAccount }): Promise<SigningProfile> {
    this.#requireAccount(input.account);
    const profile = asRecord(await this.call("getSigningProfile", { account: input.account.address })).profile;
    return asRecord(profile) as unknown as SigningProfile;
  }

  async #resolve(input: ResolveInput): Promise<Resolution> {
    this.#requireAccount(input.account);
    const count = Array.isArray(input.transactions) ? input.transactions.length : 0;
    if (count === 0) {
      throw new BridgeError("bad_request", "A request holds at least one transaction.");
    }
    if (count > MAX_TRANSACTIONS) {
      throw new BridgeError("not_implemented", `Airlock signs at most ${MAX_TRANSACTIONS} transactions per request.`);
    }
    const params = compact({
      id: input.id,
      account: input.account.address,
      chain: input.chain,
      transactions: input.transactions.map((transaction) => bytesToBase64(transaction)),
      order: input.order,
      delivery: input.delivery,
      accounts: input.accounts && notesToWire(input.accounts),
      data: input.data,
    });
    const resolution = readResolution(await this.call("resolve", params));
    this.#learn(resolution);
    return this.#decision(resolution);
  }

  async #byId(method: "getResolution" | "cancelResolution", input: { id: string }): Promise<Resolution> {
    if (typeof input?.id !== "string") throw new BridgeError("bad_request", "A resolution id is a string.");
    const resolution = readResolution(await this.call(method, { id: input.id }));
    this.#learn(resolution);
    return resolution;
  }

  async #provideSignatures(input: ProvideSignaturesInput): Promise<Resolution> {
    if (typeof input?.id !== "string" || !Array.isArray(input.signatures)) {
      throw new BridgeError("bad_request", "Give a resolution id and a list of signatures.");
    }
    const resolution = readResolution(
      await this.call("provideSignatures", {
        id: input.id,
        signatures: input.signatures.map((item) => ({
          transaction: item.transaction,
          signer: item.signer,
          signature: encodeBase58(item.signature),
        })),
      }),
    );
    this.#learn(resolution);
    return resolution;
  }

  async #resolveMessage(input: SignMessageInput): Promise<SignMessageOutput> {
    this.#requireAccount(input.account);
    const record = asRecord(
      await this.call(
        "resolveMessage",
        compact({
          id: input.id,
          account: input.account.address,
          message: bytesToBase64(input.message),
          mode: input.mode,
          chain: input.chain,
        }),
      ),
    );
    if (record.kind === "memo") {
      const resolution = readResolution(record);
      this.#learn(resolution);
      return { kind: "memo", resolution: await this.#decision(resolution) };
    }
    if (record.kind === "signature") {
      const signature = decodeBase58(requiredString(record, "signature"));
      if (!signature || signature.length !== 64) {
        throw new BridgeError("internal", "The desktop app returned a signature that is not 64 bytes.");
      }
      return {
        kind: "signature",
        signedMessage: base64ToBytes(requiredString(record, "signedMessage")),
        signature,
      };
    }
    throw new BridgeError("internal", "The desktop app answered a message with neither a signature nor a memo.");
  }

  /** Keep the latest of a resolution, and settle whoever waits on it once
   * the person has decided. A reply older than a push already heard is kept
   * out, so the state never goes back. */
  #learn(resolution: Resolution): void {
    const known = this.#seen.get(resolution.id);
    if (known && known.updatedAt > resolution.updatedAt) return;
    this.#seen.delete(resolution.id);
    this.#seen.set(resolution.id, resolution);
    if (this.#seen.size > SEEN_LIMIT) {
      const oldest = this.#seen.keys().next().value;
      if (oldest !== undefined) this.#seen.delete(oldest);
    }
    if (resolution.state === "awaiting-approval") return;
    const waiting = this.#deciding.get(resolution.id);
    if (!waiting) return;
    this.#deciding.delete(resolution.id);
    const outcome = decided(resolution);
    for (const decision of waiting) {
      if (outcome instanceof BridgeError) decision.reject(outcome);
      else decision.resolve(outcome);
    }
  }

  /** Settles when the resolution first leaves `awaiting-approval`. There is no
   * time limit here: the app expires the resolution itself. */
  #decision(resolution: Resolution): Promise<Resolution> {
    const latest = this.#seen.get(resolution.id) ?? resolution;
    if (latest.state !== "awaiting-approval") {
      const outcome = decided(latest);
      return outcome instanceof BridgeError ? Promise.reject(outcome) : Promise.resolve(outcome);
    }
    return new Promise((resolve, reject) => {
      const waiting = this.#deciding.get(resolution.id) ?? [];
      waiting.push({ resolve, reject });
      this.#deciding.set(resolution.id, waiting);
      this.#schedulePoll();
    });
  }

  #schedulePoll(): void {
    if (this.#pollTimer !== null || this.#deciding.size === 0) return;
    this.#pollTimer = setTimeout(() => {
      this.#pollTimer = null;
      void this.#poll();
    }, POLL_MS);
  }

  async #poll(): Promise<void> {
    const ids = [...this.#deciding.keys()];
    if (ids.length === 0) return;
    const id = ids[this.#pollTurn % ids.length];
    this.#pollTurn += 1;
    try {
      this.#learn(readResolution(await this.call("getResolution", { id })));
    } catch (err) {
      // The app no longer knows it, so nothing will ever be decided. Anything
      // else, such as a rate limit or a closed app, is tried again next turn.
      if (err instanceof BridgeError && err.code === "not_found") {
        const waiting = this.#deciding.get(id) ?? [];
        this.#deciding.delete(id);
        for (const decision of waiting) decision.reject(err);
      }
    } finally {
      this.#schedulePoll();
    }
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

  /** The transactions of a request for the desktop app. */
  #sendParams(inputs: readonly SolanaSignAndSendTransactionInput[]): { transactions: unknown[] } {
    if (inputs.length > MAX_TRANSACTIONS) {
      throw new BridgeError(
        "not_implemented",
        `Airlock signs at most ${MAX_TRANSACTIONS} transactions per request.`,
      );
    }
    inputs.forEach((input) => this.#requireAccount(input.account));
    return {
      transactions: inputs.map((input) => ({
        account: input.account.address,
        transaction: bytesToBase64(input.transaction),
        chain: input.chain,
        options: input.options,
      })),
    };
  }

  async #signAndSend(
    ...inputs: readonly SolanaSignAndSendTransactionInput[]
  ): Promise<readonly SolanaSignAndSendTransactionOutput[]> {
    if (inputs.length === 0) return [];
    const result = await this.call("signAndSendTransaction", this.#sendParams(inputs));
    const signatures = stringList(result, "signatures");
    if (signatures.length !== inputs.length) {
      throw new BridgeError("internal", "The desktop app returned the wrong number of signatures.");
    }
    return signatures.map((item) => ({ signature: base64ToBytes(item) }));
  }

  async #signAndSendAll(
    inputs: readonly SolanaSignAndSendTransactionInput[],
    options?: SolanaSignAndSendAllTransactionsOptions,
  ): Promise<readonly PromiseSettledResult<SolanaSignAndSendTransactionOutput>[]> {
    if (inputs.length === 0) return [];
    const result = await this.call("signAndSendAllTransactions", {
      ...this.#sendParams(inputs),
      mode: options?.mode,
    });
    const results = asRecord(result).results;
    if (!Array.isArray(results) || results.length !== inputs.length) {
      throw new BridgeError("internal", "The desktop app returned the wrong number of results.");
    }
    return results.map((item): PromiseSettledResult<SolanaSignAndSendTransactionOutput> => {
      const record = asRecord(item);
      if (record.error) {
        const error = asRecord(record.error);
        return {
          status: "rejected",
          reason: new BridgeError(
            requiredString(error, "code"),
            requiredString(error, "message"),
          ),
        };
      }
      return {
        status: "fulfilled",
        value: { signature: base64ToBytes(requiredString(record, "signature")) },
      };
    });
  }

  // One at a time: each is shown to the user on its own, and the site gets
  // all of them or an error.
  async #signMessage(
    ...inputs: readonly SolanaSignMessageInput[]
  ): Promise<readonly SolanaSignMessageOutput[]> {
    inputs.forEach((input) => this.#requireAccount(input.account));
    const outputs: SolanaSignMessageOutput[] = [];
    for (const input of inputs) {
      const record = asRecord(
        await this.call("signMessage", {
          account: input.account.address,
          message: bytesToBase64(input.message),
        }),
      );
      outputs.push({
        signedMessage: base64ToBytes(requiredString(record, "signedMessage")),
        signature: base64ToBytes(requiredString(record, "signature")),
        signatureType: "ed25519",
      });
    }
    return outputs;
  }

  async #signIn(...inputs: readonly SolanaSignInInput[]): Promise<readonly SolanaSignInOutput[]> {
    const outputs: SolanaSignInOutput[] = [];
    // A site that passes no input still asks to sign in.
    for (const input of inputs.length ? inputs : [{}]) {
      const record = asRecord(await this.call("signIn", input));
      this.setAccounts(readAccounts(record));
      const publicKey = requiredString(record, "account");
      const address = encodeBase58(base64ToBytes(publicKey));
      const account = this.#accounts.find((known) => known.address === address);
      if (!account) {
        throw new BridgeError("internal", "The desktop app signed in with an account it did not return.");
      }
      outputs.push({
        account,
        signedMessage: base64ToBytes(requiredString(record, "signedMessage")),
        signature: base64ToBytes(requiredString(record, "signature")),
        signatureType: "ed25519",
      });
    }
    return outputs;
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
    features: (wire.signsMessages ? KEY_ACCOUNT_FEATURES : KEYLESS_ACCOUNT_FEATURES) as unknown as IdentifierArray,
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
      signsMessages: record.signsMessages === true,
    };
  });
}

/** What a resolution's first state after `awaiting-approval` makes of the
 * request: the resolution, or the refusal the page gets. */
function decided(resolution: Resolution): Resolution | BridgeError {
  switch (resolution.state) {
    case "declined":
      return new ResolutionError("rejected", "The request was declined in Airlock.", resolution);
    case "expired":
      return new ResolutionError("expired", "Nobody decided in time, so the request expired.", resolution);
    case "cancelled":
      return new ResolutionError("cancelled", "The request was cancelled.", resolution);
    default:
      return resolution;
  }
}

function isResolution(value: unknown): value is Resolution {
  if (!value || typeof value !== "object") return false;
  const record = value as Record<string, unknown>;
  return typeof record.id === "string" && typeof record.state === "string";
}

/** The `resolution` of a reply, as the app wrote it. */
function readResolution(value: unknown): Resolution {
  const resolution = asRecord(value).resolution;
  if (!isResolution(resolution)) {
    throw new BridgeError("internal", "The desktop app did not return a resolution.");
  }
  return resolution;
}

/** Seed bytes go over the bridge as base64. */
function notesToWire(accounts: Record<string, AccountNote>): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  for (const [address, note] of Object.entries(accounts)) {
    if (note.kind === "derived" && "pda" in note) {
      out[address] = {
        ...note,
        pda: {
          ...note.pda,
          seeds: note.pda.seeds.map((seed) =>
            "bytes" in seed && seed.bytes instanceof Uint8Array ? { bytes: bytesToBase64(seed.bytes) } : seed,
          ),
        },
      };
    } else {
      out[address] = note;
    }
  }
  return out;
}

/** The object without its undefined fields, so they are left out rather than sent empty. */
function compact<T extends Record<string, unknown>>(value: T): Partial<T> {
  return Object.fromEntries(Object.entries(value).filter(([, item]) => item !== undefined)) as Partial<T>;
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
