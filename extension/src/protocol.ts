/** Messages shared by the page wallet, the service worker, and bridge-host.
 * The limits here state the desktop app's again, and its tests hold the two
 * together (`crates/app-desktop/src/bridge/extension_limits.rs`). */

export const HOST_NAME = "dev.airlock.bridge";
export const PAGE_SOURCE = "airlock-page";
export const EXTENSION_SOURCE = "airlock-extension";

/** How long the user has to decide, as the desktop app allows. */
export const REQUEST_TTL_MS = 10 * 60 * 1000;

/** How long a request nobody has to decide may take. */
export const QUICK_TTL_MS = 15 * 1000;

/** The most transactions one signTransaction may carry, as the desktop app
 * allows (`bridge_protocol::MAX_TRANSACTIONS`). */
export const MAX_TRANSACTIONS = 16;

/** The longest a send can take once approved: for each transaction, 30 seconds
 * for the node to take it, 90 for the commitment asked for, and 30 for the last
 * read of it (`RPC_TIMEOUT` and `COMMITMENT_TIMEOUT` in the desktop app). */
const SEND_MS = MAX_TRANSACTIONS * 150 * 1000;

/** The desktop app has the last word, so this only comes into it if it stops. */
const REPLY_GRACE_MS = 5 * 1000;

/** The most requests one site, and every site together, may have waiting for
 * the user, as the desktop app allows (`bridge_protocol::MAX_PENDING_PER_ORIGIN`
 * and `MAX_PENDING`). */
export const MAX_PENDING_PER_ORIGIN = 8;
export const MAX_PENDING = 32;

/** How much one site may ask: this many requests in each window. */
export const RATE_LIMIT = 30;
export const RATE_WINDOW_MS = 10 * 1000;

/** The most bytes of JSON a request may take. Sixteen of the largest
 * transactions take about 90 KiB. It has to stay under the 1 MiB the host reads
 * (`bridge_protocol::frame`), which ends on anything larger. */
export const MAX_REQUEST_BYTES = 256 * 1024;

/** What the desktop app answers today. Add a method here only with its desktop handler. */
export const METHODS = ["status", "connect", "disconnect", "signTransaction", "signAndSendTransaction", "signAndSendAllTransactions", "signMessage", "signIn"] as const;

/** The methods of the intents prototype (docs/intents.md), which the desktop
 * app answers only in a build that has its handlers. They are kept apart from
 * METHODS, which the app's tests hold to the handlers it ships. */
export const INTENT_METHODS = ["getSigningProfile", "resolve", "getResolution", "provideSignatures", "cancelResolution", "resolveMessage"] as const;

export type BridgeMethod = (typeof METHODS)[number] | (typeof INTENT_METHODS)[number];

/** What the app sends with no id, for every page of one origin. */
export const PUSH_METHODS = ["accountsChanged", "resolutionChanged"] as const;

export function isBridgeMethod(value: unknown): value is BridgeMethod {
  return (
    (METHODS as readonly unknown[]).includes(value) || (INTENT_METHODS as readonly unknown[]).includes(value)
  );
}

export function isPushMethod(value: unknown): value is (typeof PUSH_METHODS)[number] {
  return (PUSH_METHODS as readonly unknown[]).includes(value);
}

export interface BridgeErrorBody {
  code: string;
  message: string;
  /** What an account could not do, beside an `unsupported` refusal. */
  requirement?: string;
}

export interface NativeRequest {
  v: 1;
  id: string;
  origin: string;
  method: string;
  params?: unknown;
  expiresAt: number;
}

export interface NativeMessage {
  v?: number;
  id?: string;
  method?: string;
  /** The site a push from the app is for. */
  origin?: string;
  params?: unknown;
  result?: unknown;
  error?: BridgeErrorBody;
}

export interface AccountWire {
  publicKey: string;
  label?: string;
  /** A key on the desktop can sign a message as this account. */
  signsMessages?: boolean;
}

export interface BridgeState {
  host: "connected" | "down" | "missing";
  role: string | null;
  lastError: string | null;
  pending: PendingRequest[];
}

export interface PendingRequest {
  id: string;
  shortCode: string;
  origin: string;
  method: string;
}

export function shortCode(id: string): string {
  return id.replace(/-/g, "").slice(0, 4).toUpperCase();
}

export function isRequestId(id: unknown): id is string {
  return typeof id === "string" && /^[A-Za-z0-9-]{1,80}$/.test(id);
}

/** A connect that may only ask whether the site is connected already. The
 * desktop app reads `silent` the same way. */
function isSilentConnect(method: unknown, params: unknown): boolean {
  return method === "connect" && (params as { silent?: unknown } | null | undefined)?.silent === true;
}

/** Whether this can put a modal in front of the user from a site with no
 * connection. A page has to have been used before it may do that. */
export function needsUserGesture(method: unknown, params: unknown): boolean {
  return method === "signIn" || (method === "connect" && !isSilentConnect(method, params));
}

/** The intent methods the app answers at once. What the person decides about a
 * resolution arrives later, as a push. */
const ANSWERED_AT_ONCE: readonly string[] = ["getSigningProfile", "resolve", "getResolution", "provideSignatures", "cancelResolution"];

/** Whether the desktop app asks the user before answering this. A
 * `resolveMessage` may be a signature, which waits as `signMessage` does. */
export function waitsForUser(method: string, params: unknown): boolean {
  return (
    method !== "status" &&
    method !== "disconnect" &&
    !ANSWERED_AT_ONCE.includes(method) &&
    !isSilentConnect(method, params)
  );
}

/** How long to wait for the desktop app's reply before giving up on it. A send
 * can go on long after the user approved it, and a site told it had expired
 * could send it all again. */
export function replyWindowMs(method: string, params: unknown): number {
  if (!waitsForUser(method, params)) return QUICK_TTL_MS;
  const sends = method === "signAndSendTransaction" || method === "signAndSendAllTransactions";
  return REQUEST_TTL_MS + (sends ? SEND_MS : 0) + REPLY_GRACE_MS;
}

/** The bytes `value` takes as JSON. What cannot be written as JSON has no size
 * that fits. */
export function jsonBytes(value: unknown): number {
  try {
    return new TextEncoder().encode(JSON.stringify(value) ?? "").length;
  } catch {
    return Infinity;
  }
}

/** Request ids for the page↔extension bridge. Prefer crypto.randomUUID when the
 * page left Web Crypto intact; fall back when dApps polyfill `crypto` without
 * randomUUID, or on non-secure http origins where randomUUID is missing. */
export function newRequestId(): string {
  const c = globalThis.crypto;
  if (c && typeof c.randomUUID === "function") {
    return c.randomUUID();
  }
  if (c && typeof c.getRandomValues === "function") {
    const bytes = c.getRandomValues(new Uint8Array(16));
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    const hex = [...bytes].map((b) => b.toString(16).padStart(2, "0")).join("");
    return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
  }
  return `airlock-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 10)}`;
}

export function bytesToBase64(bytes: Uint8Array): string {
  let binary = "";
  const chunk = 0x8000;
  for (let i = 0; i < bytes.length; i += chunk) {
    binary += String.fromCharCode(...bytes.subarray(i, i + chunk));
  }
  return btoa(binary);
}

export function base64ToBytes(value: string): Uint8Array {
  const binary = atob(value);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i += 1) {
    bytes[i] = binary.charCodeAt(i);
  }
  return bytes;
}

const BASE58 = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";

export function encodeBase58(bytes: Uint8Array): string {
  let zeros = 0;
  while (zeros < bytes.length && bytes[zeros] === 0) zeros += 1;
  const digits: number[] = [];
  for (let i = zeros; i < bytes.length; i += 1) {
    let carry = bytes[i];
    for (let j = 0; j < digits.length; j += 1) {
      carry += digits[j] << 8;
      digits[j] = carry % 58;
      carry = (carry / 58) | 0;
    }
    while (carry > 0) {
      digits.push(carry % 58);
      carry = (carry / 58) | 0;
    }
  }
  let out = "1".repeat(zeros);
  for (let i = digits.length - 1; i >= 0; i -= 1) out += BASE58[digits[i]];
  return out;
}

/** The bytes of a base58 string, or null when it is not base58. */
export function decodeBase58(value: string): Uint8Array | null {
  let zeros = 0;
  while (zeros < value.length && value[zeros] === "1") zeros += 1;
  const bytes: number[] = [];
  for (let i = zeros; i < value.length; i += 1) {
    let carry = BASE58.indexOf(value[i]);
    if (carry < 0) return null;
    for (let j = 0; j < bytes.length; j += 1) {
      carry += bytes[j] * 58;
      bytes[j] = carry & 0xff;
      carry >>= 8;
    }
    while (carry > 0) {
      bytes.push(carry & 0xff);
      carry >>= 8;
    }
  }
  const out = new Uint8Array(zeros + bytes.length);
  for (let i = 0; i < bytes.length; i += 1) out[out.length - 1 - i] = bytes[i];
  return out;
}

export class BridgeError extends Error {
  readonly code: string;
  /** For `unsupported`: what the account could not do. */
  readonly requirement?: string;

  constructor(code: string, message: string, requirement?: string) {
    super(message);
    this.code = code;
    if (requirement !== undefined) this.requirement = requirement;
    this.name = code === "rejected" ? "WalletUserRejected" : "AirlockBridgeError";
  }

  /** The error a reply's error body stands for. */
  static from(body: { code?: unknown; message?: unknown; requirement?: unknown } | null | undefined): BridgeError {
    return new BridgeError(
      typeof body?.code === "string" ? body.code : "internal",
      typeof body?.message === "string" ? body.message : "The request failed.",
      typeof body?.requirement === "string" ? body.requirement : undefined,
    );
  }
}
