/** Messages shared by the page wallet, the service worker, and bridge-host. */

export const HOST_NAME = "dev.airlock.bridge";
export const PAGE_SOURCE = "airlock-page";
export const EXTENSION_SOURCE = "airlock-extension";

export const REQUEST_TTL_MS = 10 * 60 * 1000;

/** The most transactions one signTransaction may carry, as the desktop app
 * allows (`bridge_protocol::MAX_TRANSACTIONS`). */
export const MAX_TRANSACTIONS = 16;

/** What the desktop app answers today. Add a method here only with its desktop handler. */
export const METHODS = ["status", "connect", "disconnect", "signTransaction"] as const;

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
  params?: unknown;
  result?: unknown;
  error?: BridgeErrorBody;
}

export interface AccountWire {
  publicKey: string;
  label?: string;
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
