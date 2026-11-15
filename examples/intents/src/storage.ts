/** What the page keeps in this browser. Storage can be missing or refused (a
 * private window, blocked site data), and the page works without it. */

const PREFIX = "airlock-intents.";

export function load<T>(key: string): T | null {
  try {
    const raw = localStorage.getItem(PREFIX + key);
    return raw === null ? null : (JSON.parse(raw) as T);
  } catch {
    return null;
  }
}

export function save(key: string, value: unknown): void {
  try {
    localStorage.setItem(PREFIX + key, JSON.stringify(value));
  } catch {
    // Kept for this visit only.
  }
}

export function forget(key: string): void {
  try {
    localStorage.removeItem(PREFIX + key);
  } catch {
    // Nothing was kept.
  }
}
