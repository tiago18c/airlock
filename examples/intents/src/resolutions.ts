/** Every resolution this page started, kept by id so the table outlives a
 * reload. The wallet's pushes and the page's own reads both land here. */

import { ENDED, type Resolution, type ResolutionState } from "../../../extension/src/intents";
import { load, save } from "./storage";

export interface Tracked {
  id: string;
  /** The row that started it. */
  label: string;
  account: string;
  startedAt: number;
  resolution: Resolution | null;
  /** Why it never became a resolution, or the last read that failed. */
  error?: string;
  /** Set when the wallet answered `not_found`: it no longer has this id. */
  notFound?: string;
}

const KEY = "resolutions";
/** Enough for a long session; the oldest go first. */
const LIMIT = 100;
/** How often a resolution a row is waiting on is read again. One is read a
 * turn, and the wallet polls too, so the extension's rate limit is far off. */
const REFRESH_MS = 3000;

type Listener = (item: Tracked) => void;

interface Waiter {
  until: readonly ResolutionState[];
  resolve: (resolution: Resolution) => void;
  reject: (error: Error) => void;
}

/** What the page says when the wallet no longer has a resolution. */
export class NotFoundError extends Error {
  readonly code = "not_found";
}

function isNotFound(err: unknown): boolean {
  return !!err && typeof err === "object" && (err as { code?: unknown }).code === "not_found";
}

export function hasEnded(resolution: Resolution | null): boolean {
  return resolution !== null && ENDED.includes(resolution.state);
}

export class ResolutionStore {
  #items = new Map<string, Tracked>();
  #listeners = new Set<Listener>();
  #waiters = new Map<string, Waiter[]>();
  #timer: ReturnType<typeof setTimeout> | null = null;
  #turn = 0;
  /** How a resolution is read again. Set once a wallet with the features is chosen. */
  refresher: ((id: string) => Promise<Resolution>) | null = null;

  constructor() {
    const stored = load<Tracked[]>(KEY);
    if (Array.isArray(stored)) {
      for (const item of stored) {
        if (item && typeof item.id === "string") this.#items.set(item.id, item);
      }
    }
  }

  all(): Tracked[] {
    return [...this.#items.values()].sort((a, b) => b.startedAt - a.startedAt);
  }

  get(id: string): Tracked | undefined {
    return this.#items.get(id);
  }

  start(id: string, label: string, account: string): Tracked {
    const item: Tracked = { id, label, account, startedAt: Date.now(), resolution: null };
    this.#items.set(id, item);
    this.#changed(item);
    return item;
  }

  /** A resolution as the wallet gave it. One this page did not start is kept too:
   * it belongs to this site, from another tab or an earlier visit. */
  update(resolution: Resolution): void {
    const item =
      this.#items.get(resolution.id) ??
      ({ id: resolution.id, label: "Another page of this site", account: resolution.account, startedAt: Date.now(), resolution: null } as Tracked);
    // A read older than what a push already said is kept out.
    if (item.resolution && item.resolution.updatedAt > resolution.updatedAt) return;
    item.resolution = resolution;
    item.error = undefined;
    item.notFound = undefined;
    this.#items.set(item.id, item);
    this.#changed(item);
    const waiting = this.#waiters.get(item.id);
    if (!waiting) return;
    const left = waiting.filter((waiter) => {
      if (!waiter.until.includes(resolution.state) && !hasEnded(resolution)) return true;
      waiter.resolve(resolution);
      return false;
    });
    if (left.length) this.#waiters.set(item.id, left);
    else this.#waiters.delete(item.id);
  }

  fail(id: string, message: string): void {
    const item = this.#items.get(id);
    if (!item) return;
    item.error = message;
    this.#changed(item);
  }

  /** A read that failed. A `not_found` says the wallet no longer has this id,
   * so whoever waits on it is told, and the row shows it until forgotten. */
  readFailed(id: string, err: unknown, describe: (err: unknown) => string): void {
    if (!isNotFound(err)) {
      this.fail(id, describe(err));
      return;
    }
    const item = this.#items.get(id);
    if (item) {
      item.notFound = describe(err);
      item.error = undefined;
      this.#changed(item);
    }
    const waiting = this.#waiters.get(id) ?? [];
    this.#waiters.delete(id);
    for (const waiter of waiting) waiter.reject(new NotFoundError(`Airlock no longer has resolution ${id}.`));
  }

  remove(id: string): void {
    this.#items.delete(id);
    this.#persist();
    for (const listener of this.#listeners) listener({ id, label: "", account: "", startedAt: 0, resolution: null });
  }

  subscribe(listener: Listener): () => void {
    this.#listeners.add(listener);
    return () => this.#listeners.delete(listener);
  }

  /** Settles once the resolution is in one of `until`, or has ended. */
  waitFor(id: string, until: readonly ResolutionState[]): Promise<Resolution> {
    const current = this.#items.get(id)?.resolution ?? null;
    if (current && (until.includes(current.state) || hasEnded(current))) return Promise.resolve(current);
    return new Promise((resolve, reject) => {
      const waiting = this.#waiters.get(id) ?? [];
      waiting.push({ until, resolve, reject });
      this.#waiters.set(id, waiting);
      this.#schedule();
    });
  }

  /** Those that had not ended when last heard of, and that the wallet still has. */
  open(): Tracked[] {
    return this.all().filter((item) => item.resolution !== null && !hasEnded(item.resolution) && !item.notFound);
  }

  #schedule(): void {
    if (this.#timer !== null || this.#waiters.size === 0) return;
    this.#timer = setTimeout(() => {
      this.#timer = null;
      void this.#refreshOne();
    }, REFRESH_MS);
  }

  async #refreshOne(): Promise<void> {
    const ids = [...this.#waiters.keys()];
    if (ids.length > 0 && this.refresher) {
      const id = ids[this.#turn % ids.length];
      this.#turn += 1;
      try {
        this.update(await this.refresher(id));
      } catch (err) {
        // Anything but not_found is tried again next turn. The call itself is in the event log.
        if (isNotFound(err)) this.readFailed(id, err, (e) => (e instanceof Error ? e.message : String(e)));
      }
    }
    this.#schedule();
  }

  #changed(item: Tracked): void {
    this.#persist();
    for (const listener of this.#listeners) listener(item);
  }

  #persist(): void {
    save(KEY, this.all().slice(0, LIMIT));
  }
}
