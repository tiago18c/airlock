// A stand-in for a wallet app behind Mobile Wallet Adapter, for the MWA
// transport's tests: a `transact` with no association and no WebSocket. Each
// call to `transact` is a session; its wallet object turns camelCase names into
// snake_case JSON-RPC methods as the library's does, records every call with
// its session, and answers from handlers a test sets. It plays Airlock as far
// as a test needs: authorize, reauthorize with a token, get_capabilities, and
// whatever airlock_ method a test answers.

import { INTENT_FEATURES, type Resolution } from "../../../extension/src/intents";
import type { MwaWallet, Transact } from "../src/mwa";

export interface Call {
  session: number;
  method: string;
  params: Record<string, unknown>;
}

type Handler = (params: Record<string, unknown>, session: number) => unknown;

/** A JSON-RPC error as the library throws it, with `data` kept as build.mjs's patch keeps it. */
export function rpcError(code: number, message: string, data?: unknown): Error {
  return Object.assign(new Error(message), { name: "SolanaMobileWalletAdapterProtocolError", code, jsonRpcMessageId: 1, data });
}

/** What the library throws when the session's socket closes under a call. */
export function sessionClosed(): Error {
  return Object.assign(new Error("The wallet session closed before a response was received (1006: )."), {
    name: "SolanaMobileWalletAdapterError",
    code: "ERROR_SESSION_CLOSED",
  });
}

/** An Airlock refusal: a JSON-RPC error whose data is the bridge's code. The
 * number is one Airlock has not chosen yet, so the tests pick odd ones. */
export function refusal(code: string, message: string, requirement?: string, number = -32042): Error {
  return rpcError(number, message, requirement ? { code, requirement } : { code });
}

let clock = 1;

export function resolution(id: string, state: Resolution["state"], extra: Partial<Resolution> = {}): Resolution {
  clock += 1;
  return {
    id,
    account: "acct",
    chain: "solana:devnet",
    timing: "sync",
    delivery: "send",
    state,
    replaced: {},
    transactions: [],
    ran: [],
    updatedAt: clock,
    ...extra,
  };
}

export function base64(bytes: Uint8Array): string {
  return Buffer.from(bytes).toString("base64");
}

export class FakeMwa {
  readonly calls: Call[] = [];
  sessions = 0;
  readonly open = new Set<number>();
  /** What get_capabilities lists. */
  features: string[] = [...INTENT_FEATURES, "solana:signTransactions"];
  /** The accounts authorize gives: base64 public keys. */
  accounts: { address: string; label?: string }[];
  /** Tokens it takes, and the one it gives next. */
  readonly tokens = new Set<string>();
  #issued = 0;
  /** A reauthorize hands out a new token when set. */
  rotateTokens = false;
  readonly on: Record<string, Handler> = {};

  constructor(publicKeys: Uint8Array[]) {
    this.accounts = publicKeys.map((key, i) => ({ address: base64(key), label: `Key ${i + 1}` }));
    this.on.authorize = (params) => {
      const token = params.auth_token as string | undefined;
      if (token !== undefined) {
        if (!this.tokens.has(token)) throw rpcError(-1, "authorization request failed: the auth token is not valid");
        if (!this.rotateTokens) return { auth_token: token, accounts: this.accounts };
        this.tokens.delete(token);
      }
      const next = `token-${(this.#issued += 1)}`;
      this.tokens.add(next);
      return { auth_token: next, accounts: this.accounts };
    };
    this.on.get_capabilities = () => ({ max_transactions_per_request: 16, features: this.features });
  }

  /** The JSON-RPC methods of session `n`, in order. */
  methods(n: number): string[] {
    return this.calls.filter((call) => call.session === n).map((call) => call.method);
  }

  /** The calls of `method`, in order. */
  callsOf(method: string): Call[] {
    return this.calls.filter((call) => call.method === method);
  }

  readonly transact: Transact = async (callback) => {
    const n = (this.sessions += 1);
    this.open.add(n);
    let closed = false;
    const wallet = new Proxy({} as MwaWallet, {
      get: (_target, name) => {
        if (name === "then") return undefined;
        const method = String(name).replace(/[A-Z]/g, (letter) => `_${letter.toLowerCase()}`);
        return async (params?: Record<string, unknown>) => {
          if (closed) throw sessionClosed();
          this.calls.push({ session: n, method, params: params ?? {} });
          const handler = this.on[method];
          if (!handler) throw rpcError(-32601, `Method not found: ${method}`);
          return handler(params ?? {}, n);
        };
      },
    });
    try {
      return await callback(wallet);
    } finally {
      closed = true;
      this.open.delete(n);
    }
  };
}
