// Helpers for driving Airlock and Chrome on an Android emulator from this
// machine: adb, starting the emulator, the app's WebView and Chrome's page
// over the DevTools protocol, taps, and Android's own screens through
// uiautomator. scripts/drive-intents-android.mjs uses these.
//
// The app's page scripts are the desktop ones of drive-lib.mjs (`addWallet`,
// `setCustomRpc`, `createMultisig`, the approver...): a phone page here runs
// them with the phone's `nav`, which finds a section on the bottom bar or in
// the sheet More opens.
import { spawnSync } from "node:child_process";
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { PAGE_HELPERS, sleep } from "./drive-lib.mjs";

export const PKG = "dev.airlock.mobile";
export const ACTIVITY = `${PKG}/dev.dioxus.main.MainActivity`;
export const CHROME = "com.android.chrome";

// ---- adb ----

/** Runs adb against `serial`. Returns stdout, trimmed, and stderr too when it failed. */
export const adb = (serial, ...args) => {
  const r = spawnSync("adb", ["-s", serial, ...args], { encoding: "utf8", maxBuffer: 1 << 26, env: { ...process.env, MSYS_NO_PATHCONV: "1" } });
  return ((r.stdout ?? "") + (r.status === 0 ? "" : r.stderr ?? "")).trim();
};
export const shell = (serial, command) => adb(serial, "shell", command);
/** A command as the app's own user, in its private folder. Needs a debug build. */
export const asApp = (serial, command) => shell(serial, `run-as ${PKG} sh -c '${command}'`);
/** Writes bytes to a file of the app's private folder. */
export const pushPrivate = (serial, path, bytes) => {
  const r = spawnSync("adb", ["-s", serial, "shell", "-T", `run-as ${PKG} sh -c 'cat > ${path}'`], { input: bytes });
  if (r.status !== 0) throw new Error(`writing ${path}: ${r.stderr}`);
};
/** The screen as a person sees it, system bars included. */
export const screencap = (serial, file) => {
  const r = spawnSync("adb", ["-s", serial, "exec-out", "screencap", "-p"], { maxBuffer: 1 << 28, timeout: 30_000 });
  if (r.status !== 0 || !r.stdout || r.stdout.length < 1000) return false;
  writeFileSync(file, r.stdout);
  return true;
};
/** A real tap at device pixels (`x`, `y`). */
export const tapAt = (serial, x, y) => shell(serial, `input tap ${Math.round(x)} ${Math.round(y)}`);

// ---- the emulator ----

const deviceState = (serial) => spawnSync("adb", ["-s", serial, "get-state"], { encoding: "utf8" }).stdout?.trim();
export const booted = (serial) => deviceState(serial) === "device" && shell(serial, "getprop sys.boot_completed") === "1";

/** Starts AVD `avd` as `serial` (emulator-<port>) unless it is up, with no
 * window, and waits for Android. Through PowerShell's Start-Process, so the
 * emulator is nobody's child and outlives this process. Answers whether it
 * started it. */
export const ensureEmulator = async ({ serial, avd, log, logFile }) => {
  if (booted(serial)) return false;
  const port = serial.match(/^emulator-(\d+)$/)?.[1];
  if (!port) throw new Error(`${serial} is not connected, and only an emulator can be started`);
  if (deviceState(serial) !== "device" && deviceState(serial) !== "offline") {
    const sdk = process.env.ANDROID_HOME ?? join(process.env.LOCALAPPDATA ?? "", "Android", "Sdk");
    const quote = (x) => `'${String(x).replace(/'/g, "''")}'`;
    const args = ["-avd", avd, "-port", port, "-no-audio", "-no-boot-anim", "-no-window"];
    spawnSync("powershell", ["-NoProfile", "-NonInteractive", "-Command",
      `Start-Process -FilePath ${quote(join(sdk, "emulator", "emulator.exe"))} -ArgumentList ${args.map(quote).join(",")} -WindowStyle Hidden -RedirectStandardOutput ${quote(logFile)} -RedirectStandardError ${quote(`${logFile}.err`)}`,
    ], { stdio: "ignore" });
    log(`starting the emulator ${avd} as ${serial}`);
  }
  for (let i = 0; i < 240 && !booted(serial); i += 1) await sleep(1000);
  if (!booted(serial)) throw new Error(`${serial} did not boot in 4 minutes`);
  await sleep(3000);
  return true;
};

/** Awake, unlocked, portrait, no animations, no soft keyboard. */
export const prepareDevice = (serial) => {
  for (const command of [
    "settings put global window_animation_scale 0",
    "settings put global transition_animation_scale 0",
    "settings put global animator_duration_scale 0",
    "settings put system accelerometer_rotation 0",
    "settings put system user_rotation 0",
    "settings put secure show_ime_with_hard_keyboard 0",
    "svc power stayon true",
    "input keyevent KEYCODE_WAKEUP",
    "wm dismiss-keyguard",
  ]) shell(serial, command);
};

// ---- uiautomator: Android's own screens ----

/** Every node of the screen uiautomator sees: `{ text, desc, id, cls, pkg, bounds: [x1, y1, x2, y2] }`. */
export const uiNodes = (serial) => {
  shell(serial, "uiautomator dump /sdcard/window.xml");
  const xml = adb(serial, "exec-out", "cat /sdcard/window.xml");
  const nodes = [];
  for (const match of xml.matchAll(/<node [^>]*>/g)) {
    const attr = (name) => match[0].match(new RegExp(`${name}="([^"]*)"`))?.[1] ?? "";
    const b = attr("bounds").match(/\[(\d+),(\d+)\]\[(\d+),(\d+)\]/);
    nodes.push({ text: attr("text"), desc: attr("content-desc"), id: attr("resource-id"), cls: attr("class"), pkg: attr("package"), bounds: b ? b.slice(1).map(Number) : null });
  }
  return nodes;
};
export const tapNode = (serial, node) => tapAt(serial, (node.bounds[0] + node.bounds[2]) / 2, (node.bounds[1] + node.bounds[3]) / 2);

/** The app in front now (its package). */
export const foreground = (serial) => {
  const line = shell(serial, "dumpsys activity activities").split(/\r?\n/).find((l) => /topResumedActivity|mResumedActivity/.test(l)) ?? "";
  return line.match(/ ([a-zA-Z0-9_.]+)\/[a-zA-Z0-9_.$]+/)?.[1] ?? null;
};

// ---- the app ----

/** Starts the app (or brings it forward) and forwards its WebView's DevTools
 * socket to `port`. Answers its pid. */
export const startApp = async (serial, port) => {
  shell(serial, `am start -n ${ACTIVITY}`);
  let pid = "";
  for (let i = 0; i < 120; i += 1) {
    pid = shell(serial, `pidof ${PKG}`);
    if (pid && shell(serial, "cat /proc/net/unix").includes(`webview_devtools_remote_${pid}`)) break;
    await sleep(250);
  }
  if (!pid) throw new Error("the app did not start");
  adb(serial, "forward", "--remove", `tcp:${port}`);
  adb(serial, "forward", `tcp:${port}`, `localabstract:webview_devtools_remote_${pid}`);
  return Number(pid);
};
export const bringAppForward = (serial) => shell(serial, `am start -n ${ACTIVITY}`);
export const bringChromeForward = (serial) => shell(serial, `am start -a android.intent.action.MAIN -c android.intent.category.LAUNCHER -n ${CHROME}/com.google.android.apps.chrome.Main`);

/** The phone's sections: four on the bottom bar (with short labels), the
 * rest in the sheet More opens. `nav('Settings').click()` does both steps. */
const PHONE_NAV = `
  const nav = (label) => {
    const short = { 'Review and Sign': 'Sign', 'Address Book': 'Contacts' }[label] ?? label;
    const items = () => [...document.querySelectorAll('nav .nav-item')];
    const tab = () => items().find((b) => [label, short].includes(b.innerText.trim()));
    const entry = () => [...document.querySelectorAll('.more-sheet .more-item')].find((b) => b.innerText.trim() === label);
    return {
      click() {
        const t = tab();
        if (t) { document.querySelector('.more-backdrop, .phone-scrim')?.click(); t.click(); return; }
        if (!entry()) items().find((b) => b.innerText.trim() === 'More')?.click();
        (async () => { for (let i = 0; i < 40 && !entry(); i++) await w(50); entry()?.click(); })();
      },
    };
  };
`;

/** A DevTools connection to the first page target on `port`, with every
 * call given up after `ms` (an app behind another may not answer until it is
 * in front again). */
const devtools = async (port, pick, { tries = 120 } = {}) => {
  let target;
  for (let i = 0; i < tries && !target; i += 1) {
    try {
      target = (await (await fetch(`http://127.0.0.1:${port}/json`)).json()).find(pick);
    } catch {}
    if (!target) await sleep(250);
  }
  if (!target) throw new Error(`no page on port ${port}`);
  const ws = new WebSocket(target.webSocketDebuggerUrl);
  await new Promise((done, fail) => {
    ws.onopen = done;
    ws.onerror = () => fail(new Error(`could not open the page on port ${port}`));
  });
  let nextId = 0;
  const pending = new Map();
  const listeners = [];
  let closed = false;
  ws.onmessage = (m) => {
    const msg = JSON.parse(m.data);
    if (msg.id && pending.has(msg.id)) {
      pending.get(msg.id)(msg);
      pending.delete(msg.id);
      return;
    }
    for (const listener of listeners) listener(msg);
  };
  ws.onclose = () => {
    closed = true;
    for (const done of pending.values()) done({ error: { message: "the page closed" } });
    pending.clear();
  };
  const send = (method, params = {}, ms = 30_000) => new Promise((done) => {
    if (closed) return done({ error: { message: "the page closed" } });
    const id = ++nextId;
    const timer = setTimeout(() => {
      pending.delete(id);
      done({ error: { message: `${method}: no answer in ${ms / 1000} s` }, timedOut: true });
    }, ms);
    pending.set(id, (msg) => {
      clearTimeout(timer);
      done(msg);
    });
    ws.send(JSON.stringify({ id, method, params }));
  });
  return { target, send, on: (listener) => listeners.push(listener), close: () => ws.close(), closed: () => closed };
};

/** Airlock's page on the phone, as drive-lib.mjs's `openPage` gives the
 * desktop one: `run(body)` runs `body` with `PAGE_HELPERS` and the phone's
 * `nav` in scope. A screenshot is the whole screen (`adb screencap`): a
 * WebView cannot be made taller than the screen, and the modal is a sheet. */
export const openAppPage = async (serial, port) => {
  const cdp = await devtools(port, (t) => t.type === "page");
  const evaluate = async (expression, ms = 15_000) => {
    const res = await cdp.send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true }, ms);
    if (res.result?.exceptionDetails) console.log("app eval error:", JSON.stringify(res.result.exceptionDetails).slice(0, 400));
    return res.result?.result?.value;
  };
  await cdp.send("Page.enable");
  await cdp.send("Runtime.enable");
  for (let i = 0; i < 80; i += 1) {
    if (await evaluate(`!!document.querySelector('button')`, 5000)) break;
    await sleep(250);
  }
  const run = (body, ms = 240_000) => evaluate(`(async () => { ${PAGE_HELPERS} return await (async () => { ${PHONE_NAV} ${body} })(); })()`, ms);
  const shot = async (path) => screencap(serial, path);
  return { send: cdp.send, evaluate, run, shotModal: shot, shotPage: shot, close: cdp.close, closed: cdp.closed };
};

/** Make the wallet labelled `label` current, in the phone's switcher: its
 * button on the top bar opens a sheet of wallets. Answers "ok" or what went
 * wrong. */
export const selectPhoneWallet = (page, label) => page.run(`
  const open = document.querySelector('.wallet-switch-btn');
  if (!open) return 'no wallet switcher';
  open.click();
  const row = await until(() => [...document.querySelectorAll('.wallet-sheet-pick')].find((b) => b.querySelector('.addr-label')?.innerText.trim() === ${JSON.stringify(label)}), 5000);
  if (!row) return 'no wallet ' + ${JSON.stringify(label)} + ' in the sheet';
  row.click();
  await w(600);
  return document.querySelector('.wallet-switch-btn .addr-label')?.innerText.trim() === ${JSON.stringify(label)} ? 'ok' : 'not selected: ' + document.querySelector('.wallet-switch-btn')?.innerText;
`);

/** Settings, "Apps on this phone": On. Pressed even when it reads On, since
 * that is what lists Airlock as a wallet. Answers "ok" or what went wrong. */
export const allowPhoneApps = (page) => page.run(`
  nav('Settings').click();
  const card = await until(() => [...document.querySelectorAll('.card')].find((c) => c.querySelector('h3')?.innerText.trim() === 'Apps on this phone'));
  if (!card) return 'no Apps on this phone in Settings: ' + buttons();
  const on = [...card.querySelectorAll('button')].find((b) => b.innerText.trim() === 'On');
  on.click();
  await w(600);
  return on.classList.contains('on') ? 'ok' : 'not on: ' + card.innerText;
`);

// ---- Chrome ----

/** Chrome's page at `url` on the phone, over Chrome's DevTools socket
 * forwarded to `port`. Other tabs of the same origin are closed first, so
 * only one page talks to the wallet. */
export const openChromePage = async (serial, port, url) => {
  adb(serial, "forward", "--remove", `tcp:${port}`);
  adb(serial, "forward", `tcp:${port}`, "localabstract:chrome_devtools_remote");
  const origin = new URL(url).origin;
  bringChromeForward(serial);
  let targets = [];
  for (let i = 0; i < 60; i += 1) {
    try {
      targets = await (await fetch(`http://127.0.0.1:${port}/json`)).json();
      break;
    } catch {
      await sleep(500);
    }
  }
  for (const t of targets.filter((t) => t.type === "page" && t.url.startsWith(origin))) {
    await fetch(`http://127.0.0.1:${port}/json/close/${t.id}`).catch(() => {});
  }
  // A new tab with the page, in front.
  shell(serial, `am start -a android.intent.action.VIEW -d '${url}' -n ${CHROME}/com.google.android.apps.chrome.Main --ez create_new_tab true`);
  const cdp = await devtools(port, (t) => t.type === "page" && t.url.startsWith(origin));
  const evaluate = async (expression, ms = 15_000, extra = {}) => {
    const res = await cdp.send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true, ...extra }, ms);
    if (res.result?.exceptionDetails) throw new Error(`In the page: ${res.result.exceptionDetails.exception?.description ?? res.result.exceptionDetails.text}`);
    return res.result?.result?.value;
  };
  for (const domain of ["Runtime", "Log", "Page"]) await cdp.send(`${domain}.enable`);
  return { ...cdp, evaluate };
};

/** Tap the element `selector` matches on the Chrome page: scrolled into the
 * middle of the screen, then pressed where it is, as a touch the browser
 * takes for a person's. `how` is "cdp" (a touch through the DevTools
 * protocol, which the page sees as a user's) or "adb" (`input tap` at the
 * element's place on the screen: CSS pixels times the page's scale, below
 * Chrome's toolbar). Answers where it tapped, or null when there was no such
 * element. */
export const tapElement = async (serial, page, selector, how = "cdp") => {
  const box = await page.evaluate(`(() => {
    const el = document.querySelector(${JSON.stringify(selector)});
    if (!el) return null;
    el.scrollIntoView({ block: 'center', inline: 'center' });
    const r = el.getBoundingClientRect();
    return { x: r.left + r.width / 2, y: r.top + r.height / 2, dpr: devicePixelRatio, innerHeight, screenHeight: screen.height };
  })()`);
  if (!box) return null;
  await sleep(250);
  // Again, once scrolling settled.
  const at = await page.evaluate(`(() => { const r = document.querySelector(${JSON.stringify(selector)}).getBoundingClientRect(); return { x: r.left + r.width / 2, y: r.top + r.height / 2, innerHeight }; })()`);
  if (how === "cdp") {
    const point = { x: at.x, y: at.y, radiusX: 4, radiusY: 4, force: 1, id: 0 };
    await page.send("Input.dispatchTouchEvent", { type: "touchStart", touchPoints: [point] });
    await sleep(60);
    await page.send("Input.dispatchTouchEvent", { type: "touchEnd", touchPoints: [] });
    return { how, x: at.x, y: at.y };
  }
  // Where Chrome draws the page: it ends at the navigation bar, and is the
  // viewport's height tall. Its toolbar above hides as the page scrolls, so
  // the top is counted from the bottom.
  const nodes = uiNodes(serial);
  const navBar = nodes.find((n) => n.id === "android:id/navigationBarBackground")?.bounds?.[1];
  const bottom = navBar ?? Math.max(...nodes.filter((n) => n.bounds).map((n) => n.bounds[3]));
  const top = bottom - at.innerHeight * box.dpr;
  const x = at.x * box.dpr;
  const y = top + at.y * box.dpr;
  tapAt(serial, x, y);
  return { how, x, y, top };
};

/** The Chrome page as drawn (the page alone), for the record. */
export const chromeShot = async (page, file, clip) => {
  const res = await page.send("Page.captureScreenshot", { format: "png", ...(clip ? { clip: { ...clip, scale: 1 }, captureBeyondViewport: true } : {}) }, 20_000);
  if (!res.result?.data) return false;
  writeFileSync(file, Buffer.from(res.result.data, "base64"));
  return true;
};

export const ensureDir = (dir) => {
  mkdirSync(dir, { recursive: true });
  return dir;
};
