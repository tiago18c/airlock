import assert from "node:assert/strict";
import { test } from "node:test";
import type { Resolution } from "../../../extension/src/intents";
import { ResolutionStore } from "../src/resolutions";

function resolution(id: string, state: Resolution["state"], updatedAt = 1): Resolution {
  return { id, account: "acct", chain: "solana:devnet", timing: "sync", delivery: "send", state, replaced: {}, transactions: [], ran: [], updatedAt };
}

const describe = (err: unknown) => (err instanceof Error ? err.message : String(err));

test("a not_found marks the row, frees whoever waits on it, and leaves it out of the open ones", async () => {
  const store = new ResolutionStore();
  store.start("one", "Send a memo", "acct");
  store.update(resolution("one", "awaiting-signatures"));
  assert.deepEqual(store.open().map((item) => item.id), ["one"]);
  const waiting = store.waitFor("one", ["done"]);
  store.readFailed("one", Object.assign(new Error("No resolution of this site has this id."), { code: "not_found" }), describe);
  await assert.rejects(waiting, /no longer has resolution one/);
  assert.equal(store.get("one")?.notFound, "No resolution of this site has this id.");
  assert.deepEqual(store.open(), []);
  // Forgetting removes it.
  store.remove("one");
  assert.equal(store.get("one"), undefined);
});

test("another failed read is kept as an error, and a later answer clears both", () => {
  const store = new ResolutionStore();
  store.start("two", "Transfer SOL", "acct");
  store.readFailed("two", Object.assign(new Error("busy"), { code: "too_many_requests" }), describe);
  assert.equal(store.get("two")?.error, "busy");
  assert.equal(store.get("two")?.notFound, undefined);
  store.update(resolution("two", "done", 5));
  assert.equal(store.get("two")?.error, undefined);
});
