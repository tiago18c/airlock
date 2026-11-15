/** A few DOM helpers. Text from a wallet or a node always goes in as text. */

type Child = Node | string | number | null | undefined | false;
type Attrs = Record<string, string | number | boolean | EventListener | undefined | null>;

export function h<K extends keyof HTMLElementTagNameMap>(tag: K, attrs: Attrs = {}, ...children: Child[]): HTMLElementTagNameMap[K] {
  const element = document.createElement(tag);
  for (const [name, value] of Object.entries(attrs)) {
    if (value === undefined || value === null || value === false) continue;
    if (name.startsWith("on") && typeof value === "function") {
      element.addEventListener(name.slice(2), value);
    } else if (name === "class") {
      element.className = String(value);
    } else if (value === true) {
      element.setAttribute(name, "");
    } else {
      element.setAttribute(name, String(value));
    }
  }
  append(element, ...children);
  return element;
}

export function append(parent: Node, ...children: Child[]): void {
  for (const child of children) {
    if (child === null || child === undefined || child === false) continue;
    parent.appendChild(typeof child === "string" || typeof child === "number" ? document.createTextNode(String(child)) : child);
  }
}

export function replace(parent: Node, ...children: Child[]): void {
  while (parent.firstChild) parent.removeChild(parent.firstChild);
  append(parent, ...children);
}

export type Tone = "ok" | "warn" | "danger" | "info" | "neutral";

export function chip(text: string, tone: Tone | "kind" | "selected" = "neutral"): HTMLSpanElement {
  return h("span", { class: `chip ${tone}` }, text);
}

export function short(address: string): string {
  return address.length > 12 ? `${address.slice(0, 4)}…${address.slice(-4)}` : address;
}

/** An address in mono, short, with the whole of it in its tooltip. */
export function address(value: string, name?: string): HTMLElement {
  return h(
    "span",
    { class: "address", title: value },
    name ? h("span", { class: "address-name" }, name) : null,
    h("code", {}, short(value)),
  );
}

export function link(href: string, text: string): HTMLAnchorElement {
  return h("a", { href, target: "_blank", rel: "noopener noreferrer" }, text);
}

export function copyButton(text: () => string, label = "Copy"): HTMLButtonElement {
  const button = h("button", { class: "sm", type: "button", title: "Copy to clipboard" }, label);
  button.addEventListener("click", async () => {
    try {
      await navigator.clipboard.writeText(text());
      button.textContent = "Copied";
    } catch {
      button.textContent = "Copy failed";
    }
    setTimeout(() => (button.textContent = label), 1200);
  });
  return button;
}

export function time(at: number = Date.now()): string {
  return new Date(at).toLocaleTimeString([], { hour12: false });
}

let toasts: HTMLElement | null = null;

/** A notification in the corner, as the wallet adapter's example shows results. */
export function toast(text: string, tone: Tone = "info", action?: { href: string; label: string }): void {
  toasts ??= document.getElementById("toasts");
  if (!toasts) return;
  const item = h(
    "div",
    { class: `toast ${tone}`, role: "status" },
    h("span", {}, text),
    action ? link(action.href, action.label) : null,
    h("button", { class: "ghost sm", type: "button", "aria-label": "Dismiss", onclick: () => item.remove() }, "Dismiss"),
  );
  toasts.appendChild(item);
  setTimeout(() => item.remove(), tone === "danger" ? 12_000 : 8_000);
}
