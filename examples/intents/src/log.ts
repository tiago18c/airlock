/** Every call, reply, push and error, with its time, as text to copy. */

import { time } from "./ui";

export type Kind = "call" | "reply" | "push" | "error" | "info" | "relay";

export interface Entry {
  at: number;
  kind: Kind;
  text: string;
}

const LIMIT = 1000;

export class EventLog {
  readonly entries: Entry[] = [];
  #listeners = new Set<(entry: Entry | null) => void>();

  add(kind: Kind, text: string, detail?: unknown): void {
    const entry: Entry = { at: Date.now(), kind, text: detail === undefined ? text : `${text} ${stringify(detail)}` };
    this.entries.push(entry);
    if (this.entries.length > LIMIT) this.entries.shift();
    for (const listener of this.#listeners) listener(entry);
  }

  clear(): void {
    this.entries.length = 0;
    for (const listener of this.#listeners) listener(null);
  }

  /** Called with each new entry, and with null when the log is cleared. */
  subscribe(listener: (entry: Entry | null) => void): void {
    this.#listeners.add(listener);
  }

  text(): string {
    return this.entries.map(line).join("\n");
  }
}

export function line(entry: Entry): string {
  return `${time(entry.at)}  ${entry.kind.padEnd(5)}  ${entry.text}`;
}

/** JSON for the log, with bytes shown as their length rather than as a list of numbers. */
export function stringify(value: unknown): string {
  try {
    return JSON.stringify(value, (_key, item) => {
      if (item instanceof Uint8Array) return `<${item.length} bytes>`;
      if (typeof item === "bigint") return item.toString();
      if (item && typeof item === "object" && "address" in item && "publicKey" in item) return (item as { address: string }).address;
      return item;
    });
  } catch {
    return String(value);
  }
}
