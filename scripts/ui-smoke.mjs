// Web UI smoke steps. Driven by scripts/ui-smoke.sh, which owns the server,
// the browser and the session cookie — run that, not this.
//
// Covers the lobe membership flow today. To extend: add steps with the same
// shape — click through the real chrome (the rail button, then the nav item),
// assert on the DOM, screenshot. Asserting without going through the rail is
// how a check passes on a page nobody can see.
//
// No npm dependency on purpose: the project ships its UI as plain ES modules
// with no package.json, and a screenshot harness is no reason to introduce one.
// Node 22 has a global WebSocket, Chrome ships the protocol — that is enough.
//
// usage: node ui-smoke.mjs <uiBaseUrl> <sessionCookie> <outDir>

const [, , BASE, COOKIE, OUT] = process.argv;
const CDP = process.env.CDP_PORT || "9222";

const targets = await (await fetch(`http://127.0.0.1:${CDP}/json`)).json();
const page = targets.find((t) => t.type === "page");
if (!page) throw new Error("no page target on the debugging port");

const ws = new WebSocket(page.webSocketDebuggerUrl);
await new Promise((ok, ko) => { ws.onopen = ok; ws.onerror = ko; });

let id = 0;
const pending = new Map();
const events = [];
ws.onmessage = (m) => {
    const msg = JSON.parse(m.data);
    if (msg.id && pending.has(msg.id)) {
        const { ok, ko } = pending.get(msg.id);
        pending.delete(msg.id);
        msg.error ? ko(new Error(JSON.stringify(msg.error))) : ok(msg.result);
    } else if (msg.method) {
        events.push(msg);
    }
};
const send = (method, params = {}) =>
    new Promise((ok, ko) => { const n = ++id; pending.set(n, { ok, ko }); ws.send(JSON.stringify({ id: n, method, params })); });

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// Console errors are the whole point of a visual pass: a page that throws
// renders blank and a screenshot alone would not say why.
const consoleErrors = [];
await send("Runtime.enable");
await send("Page.enable");
await send("Network.enable");
ws.addEventListener("message", (m) => {
    const msg = JSON.parse(m.data);
    if (msg.method === "Runtime.consoleAPICalled" && msg.params.type === "error") {
        consoleErrors.push(msg.params.args.map((a) => a.value ?? a.description ?? "").join(" "));
    }
    if (msg.method === "Runtime.exceptionThrown") {
        consoleErrors.push(msg.params.exceptionDetails.text + " " + (msg.params.exceptionDetails.exception?.description || ""));
    }
});

await send("Network.setCookie", { name: "n3ur0n_session", value: COOKIE, url: BASE });

const evaluate = async (expr) => {
    const r = await send("Runtime.evaluate", { expression: expr, returnByValue: true, awaitPromise: true });
    if (r.exceptionDetails) throw new Error(r.exceptionDetails.text + " " + (r.exceptionDetails.exception?.description || ""));
    return r.result.value;
};

const shot = async (name) => {
    const { data } = await send("Page.captureScreenshot", { format: "png" });
    const { writeFile } = await import("node:fs/promises");
    await writeFile(`${OUT}/${name}.png`, Buffer.from(data, "base64"));
    return `${OUT}/${name}.png`;
};

const out = { steps: [], shots: [], consoleErrors };
const step = (label, detail) => { out.steps.push({ label, detail }); };

await send("Page.navigate", { url: `${BASE}/ui/` });
await sleep(2500);

step("title", await evaluate("document.title"));
step("auth gate visible", await evaluate(`!!document.querySelector('#auth-gate:not(.hidden)')`));

// Open settings, then the Lobes nav item. The rail button is the only way
// in: clicking the nav list while the panel is hidden renders it off-screen,
// which is how a DOM-only assertion passes on an invisible page.
const openLobes = async () => {
    await evaluate(`document.querySelector('.rail-btn[data-section="settings"]').click()`);
    await sleep(900);
    await evaluate(`document.querySelector('#settings-nav .settings-nav-item[data-section="lobes"]').click()`);
    await sleep(1200);
};
await openLobes();
step("section", await evaluate("document.body.dataset.section"));
step("settings panel visible", await evaluate(
    `(() => { const p = document.getElementById('settings-page'); const r = p.getBoundingClientRect(); return !p.classList.contains('hidden') && r.width > 0 && r.height > 0; })()`));

step("page title", await evaluate(`document.getElementById('settings-page-title')?.textContent`));
step("empty state", await evaluate(`document.getElementById('lobes-chips')?.textContent?.trim()`));
out.shots.push(await shot("01-lobes-empty"));

// Type a lobe and add it, without saving: exercises the client-side grammar.
await evaluate(`(() => { const i = document.getElementById('lobes-input'); i.value = 'NOT valid'; document.getElementById('lobes-add').click(); })()`);
await sleep(300);
step("rejects invalid id", await evaluate(`document.getElementById('lobes-status')?.textContent`));

await evaluate(`(() => { const i = document.getElementById('lobes-input'); i.value = 'medical'; document.getElementById('lobes-add').click(); })()`);
await evaluate(`(() => { const i = document.getElementById('lobes-input'); i.value = 'legal-fr'; document.getElementById('lobes-add').click(); })()`);
await sleep(300);
step("chips after adding two", await evaluate(`[...document.querySelectorAll('#lobes-chips [data-lobe]')].map(e => e.dataset.lobe)`));
out.shots.push(await shot("02-lobes-two-chips"));

await evaluate(`document.getElementById('lobes-save').click()`);
await sleep(1200);
step("save status", await evaluate(`document.getElementById('lobes-status')?.textContent`));
out.shots.push(await shot("03-lobes-saved"));

// Reload: the set must come back from the server, not from the DOM.
await send("Page.navigate", { url: `${BASE}/ui/` });
await sleep(2500);
await openLobes();
step("chips after reload", await evaluate(`[...document.querySelectorAll('#lobes-chips [data-lobe]')].map(e => e.dataset.lobe)`));
out.shots.push(await shot("04-lobes-after-reload"));

// The capability form must offer those lobes as checkboxes.
await evaluate(`document.querySelector('#settings-nav .settings-nav-item[data-section="caps"]')?.click()`);
await sleep(1000);
await evaluate(`document.getElementById('settings-add-cap')?.click()`);
await sleep(800);
await evaluate(`(() => { const c = document.querySelector('[data-template]'); c?.click(); })()`);
await sleep(1500);
step("cap form lobe checkboxes", await evaluate(`[...document.querySelectorAll('.cf-lobe')].map(e => e.value)`));
out.shots.push(await shot("05-cap-form-lobes"));

console.log(JSON.stringify(out, null, 2));
ws.close();
