// Frontend auth glue.
//
// Three states the app cares about:
//   - bootstrap_required: zero users exist → render the bootstrap form.
//   - unauthenticated:    has users, no session cookie or expired → login.
//   - authenticated:      session cookie valid; me().role / .permissions
//                         drive UI gating.
//
// The server enforces auth. This module is cosmetic (decides which screen
// to show + whether a button is rendered). Never assume role checks here
// are security boundaries.

import { t, listLocales, setLocale, currentLocale } from "./i18n.js";

let _state = {
    authenticated: false,
    bootstrap_required: false,
    id: null,
    username: null,
    role: null,
    permissions: [],
};

const _listeners = new Set();

/// Fetch /auth/me and cache the result. Returns the new state.
export async function refresh() {
    try {
        const r = await fetch("/api/v0/auth/me", { credentials: "same-origin" });
        const d = await r.json();
        _state = {
            authenticated: !!d.authenticated,
            bootstrap_required: !!d.bootstrap_required,
            id: d.id ?? null,
            username: d.username ?? null,
            role: d.role ?? null,
            permissions: d.permissions || [],
        };
    } catch (e) {
        _state = { authenticated: false, bootstrap_required: false, id: null, username: null, role: null, permissions: [] };
    }
    _emit();
    return _state;
}

export function state() { return _state; }
export function isAuthed() { return _state.authenticated; }
export function role() { return _state.role; }
export function hasPerm(p) { return _state.permissions.includes(p); }

export function onChange(fn) {
    _listeners.add(fn);
    return () => _listeners.delete(fn);
}

function _emit() {
    for (const fn of _listeners) {
        try { fn(_state); } catch (e) { console.error("auth listener", e); }
    }
}

/// Apply permission gating to the DOM. Any element with `data-perm="key"`
/// is hidden when the current user lacks that permission. `data-perm-any`
/// accepts a comma-separated list — element shown when ANY of them match.
export function applyPermDom(scope) {
    const root = scope || document;
    root.querySelectorAll("[data-perm]").forEach(el => {
        const want = el.getAttribute("data-perm");
        el.classList.toggle("hidden", !hasPerm(want));
    });
    root.querySelectorAll("[data-perm-any]").forEach(el => {
        const list = (el.getAttribute("data-perm-any") || "").split(",").map(s => s.trim()).filter(Boolean);
        const ok = list.some(p => hasPerm(p));
        el.classList.toggle("hidden", !ok);
    });
}

// ---- Network actions -----------------------------------------------------

async function postJson(path, body) {
    const r = await fetch(path, {
        method: "POST",
        credentials: "same-origin",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(body),
    });
    const text = await r.text();
    let payload = null;
    try { payload = text ? JSON.parse(text) : null; } catch { /* ignore */ }
    if (!r.ok) {
        const err = new Error(payload?.error || `HTTP ${r.status}`);
        err.status = r.status;
        throw err;
    }
    return payload;
}

export async function login(username, password) {
    await postJson("/api/v0/auth/login", { username, password });
    await refresh();
}

export async function bootstrap(username, password) {
    await postJson("/api/v0/auth/bootstrap", { username, password });
    await refresh();
}

export async function logout() {
    try { await postJson("/api/v0/auth/logout", {}); } catch { /* ignore */ }
    await refresh();
}

export async function changePassword(current, next) {
    await postJson("/api/v0/auth/password", { current_password: current, new_password: next });
}

// ---- Gate UI ------------------------------------------------------------

/// Render the appropriate gate screen (bootstrap or login) when the user
/// isn't authenticated. Returns true when a gate is currently shown — the
/// caller should skip further app rendering.
export function renderAuthGate() {
    const gate = document.getElementById("auth-gate");
    if (!gate) return false;
    if (_state.authenticated) {
        gate.classList.add("hidden");
        return false;
    }
    gate.classList.remove("hidden");
    if (_state.bootstrap_required) {
        renderBootstrapForm(gate);
    } else {
        renderLoginForm(gate);
    }
    return true;
}

function readAuthDraft(host) {
    return {
        user: host.querySelector("#auth-user")?.value ?? "",
        password: host.querySelector("#auth-pw")?.value ?? "",
    };
}

function restoreAuthDraft(host, draft) {
    if (!draft) return;
    const user = host.querySelector("#auth-user");
    const pw = host.querySelector("#auth-pw");
    if (user && draft.user) user.value = draft.user;
    if (pw && draft.password) pw.value = draft.password;
}

async function wireLangToggle(host) {
    const bar = host.querySelector(".auth-lang-toggle");
    if (!bar) return;
    const locales = await listLocales();
    if (!locales.length) {
        bar.remove();
        return;
    }
    const cur = currentLocale();
    bar.innerHTML = locales.map(l => {
        const code = String(l.code || "").toUpperCase();
        const on = l.code === cur;
        return `<button type="button" data-locale="${escapeHtml(l.code)}"
            class="${on ? "active" : ""}" aria-pressed="${on ? "true" : "false"}"
            title="${escapeHtml(l.native_name || l.name || l.code)}">${escapeHtml(code)}</button>`;
    }).join("");
    bar.querySelectorAll("[data-locale]").forEach(btn => {
        btn.addEventListener("click", async () => {
            if (btn.dataset.locale === currentLocale()) return;
            const draft = readAuthDraft(host);
            await setLocale(btn.dataset.locale);
            document.documentElement.lang = currentLocale();
            renderAuthGate();
            restoreAuthDraft(document.getElementById("auth-gate"), draft);
        });
    });
}

function authLangToggleHtml() {
    return `<div class="auth-lang-toggle" role="group"
        aria-label="${escapeHtml(t("settings.ui.language"))}"></div>`;
}

function renderLoginForm(host) {
    host.innerHTML = `
        <div class="auth-card">
            <img class="brand-mark" src="/ui/brand-mark.png" width="64" height="64" alt="" aria-hidden="true">
            <h2>${escapeHtml(t("auth.login.title"))}</h2>
            <p class="row-sub">${escapeHtml(t("auth.login.help"))}</p>
            <form class="kv" id="auth-form" onsubmit="return false;">
                <label for="auth-user">${escapeHtml(t("auth.field.username"))}</label>
                <input id="auth-user" type="text" required autocomplete="username" autofocus />
                <label for="auth-pw">${escapeHtml(t("auth.field.password"))}</label>
                <input id="auth-pw" type="password" required autocomplete="current-password" />
            </form>
            <div class="auth-actions">
                <button class="primary" id="auth-submit" type="button">${escapeHtml(t("auth.login.submit"))}</button>
            </div>
            <p class="row-sub auth-status" id="auth-status"></p>
        </div>
        ${authLangToggleHtml()}
    `;
    void wireLangToggle(host);
    wireSubmit(host, async (u, p, status) => {
        try {
            await login(u, p);
            renderAuthGate();
            // Re-render the visible area: let the rest of the app boot.
            document.dispatchEvent(new CustomEvent("n3ur0n:auth-changed"));
        } catch (e) {
            status.textContent = t("auth.login.error", { msg: e.message || "" });
        }
    });
}

function renderBootstrapForm(host) {
    host.innerHTML = `
        <div class="auth-card">
            <img class="brand-mark" src="/ui/brand-mark.png" width="64" height="64" alt="" aria-hidden="true">
            <h2>${escapeHtml(t("auth.bootstrap.title"))}</h2>
            <p class="row-sub">${escapeHtml(t("auth.bootstrap.help"))}</p>
            <form class="kv" id="auth-form" onsubmit="return false;">
                <label for="auth-user">${escapeHtml(t("auth.field.username"))}</label>
                <input id="auth-user" type="text" required autocomplete="username" autofocus />
                <label for="auth-pw">${escapeHtml(t("auth.field.password_new"))}</label>
                <input id="auth-pw" type="password" required autocomplete="new-password" minlength="6" />
            </form>
            <div class="auth-actions">
                <button class="primary" id="auth-submit" type="button">${escapeHtml(t("auth.bootstrap.submit"))}</button>
            </div>
            <p class="row-sub auth-status" id="auth-status"></p>
        </div>
        ${authLangToggleHtml()}
    `;
    void wireLangToggle(host);
    wireSubmit(host, async (u, p, status) => {
        try {
            await bootstrap(u, p);
            renderAuthGate();
            document.dispatchEvent(new CustomEvent("n3ur0n:auth-changed"));
        } catch (e) {
            status.textContent = t("auth.bootstrap.error", { msg: e.message || "" });
        }
    });
}

function wireSubmit(host, handler) {
    const submit = host.querySelector("#auth-submit");
    const status = host.querySelector("#auth-status");
    const form = host.querySelector("#auth-form");
    const fire = async () => {
        const u = host.querySelector("#auth-user").value.trim();
        const p = host.querySelector("#auth-pw").value;
        if (!u || !p) { status.textContent = t("auth.required"); return; }
        status.textContent = "…";
        await handler(u, p, status);
    };
    submit?.addEventListener("click", fire);
    form?.addEventListener("keydown", (e) => {
        if (e.key === "Enter") { e.preventDefault(); fire(); }
    });
}

function escapeHtml(s) {
    if (s === null || s === undefined) return "";
    return String(s).replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
}
