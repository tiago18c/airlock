/** Which network the page acts on, and the only node it talks to. */

import { Connection } from "@solana/web3.js";
import { RpcRefusal, type Rpc } from "./memo-check";
import { load, save } from "./storage";

export type Chain = "solana:devnet" | "solana:mainnet" | "solana:testnet";

export interface Network {
  kind: "devnet" | "mainnet" | "custom";
  url: string;
  chain: Chain;
}

export const DEVNET: Network = { kind: "devnet", url: "https://api.devnet.solana.com", chain: "solana:devnet" };
export const MAINNET: Network = { kind: "mainnet", url: "https://api.mainnet-beta.solana.com", chain: "solana:mainnet" };
export const LOCAL_URL = "http://127.0.0.1:8899";

const KEY = "network";

export function loadNetwork(): Network {
  const stored = load<Partial<Network>>(KEY);
  if (stored?.kind === "mainnet") return MAINNET;
  if (stored?.kind === "custom" && typeof stored.url === "string" && isChain(stored.chain)) {
    return { kind: "custom", url: stored.url, chain: stored.chain };
  }
  return DEVNET;
}

export function saveNetwork(network: Network): void {
  save(KEY, network);
}

export function isChain(value: unknown): value is Chain {
  return value === "solana:devnet" || value === "solana:mainnet" || value === "solana:testnet";
}

export function connectionFor(network: Network): Connection {
  // No websocket: the page reads statuses by asking, so it talks only to the URL chosen.
  return new Connection(network.url, { commitment: "confirmed", disableRetryOnRateLimit: true });
}

/** An address or a transaction on the Solana explorer, for this network. */
export function explorerUrl(network: Network, kind: "tx" | "address", value: string): string {
  const base = `https://explorer.solana.com/${kind}/${value}`;
  if (network.kind === "mainnet") return base;
  if (network.kind === "devnet") return `${base}?cluster=devnet`;
  return `${base}?cluster=custom&customUrl=${encodeURIComponent(network.url)}`;
}

/** Waits until the node has `signature` at confirmed, or says it failed. */
export async function confirm(connection: Connection, signature: string, timeoutMs = 60_000): Promise<void> {
  const started = Date.now();
  while (Date.now() - started < timeoutMs) {
    const { value } = await connection.getSignatureStatuses([signature]);
    const status = value[0];
    if (status?.err) throw new Error(`The transaction failed: ${JSON.stringify(status.err)}`);
    if (status && (status.confirmationStatus === "confirmed" || status.confirmationStatus === "finalized")) return;
    await sleep(1000);
  }
  throw new Error("The node has not confirmed it after a minute. It may still land: look it up on the explorer.");
}

/** Plain JSON-RPC to the node, so a v1 transaction reads too: web3.js 1.x
 * would refuse it. An error the node answers is an `RpcRefusal`, with the
 * `data` where a node says why ("Encoding does not support transaction
 * version 1" comes as `data` under "Internal error"). */
export function rpcFor(network: Network): Rpc {
  return async (method, params) => {
    const response = await fetch(network.url, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ jsonrpc: "2.0", id: 1, method, params }),
    });
    const reply = (await response.json()) as { result?: unknown; error?: { message?: string; data?: unknown } };
    if (reply.error) throw new RpcRefusal(reply.error.message ?? `The node refused ${method}.`, reply.error.data);
    return reply.result ?? null;
  };
}

export function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}
