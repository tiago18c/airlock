/** Messages shared by the page wallet, the service worker, and bridge-host. */

export const HOST_NAME = "dev.airlock.bridge";
export const PAGE_SOURCE = "airlock-page";
export const EXTENSION_SOURCE = "airlock-extension";

export const REQUEST_TTL_MS = 10 * 60 * 1000;

/** How long a request nobody has to decide may take. */
export const QUICK_TTL_MS = 15 * 1000;

/** The most transactions one signTransaction may carry, as the desktop app
 * allows (`bridge_protocol::MAX_TRANSACTIONS`). */
export const MAX_TRANSACTIONS = 16;

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

export type BridgeMethod = (typeof METHODS)[number];

export interface BridgeErrorBody {
  code: string;
  message: string;
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

/** Whether the desktop app asks the user before answering this. */
export function waitsForUser(method: string, params: unknown): boolean {
  return method !== "status" && method !== "disconnect" && !isSilentConnect(method, params);
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

export class BridgeError extends Error {
  readonly code: string;

  constructor(code: string, message: string) {
    super(message);
    this.code = code;
    this.name = code === "rejected" ? "WalletUserRejected" : "AirlockBridgeError";
  }
}
