// The parties of an end-to-end run other than the person at the app's
// window: another device that holds a key, and the setup of a programmatic
// signer. scripts/drive-intents-e2e.mjs uses these.

import { b58encode, sleep } from "./drive-lib.mjs";

/** The device that holds an offline wallet's key, played as a person
 * would: read the transaction from Review and Sign's Share (its Transaction
 * tab, as base64), sign its message with the key, and bring the signature
 * back by pasting a signature object (docs/signing.md, "The pipeline") into
 * Add a signature. */
export const offlineSigner = {
  async sign({ page, key, run, shootApp, say }) {
    // What this device signs comes first (a key made here for a new account,
    // say): the person presses Sign, then shares what is left.
    const signedHere = await page.run(`
      const ready = await until(() => [...document.querySelectorAll('.content button')].find((b) => visible(b) && !b.disabled && /^(Share with another device|Sign transaction)$/.test(b.innerText.trim())), 30000);
      if (!ready || ready.innerText.trim() !== 'Sign transaction') return false;
      await until(() => !document.body.innerText.includes('Simulating'), 15000);
      await w(800);
      ready.click();
      await until(() => [...document.querySelectorAll('.content button')].some((b) => visible(b) && b.innerText.trim() === 'Share with another device'), 15000);
      return true;
    `);
    if (signedHere) say("other device: this device signed its own part first (Sign transaction on Review and Sign)");
    // Review and Sign as the person sees it, waiting for the other device.
    await page.run(`await until(() => [...document.querySelectorAll('.content button')].some((b) => visible(b) && b.innerText.trim() === 'Share with another device'), 30000); await w(800); return true;`);
    await shootApp(`${run}-waiting-for-the-other-device`);
    const shared = await page.run(`
      const content = () => document.querySelector('.content');
      const share = await until(() => [...(content()?.querySelectorAll('button') ?? [])].find((b) => visible(b) && !b.disabled && b.innerText.trim() === 'Share with another device'), 30000);
      if (!share) return { error: 'no Share with another device: ' + buttons() };
      share.click();
      const dialog = await until(() => document.querySelector('.modal.share'), 10000);
      if (!dialog) return { error: 'no share dialog' };
      button('Transaction', dialog)?.click();
      await w(400);
      const disclose = [...dialog.querySelectorAll('button.disclosure')].find((b) => b.innerText.includes('Base64'));
      disclose?.click();
      const area = await until(() => dialog.querySelector('textarea[aria-label=Base64]'), 5000);
      if (!area) return { error: 'no base64 of the transaction: ' + dialog.innerText.slice(0, 400) };
      return { wire: area.value };
    `);
    if (shared?.error || !shared?.wire) throw new Error(shared?.error ?? "nothing read from Share");
    await shootApp(`${run}-share`);
    await page.run(`
      const dialog = document.querySelector('.modal.share');
      const close = dialog && [...dialog.querySelectorAll('button')].find((b) => visible(b) && (/^(Close|Done|×)$/.test(b.innerText.trim()) || b.getAttribute('aria-label') === 'Close'));
      if (close) close.click(); else document.querySelector('.modal-backdrop')?.click();
      await until(() => !document.querySelector('.modal.share'), 5000);
      return true;
    `);
    // A v1 wire is its message, then a signature for each signer.
    const wire = Buffer.from(shared.wire, "base64");
    if (wire[0] !== 0x81) throw new Error(`the shared transaction is not v1 (first byte ${wire[0]})`);
    const signers = wire[1];
    const message = wire.subarray(0, wire.length - 64 * signers);
    const object = JSON.stringify({ type: "signature", signs: "transaction", pubkey: b58encode(key.publicKey), signature: b58encode(key.sign(message)) });
    say(`other device: signed the transaction's message (${message.length} bytes), pasting the signature back`);
    const pasted = await page.run(`
      const add = await until(() => [...document.querySelectorAll('.content button')].find((b) => visible(b) && !b.disabled && b.innerText.trim() === 'Add a signature'), 10000);
      if (!add) return 'no Add a signature: ' + buttons();
      add.click();
      const area = await until(() => [...document.querySelectorAll('.modal textarea')].find(visible), 5000);
      if (!area) return 'no paste box';
      type(area, ${JSON.stringify(object)});
      await w(1500);
      const error = document.querySelector('.modal .error')?.innerText;
      if (error) return 'refused: ' + error;
      // Taken as it lands; the box closes, or Load takes it.
      if (document.querySelector('.modal textarea')) button('Load', document.querySelector('.modal'))?.click();
      await w(1000);
      return document.querySelector('.modal .error')?.innerText ? 'refused: ' + document.querySelector('.modal .error').innerText : 'ok';
    `);
    await sleep(1500);
    await shootApp(`${run}-signature-added`);
    if (pasted !== "ok") throw new Error(pasted);
    return `signed on the other device and pasted back (${object.length} characters)`;
  },
};

/** A programmatic signer whose authority is Alice, a hot key of the app,
 * made through the app as a person does: Add wallet, Create new,
 * Programmatic signer; then its executor nonce account from its Settings,
 * signed and submitted on Review and Sign. Answers `{ label, address }`. */
export const programmaticSigner = {
  async setup({ page, need, say, label = "Signer", authority = "Alice" }) {
    const made = await page.run(`
      if (!button('Add wallet')) nav('Wallets')?.click();
      if (!await until(() => button('Add wallet'))) return { error: 'no Wallets' };
      button('Add wallet').click();
      await w(500);
      button('Create new', modal()).click();
      await w(300);
      button('Programmatic signer', modal()).click();
      await w(500);
      const m = modal();
      type(field('Label', m).querySelector('input'), ${JSON.stringify(label)});
      const source = field('Authority key', m).querySelector('select');
      source.value = 'wallet';
      source.dispatchEvent(new Event('change', { bubbles: true }));
      await w(400);
      const pick = async (fieldLabel, name) => {
        const holder = field(fieldLabel, m);
        if (!holder) return false;
        holder.querySelector('.picker-button').click();
        await w(300);
        const item = [...document.querySelectorAll('.picker-menu .picker-item')].find((b) => b.querySelector('.addr-label')?.innerText.trim() === name);
        if (!item) return false;
        item.click();
        await w(300);
        return true;
      };
      if (!await pick('Wallet', ${JSON.stringify(authority)})) return { error: 'no authority ' + ${JSON.stringify(authority)} + ': ' + m.innerText };
      if (!await pick('Pays fees', ${JSON.stringify(authority)})) return { error: 'no payer: ' + m.innerText };
      const address = await until(() => m.querySelector('.wallet-fact code')?.innerText.trim(), 5000);
      button('Create programmatic signer', m).click();
      if (!await until(() => !modal(), 10000)) return { error: 'the form stayed: ' + (modal()?.querySelector('.error')?.innerText ?? '') };
      return { address };
    `);
    if (made?.error || !made?.address) await need("adding the programmatic signer", made?.error ?? "no address");
    const opened = await page.run(`
      const gear = await until(() => document.querySelector('button[aria-label=' + JSON.stringify('Settings of ' + ${JSON.stringify(label)}) + ']'), 10000);
      if (!gear) return 'no Settings of ' + ${JSON.stringify(label)};
      gear.click();
      const create = await until(() => [...document.querySelectorAll('.modal button')].find((b) => visible(b) && !b.disabled && b.innerText.trim() === 'Create nonce account'), 10000);
      if (!create) return 'no Create nonce account: ' + (modal()?.innerText ?? '');
      create.click();
      if (!await until(() => !modal(), 20000)) return 'the settings stayed: ' + (modal()?.querySelector('.error')?.innerText ?? '');
      return 'ok';
    `);
    await need("opening the executor nonce account's creation", opened);
    const { signAndSubmit } = await import("./drive-lib.mjs");
    await need("creating the executor nonce account", await signAndSubmit(page));
    say(`programmatic signer ${label} ${made.address}, authority ${authority}, executor nonce account created`);
    return { label, address: made.address };
  },
};
