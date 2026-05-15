// Tiny i18n runtime for the N3UR0N web UI.
//
// Architecture:
//   - Locale catalogs are flat JSON maps shipped under /ui/locales/<code>.json.
//   - Each catalog carries an optional `_meta` block with display metadata.
//   - The server enumerates available catalogs via /api/v0/locales so the
//     language picker is data-driven; drop a new JSON file in
//     `crates/server/ui/locales/` and rebuild — no code change needed.
//   - The chosen locale is cached in `localStorage["n3ur0n_locale"]`.
//   - Templates use `t("key")` or `[data-i18n="key"]` attributes for static
//     HTML; `[data-i18n-attr-<name>="key"]` sets attribute values
//     (e.g. `data-i18n-attr-placeholder`, `data-i18n-attr-title`).
//
// Adding a new language at runtime:
//   1. Drop `crates/server/ui/locales/<code>.json` next to en.json / fr.json.
//   2. Translate every key (missing keys fall back to the key string).
//   3. Rebuild + restart — `/api/v0/locales` lists it automatically.

const STORAGE_KEY = "n3ur0n_locale";
const FALLBACK = "en";

let _current = FALLBACK;
let _catalog = {};
let _available = [];

/// Resolve which locale to start with. Priority: localStorage → navigator
/// → fallback. Trimmed to the primary subtag (e.g. "fr-CA" → "fr").
function detect() {
    const stored = localStorage.getItem(STORAGE_KEY);
    if (stored) return stored;
    const nav = (navigator.language || "en").split("-")[0];
    return nav;
}

/// Fetch the available-locales index from the server. Returns the array
/// directly so callers can render a picker. Empty array on failure.
export async function listLocales() {
    if (_available.length) return _available;
    try {
        const r = await fetch("/api/v0/locales");
        if (!r.ok) return [];
        const d = await r.json();
        _available = d.available || [];
        return _available;
    } catch {
        return [];
    }
}

/// Load a catalog by code. Falls back to FALLBACK if the requested one
/// 404s or fails to parse. Returns the resolved code.
async function loadCatalog(code) {
    try {
        const r = await fetch(`/ui/locales/${encodeURIComponent(code)}.json`);
        if (!r.ok) throw new Error(`HTTP ${r.status}`);
        _catalog = await r.json();
        _current = code;
        return code;
    } catch {
        if (code !== FALLBACK) return loadCatalog(FALLBACK);
        _catalog = {};
        _current = FALLBACK;
        return FALLBACK;
    }
}

/// Initialise i18n: fetch available locales + load the resolved catalog.
/// Returns the active locale code. Call before the first render.
export async function initI18n() {
    await listLocales();
    let want = detect();
    // If the detected locale isn't available, fall back rather than
    // returning a half-translated UI.
    if (_available.length && !_available.some(l => l.code === want)) {
        want = FALLBACK;
    }
    await loadCatalog(want);
    applyDom();
    return _current;
}

/// Switch the active locale. Persists to localStorage + re-applies DOM
/// substitutions. Caller is responsible for re-rendering any JS-built
/// content (cards, lists, etc.) that doesn't go through data-i18n attrs.
export async function setLocale(code) {
    localStorage.setItem(STORAGE_KEY, code);
    await loadCatalog(code);
    applyDom();
    document.dispatchEvent(new CustomEvent("n3ur0n:locale-changed", { detail: { code } }));
    return _current;
}

/// Translate a key. Optional `params` substitutes `{name}` tokens.
/// Missing keys return the key itself so untranslated paths are
/// immediately visible.
export function t(key, params) {
    let s = _catalog[key];
    if (s === undefined || s === null) s = key;
    if (params && typeof s === "string") {
        for (const [k, v] of Object.entries(params)) {
            s = s.replace(new RegExp(`\\{${k}\\}`, "g"), String(v));
        }
    }
    return s;
}

export function currentLocale() {
    return _current;
}

/// Walk the document, replace `[data-i18n]` text + `[data-i18n-attr-*]`
/// attributes. Safe to call repeatedly; only nodes carrying the attrs are
/// touched.
function applyDom(root) {
    const scope = root || document;
    scope.querySelectorAll("[data-i18n]").forEach(el => {
        const key = el.getAttribute("data-i18n");
        if (!key) return;
        // Permit minimal inline markup (<strong>, <code>) by setting
        // innerHTML when the catalog value contains an angle bracket;
        // otherwise textContent for safety.
        const val = t(key);
        if (typeof val === "string" && val.indexOf("<") >= 0) {
            el.innerHTML = val;
        } else {
            el.textContent = val;
        }
    });
    scope.querySelectorAll("*").forEach(el => {
        for (const attr of el.attributes) {
            if (!attr.name.startsWith("data-i18n-attr-")) continue;
            const target = attr.name.slice("data-i18n-attr-".length);
            const val = t(attr.value);
            if (typeof val === "string") el.setAttribute(target, val);
        }
    });
}

// Convenience re-export so callers can pin a manual scope.
export const refresh = applyDom;
