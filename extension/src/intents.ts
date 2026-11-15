/** The shapes of the intents prototype, as docs/intents.md gives them. On the
 * bridge an address or a signature is base58 and a transaction or a message is
 * base64; a resolution is plain JSON and passes through untouched. */

import type { WalletAccount } from "@wallet-standard/base";

export const GET_SIGNING_PROFILE = "airlock:getSigningProfile";
export const RESOLVE = "airlock:resolve";
export const GET_RESOLUTION = "airlock:getResolution";
export const PROVIDE_SIGNATURES = "airlock:provideSignatures";
export const CANCEL_RESOLUTION = "airlock:cancelResolution";
export const SIGN_MESSAGE = "airlock:signMessage";

/** Every account gets these: an account with no key still signs a message as a memo. */
export const INTENT_FEATURES = [
  GET_SIGNING_PROFILE,
  RESOLVE,
  GET_RESOLUTION,
  PROVIDE_SIGNATURES,
  CANCEL_RESOLUTION,
  SIGN_MESSAGE,
] as const;

export const INTENT_FEATURE_VERSION = "0.1.0";

export interface SigningProfile {
  account: string;
  timing: "sync" | "async";
  execution: "top-level" | "carrier";
  carrier?: string;
  feePayer: ("account" | "wallet" | "app")[];
  newAccounts: ("wallet-key" | "carrier" | "app-key")[];
  delivery: Delivery[];
  messages: MessageMode[];
  maxTransactions: number;
}

export type Delivery = "send" | "cosign" | "return";
export type Order = "atomic" | "serial" | "any";
export type MessageMode = "signature" | "memo";

/** Seed bytes are base64 on the bridge. A page may give them as bytes. */
export type Seed = { account: string } | { bytes: string | Uint8Array };

export type AccountNote =
  | { kind: "new"; replaceable: boolean }
  | { kind: "derived"; pda: { program: string; seeds: Seed[] } }
  | { kind: "derived"; withSeed: { base: string; seed: string; owner: string } }
  | { kind: "feePayer"; replaceable: boolean; signs?: "app" }
  | { kind: "signer"; signs: "app" };

export type DataNote =
  | { transaction: number; instruction: number; offset: number; account: string }
  | { transaction: number; instruction: number; offset: number; bumpOf: string };

/** What a page passes to `resolve`. */
export interface ResolveInput {
  id?: string;
  account: WalletAccount;
  chain: string;
  transactions: Uint8Array[];
  order?: Order;
  delivery?: Delivery;
  accounts?: Record<string, AccountNote>;
  data?: DataNote[];
}

export type ResolutionState =
  | "awaiting-approval"
  | "awaiting-signatures"
  | "awaiting-app"
  | "returned"
  | "sending"
  | "done"
  | "failed"
  | "unknown"
  | "declined"
  | "expired"
  | "cancelled";

export interface ResolutionTransaction {
  role: "direct" | "carrier" | "proposal" | "vote" | "execution";
  of: number[];
  state: "pending" | "landed" | "failed" | "unknown";
  signature?: string;
  error?: string;
  wire?: string;
}

export interface Resolution {
  id: string;
  account: string;
  chain: string;
  timing: "sync" | "async";
  delivery: Delivery;
  state: ResolutionState;
  replaced: Record<string, string>;
  transactions: ResolutionTransaction[];
  ran: { transaction: number; signature: string }[];
  /** In `awaiting-app` and `returned`: an index into `transactions`, and whose
   * signatures the app owes on it. */
  awaiting?: { transaction: number; signers: string[] }[];
  message?: { memo: string };
  /** `requirement` names what the account could not do, as in an `unsupported` refusal. */
  error?: { code: string; message: string; requirement?: string };
  updatedAt: number;
}

export interface ProvideSignaturesInput {
  id: string;
  /** `transaction` is an index into the resolution's `transactions`, as in `awaiting`. */
  signatures: { transaction: number; signer: string; signature: Uint8Array }[];
}

export interface SignMessageInput {
  account: WalletAccount;
  message: Uint8Array;
  mode?: MessageMode;
  /** The chain a memo is for, as in any request. With none, the network the wallet is on. */
  chain?: string;
  id?: string;
}

export type SignMessageOutput =
  | { kind: "signature"; signedMessage: Uint8Array; signature: Uint8Array }
  | { kind: "memo"; resolution: Resolution };

export type ResolutionListener = (resolution: Resolution) => void;

/** The features, as a page finds them on the wallet. */
export interface IntentFeatures {
  [GET_SIGNING_PROFILE]: {
    version: typeof INTENT_FEATURE_VERSION;
    getSigningProfile(input: { account: WalletAccount }): Promise<SigningProfile>;
  };
  [RESOLVE]: {
    version: typeof INTENT_FEATURE_VERSION;
    resolve(input: ResolveInput): Promise<Resolution>;
    onResolution(listener: ResolutionListener): () => void;
  };
  [GET_RESOLUTION]: {
    version: typeof INTENT_FEATURE_VERSION;
    getResolution(input: { id: string }): Promise<Resolution>;
  };
  [PROVIDE_SIGNATURES]: {
    version: typeof INTENT_FEATURE_VERSION;
    provideSignatures(input: ProvideSignaturesInput): Promise<Resolution>;
  };
  [CANCEL_RESOLUTION]: {
    version: typeof INTENT_FEATURE_VERSION;
    cancelResolution(input: { id: string }): Promise<Resolution>;
  };
  [SIGN_MESSAGE]: {
    version: typeof INTENT_FEATURE_VERSION;
    signMessage(input: SignMessageInput): Promise<SignMessageOutput>;
  };
}

/** The states a resolution ends in. `returned` ends only when the wallet
 * cannot watch for it, which a page cannot tell, so it is not listed. */
export const ENDED: readonly ResolutionState[] = ["done", "failed", "unknown", "declined", "expired", "cancelled"];
