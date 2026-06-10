import { initI18n, t, setLocale, listLocales, currentLocale, refresh as i18nRefresh } from "./i18n.js";
import * as auth from "./auth.js";
import { applyIcons, iconHtml, mimeIcon } from "./icons.js";

// --- Theme ---------------------------------------------------------------
// Pref persisted as one of "dark" / "light" / "system". Applied to
// `:root[data-theme]`; CSS variables in style.css branch on the attribute.
const THEME_KEY = "n3ur0n_theme";
function currentThemePref() {
    return localStorage.getItem(THEME_KEY) || "system";
}
function resolveTheme(pref) {
    if (pref === "dark" || pref === "light") return pref;
    return window.matchMedia("(prefers-color-scheme: light)").matches ? "light" : "dark";
}
function applyTheme() {
    const pref = currentThemePref();
    document.documentElement.setAttribute("data-theme", resolveTheme(pref));
}
function setThemePref(pref) {
    localStorage.setItem(THEME_KEY, pref);
    applyTheme();
    document.dispatchEvent(new CustomEvent("n3ur0n:theme-changed", { detail: { pref } }));
}
// Apply immediately so the initial paint matches the user's choice.
applyTheme();
// Track OS changes when pref is "system".
window.matchMedia("(prefers-color-scheme: light)").addEventListener("change", () => {
    if (currentThemePref() === "system") applyTheme();
});

// Boot i18n before the rest of the app runs. Static `data-i18n` attrs are
// applied once the catalog loads; dynamic render paths call `t(...)` at
// build time and re-render on locale-changed events.
initI18n().then(() => {
    applyIcons();
    return bootAuth();
}).catch(err => console.warn("i18n init failed:", err));

// Run after i18n so the auth gate uses translated strings.
async function bootAuth() {
    await auth.refresh();
    refreshAuthChrome();
    auth.renderAuthGate();
}
function refreshAuthChrome() {
    const nameEl = document.getElementById("user-name");
    const logoutBtn = document.getElementById("user-logout");
    const s = auth.state();
    if (s.authenticated && s.username) {
        if (nameEl) nameEl.textContent = s.username;
        logoutBtn?.classList.remove("hidden");
    } else {
        if (nameEl) nameEl.textContent = "";
        logoutBtn?.classList.add("hidden");
    }
    auth.applyPermDom();
}
document.addEventListener("n3ur0n:auth-changed", () => {
    refreshAuthChrome();
    // Trigger refresh of any currently-mounted view.
    try {
        if (document.body.dataset.section === "settings") {
            const active = document.querySelector("#settings-nav .settings-nav-item.active");
            if (active) activateSettingsSection(active.dataset.section);
        }
    } catch { /* boot order edge case */ }
});
document.getElementById("user-logout")?.addEventListener("click", async () => {
    await auth.logout();
    refreshAuthChrome();
    auth.renderAuthGate();
});
document.addEventListener("n3ur0n:locale-changed", () => {
    // Re-render whatever section is currently visible so dynamic strings
    // refresh. Cheap because every renderer is idempotent + pulls from
    // cached state.
    syncComposerModeUi();
    if (document.body.dataset.section === "settings") {
        const active = document.querySelector("#settings-nav .settings-nav-item.active");
        if (active) activateSettingsSection(active.dataset.section);
    }
    if (document.body.dataset.section === "network" || document.body.dataset.section === "skills") {
        const hint = document.getElementById("workspace-empty-hint");
        if (hint) {
            hint.innerHTML = document.body.dataset.section === "network"
                ? t("workspace.hint.network")
                : t("workspace.hint.skills");
        }
    }
    renderFilesList();
    applyIcons();
});

const $ = (id) => document.getElementById(id);
const sidebar = $("conv-list");
const conv = $("conversation");
const titleEl = $("conv-title");
const promptEl = $("prompt");
const sendBtn = $("send");
const composerAttachBtn = $("composer-attach");
const composerFileInput = $("composer-file-input");
const composerAttachmentsEl = $("composer-attachments");
const composerMenuBtn = $("composer-menu");
const composerMenuPopover = $("composer-menu-popover");
const chatModeDirectEl = $("chat-mode-direct");
const chatModeToggleEl = $("chat-mode-toggle");
const directModelEl = $("direct-model");
const newBtn = $("new-chat");
const renameBtn = $("rename");
const deleteBtn = $("delete");
const selfId = $("self-id");

const LS_CURRENT = "n3ur0n_current_conversation";
const LS_CHAT_MODE_PREFIX = "n3ur0n_chat_mode:";
const LS_DIRECT_MODEL = "n3ur0n_direct_model";
let activeId = localStorage.getItem(LS_CURRENT) || null;

if (directModelEl && localStorage.getItem(LS_DIRECT_MODEL)) {
    directModelEl.value = localStorage.getItem(LS_DIRECT_MODEL);
}

function chatModeKey(convId) {
    return `${LS_CHAT_MODE_PREFIX}${convId}`;
}

function getChatMode(convId) {
    if (!convId) return "auto";
    return localStorage.getItem(chatModeKey(convId)) === "direct" ? "direct" : "auto";
}

function setChatMode(convId, mode) {
    if (!convId) return;
    localStorage.setItem(chatModeKey(convId), mode);
}

function syncComposerModeUi() {
    const hasConv = !!activeId;
    const isDirect = hasConv && getChatMode(activeId) === "direct";
    if (chatModeDirectEl) {
        chatModeDirectEl.checked = !!isDirect;
        chatModeDirectEl.disabled = !hasConv || inFlight;
    }
    const directModelField = document.getElementById("direct-model-field");
    if (directModelEl) {
        directModelEl.disabled = !hasConv || inFlight || !isDirect;
        directModelEl.placeholder = t("composer.direct.model_placeholder");
    }
    if (directModelField) {
        directModelField.classList.toggle("hidden", !isDirect);
    }
    if (chatModeToggleEl) {
        chatModeToggleEl.classList.toggle("hidden", !hasConv);
        chatModeToggleEl.disabled = !hasConv || inFlight;
        chatModeToggleEl.dataset.mode = isDirect ? "direct" : "auto";
        chatModeToggleEl.textContent = isDirect
            ? t("composer.mode.direct")
            : t("composer.mode.auto");
        chatModeToggleEl.title = t("composer.mode.direct.tooltip");
    }
    updateComposerControls();
}

function updateComposerControls() {
    const enabled = !!activeId && !inFlight;
    const canSend = enabled && (promptEl.value.trim().length > 0 || draftAttachments.length > 0);
    if (composerMenuBtn) composerMenuBtn.disabled = !enabled;
    if (composerAttachBtn) composerAttachBtn.disabled = !enabled;
    if (promptEl) promptEl.disabled = !enabled;
    if (sendBtn) sendBtn.disabled = !canSend;
}

function clearDraftAttachments() {
    draftAttachments = [];
    renderComposerDraft();
}

function renderComposerDraft() {
    if (!composerAttachmentsEl) return;
    if (draftAttachments.length === 0) {
        composerAttachmentsEl.classList.add("hidden");
        composerAttachmentsEl.innerHTML = "";
        updateComposerControls();
        return;
    }
    composerAttachmentsEl.classList.remove("hidden");
    composerAttachmentsEl.innerHTML = "";
    for (const att of draftAttachments) {
        const chip = document.createElement("div");
        chip.className = "draft-chip";
        chip.title = att.hash || "";
        const icon = document.createElement("span");
        icon.className = "draft-chip-icon";
        icon.innerHTML = iconHtml(mimeIcon(att.mime), { size: 14 });
        chip.appendChild(icon);
        const label = document.createElement("span");
        label.className = "draft-chip-label";
        label.textContent = att.name || att.mime || "file";
        chip.appendChild(label);
        const remove = document.createElement("button");
        remove.type = "button";
        remove.className = "draft-chip-remove";
        remove.setAttribute("aria-label", "Remove");
        remove.textContent = "×";
        remove.addEventListener("click", () => {
            draftAttachments = draftAttachments.filter(a => a.hash !== att.hash);
            renderComposerDraft();
        });
        chip.appendChild(remove);
        composerAttachmentsEl.appendChild(chip);
    }
    updateComposerControls();
}

function attachmentLabel(att) {
    return att.name || att.mime || "file";
}

function appendAttachmentCards(container, attachments) {
    if (!attachments?.length) return;
    const wrap = document.createElement("div");
    wrap.className = "bubble-attachments";
    for (const att of attachments) {
        const link = document.createElement("a");
        link.className = "bubble-attachment";
        link.href = `/api/v0/files/${encodeURIComponent(att.hash)}`;
        link.target = "_blank";
        link.rel = "noopener";
        const icon = document.createElement("span");
        icon.className = "draft-chip-icon";
        icon.innerHTML = iconHtml(mimeIcon(att.mime), { size: 14 });
        link.appendChild(icon);
        const label = document.createElement("span");
        label.className = "draft-chip-label";
        label.textContent = attachmentLabel(att);
        link.appendChild(label);
        wrap.appendChild(link);
    }
    container.appendChild(wrap);
}

function resizeComposerTextarea() {
    if (!promptEl) return;
    promptEl.style.height = "auto";
    promptEl.style.height = `${Math.min(promptEl.scrollHeight, 160)}px`;
}

function toggleComposerMenu(open) {
    if (!composerMenuPopover) return;
    const show = open ?? composerMenuPopover.classList.contains("hidden");
    composerMenuPopover.classList.toggle("hidden", !show);
}

function setChatModeFromUi(mode) {
    if (!activeId) return;
    setChatMode(activeId, mode);
    if (chatModeDirectEl) chatModeDirectEl.checked = mode === "direct";
    syncComposerModeUi();
}

chatModeToggleEl?.addEventListener("click", () => {
    if (!activeId || inFlight) return;
    const next = getChatMode(activeId) === "direct" ? "auto" : "direct";
    setChatModeFromUi(next);
});

directModelEl?.addEventListener("input", () => {
    localStorage.setItem(LS_DIRECT_MODEL, directModelEl.value);
});
let conversations = [];
let inFlight = false;
/** Per-message draft attachments (cleared on send or conversation switch). */
let draftAttachments = [];

// ---------------------------------------------------------------------------
// Generic helpers
// ---------------------------------------------------------------------------

function fmtDate(ts) {
    if (!ts) return "";
    const d = new Date(ts * 1000);
    return d.toLocaleString(undefined, { dateStyle: "short", timeStyle: "short" });
}

async function api(method, path, body) {
    const opts = { method, credentials: "same-origin", headers: {} };
    if (body !== undefined) {
        opts.headers["content-type"] = "application/json";
        opts.body = JSON.stringify(body);
    }
    const res = await fetch(path, opts);
    if (res.status === 204) return null;
    const text = await res.text();
    let payload = null;
    try { payload = text ? JSON.parse(text) : null; } catch { /* ignore */ }
    if (res.status === 401) {
        // Session expired or never existed. Force the gate; downstream
        // callers can still inspect the thrown error if they care.
        auth.refresh().then(() => auth.renderAuthGate());
    }
    if (!res.ok) {
        const msg = payload?.error || payload?.message || text || `HTTP ${res.status}`;
        const err = new Error(msg);
        err.status = res.status;
        err.payload = payload;
        throw err;
    }
    return payload;
}

// ---------------------------------------------------------------------------
// Modal dialog (replaces native confirm/alert — Tauri WKWebView blocks them)
// ---------------------------------------------------------------------------

function _openModal({ title, message, okLabel, cancelLabel, danger, withCancel }) {
    return new Promise((resolve) => {
        const modal = $("modal");
        const titleEl = $("modal-title");
        const bodyEl = $("modal-body");
        const okBtn = $("modal-ok");
        const cancelBtn = $("modal-cancel");
        titleEl.textContent = title;
        bodyEl.textContent = message;
        okBtn.textContent = okLabel;
        cancelBtn.textContent = cancelLabel;
        cancelBtn.style.display = withCancel ? "" : "none";
        okBtn.classList.toggle("modal-ok-danger", !!danger);
        modal.classList.remove("hidden");
        modal.setAttribute("aria-hidden", "false");

        const cleanup = (result) => {
            modal.classList.add("hidden");
            modal.setAttribute("aria-hidden", "true");
            okBtn.removeEventListener("click", onOk);
            cancelBtn.removeEventListener("click", onCancel);
            document.removeEventListener("keydown", onKey);
            modal.querySelector(".modal-backdrop").removeEventListener("click", onCancel);
            okBtn.classList.remove("modal-ok-danger");
            resolve(result);
        };
        const onOk = () => cleanup(true);
        const onCancel = () => cleanup(false);
        const onKey = (e) => {
            if (e.key === "Escape") onCancel();
            else if (e.key === "Enter") onOk();
        };
        okBtn.addEventListener("click", onOk);
        cancelBtn.addEventListener("click", onCancel);
        modal.querySelector(".modal-backdrop").addEventListener("click", onCancel);
        document.addEventListener("keydown", onKey);
        setTimeout(() => okBtn.focus(), 0);
    });
}

function confirmModal(message, opts = {}) {
    return _openModal({
        title: opts.title || "Confirm",
        message,
        okLabel: opts.okLabel || "OK",
        cancelLabel: opts.cancelLabel || "Cancel",
        danger: !!opts.danger,
        withCancel: true,
    });
}

function alertModal(message, opts = {}) {
    return _openModal({
        title: opts.title || "Notice",
        message,
        okLabel: opts.okLabel || "OK",
        cancelLabel: "Cancel",
        danger: !!opts.danger,
        withCancel: false,
    });
}

// ---------------------------------------------------------------------------
// Sidebar
// ---------------------------------------------------------------------------

async function loadConversations() {
    sidebar.innerHTML = "";
    try {
        const r = await api("GET", "/api/v0/whoami");
        const id = r?.instance_id || "?";
        selfId.textContent = id;
        selfId.title = id;
    } catch { /* ignore */ }
    try {
        const r = await api("GET", "/api/v0/conversations");
        conversations = r?.conversations || [];
    } catch (e) {
        conversations = [];
        sidebar.innerHTML = `<li class="empty">Load failed: ${e.message}</li>`;
        return;
    }
    if (conversations.length === 0) {
        sidebar.innerHTML = '<li class="empty">No conversations yet.</li>';
        return;
    }
    for (const c of conversations) {
        const li = document.createElement("li");
        li.dataset.id = c.id;
        if (c.id === activeId) li.classList.add("active");
        const t = document.createElement("span");
        t.className = "title";
        t.textContent = c.title || "(untitled)";
        const ts = document.createElement("span");
        ts.className = "ts";
        ts.textContent = fmtDate(c.updated_at);
        li.appendChild(t);
        li.appendChild(ts);
        li.addEventListener("click", () => selectConversation(c.id));
        sidebar.appendChild(li);
    }
}

async function selectConversation(id) {
    activeId = id;
    localStorage.setItem(LS_CURRENT, id);
    clearDraftAttachments();
    toggleComposerMenu(false);
    document.querySelectorAll(".conv-list li").forEach(li => {
        li.classList.toggle("active", li.dataset.id === id);
    });
    await renderActive();
}

async function renderActive() {
    if (!activeId) {
        titleEl.textContent = "No conversation";
        renameBtn.disabled = true;
        deleteBtn.disabled = true;
        promptEl.disabled = true;
        sendBtn.disabled = true;
        clearDraftAttachments();
        syncComposerModeUi();
        conv.innerHTML = '<p class="empty-hint">Pick a conversation in the sidebar or click <strong>+ New chat</strong>.</p>';
        return;
    }
    let data;
    try {
        data = await api("GET", `/api/v0/conversations/${encodeURIComponent(activeId)}`);
    } catch (e) {
        if (e.status === 404) {
            // Stale cookie or deleted — drop selection.
            localStorage.removeItem(LS_CURRENT);
            activeId = null;
            await loadConversations();
            await renderActive();
            return;
        }
        conv.innerHTML = `<div class="bubble error">${e.message}</div>`;
        return;
    }
    titleEl.textContent = data.title || "(untitled)";
    renameBtn.disabled = false;
    deleteBtn.disabled = false;
    syncComposerModeUi();
    resizeComposerTextarea();
    promptEl.focus();
    renderTurns(data.turns || []);
}

// ---------------------------------------------------------------------------
// Conversation rendering
// ---------------------------------------------------------------------------

function renderTurns(turns) {
    conv.innerHTML = "";
    let pending = []; // accumulated tool_call/tool_result turns since last user
    const flushPending = () => {
        if (pending.length === 0) return;
        renderHistoricalStepper(pending);
        pending = [];
    };
    for (const t of turns) {
        if (!t || !t.role) continue;
        if (t.role === "user") {
            flushPending();
            appendBubble("user", "you", t.content, t.attachments);
        } else if (t.role === "assistant") {
            flushPending();
            const who = t.model ? `assistant · ${t.model}` : "assistant";
            appendBubble("assistant", who, t.content || "");
        } else if (t.role === "tool_call" || t.role === "tool_result") {
            pending.push(t);
        }
        // system turns hidden
    }
    flushPending();
    conv.scrollTop = conv.scrollHeight;
}

/// Render a finished dispatch as a static chip row matching the live stepper.
function renderHistoricalStepper(toolTurns) {
    const wrap = document.createElement("div");
    wrap.className = "stepper complete";
    const status = document.createElement("div");
    status.className = "stepper-status";
    wrap.appendChild(status);
    const row = document.createElement("div");
    row.className = "stepper-row";
    wrap.appendChild(row);

    // Pair tool_call + tool_result by call_id (or fallback by index).
    const calls = toolTurns.filter(t => t.role === "tool_call");
    const resultsById = new Map();
    const resultsByIdx = [];
    for (const t of toolTurns) {
        if (t.role === "tool_result") {
            if (t.call_id) resultsById.set(t.call_id, t);
            resultsByIdx.push(t);
        }
    }

    let n = 0;
    let errors = 0;
    for (let i = 0; i < calls.length; i++) {
        const call = calls[i];
        const result = (call.id && resultsById.get(call.id)) || resultsByIdx[i] || null;
        const hasError = result && !!result.error;
        const chip = document.createElement("div");
        chip.className = `chip ${hasError ? "error" : "done"}`;
        const num = document.createElement("span");
        num.className = "chip-num";
        num.textContent = ++n;
        const label = document.createElement("span");
        label.className = "chip-label";
        label.textContent = `${shortPeer(call.peer_id).slice(0, 6)}::${call.capability}`;
        chip.appendChild(num);
        chip.appendChild(label);
        chip.dataset.idx = String(i);
        chip.style.cursor = "pointer";
        chip.addEventListener("click", () => toggleStepDetails(wrap, call, result, chip));
        row.appendChild(chip);
        if (hasError) errors++;
    }
    status.textContent = errors
        ? `dispatch · ${calls.length} step${calls.length > 1 ? "s" : ""} · ${errors} error${errors > 1 ? "s" : ""}`
        : `dispatch · ${calls.length} step${calls.length > 1 ? "s" : ""}`;

    conv.appendChild(wrap);
}

function toggleStepDetails(wrap, call, result, chipEl) {
    const stepKey = call.id || `${call.peer_id}::${call.capability}::${chipEl?.dataset.idx ?? ""}`;
    const existing = wrap.querySelector(".stepper-details");
    const wasSame = existing && existing.dataset.stepKey === stepKey;
    if (existing) existing.remove();
    wrap.querySelectorAll(".chip.active").forEach(c => c.classList.remove("active"));
    if (wasSame) return;

    const panel = document.createElement("div");
    panel.className = "stepper-details";
    panel.dataset.stepKey = stepKey;
    const cap = `${shortPeer(call.peer_id)}::${call.capability}`;
    const stepNum = chipEl?.querySelector(".chip-num")?.textContent
        ?? (chipEl?.dataset.idx ? String(parseInt(chipEl.dataset.idx, 10) + 1) : "?");
    const hdr = document.createElement("div");
    hdr.className = "stepper-details-header";
    const title = document.createElement("span");
    title.textContent = `step ${stepNum} · ${cap}`;
    hdr.appendChild(title);
    const close = document.createElement("button");
    close.className = "stepper-details-close";
    close.type = "button";
    close.textContent = "×";
    close.title = "close";
    close.addEventListener("click", (e) => {
        e.stopPropagation();
        panel.remove();
        wrap.querySelectorAll(".chip.active").forEach(c => c.classList.remove("active"));
    });
    hdr.appendChild(close);
    panel.appendChild(hdr);
    if (chipEl) chipEl.classList.add("active");

    const argsBlock = document.createElement("details");
    argsBlock.open = true;
    const argsSum = document.createElement("summary");
    argsSum.textContent = "args";
    argsBlock.appendChild(argsSum);
    const argsPre = document.createElement("pre");
    argsPre.textContent = JSON.stringify(call.args, null, 2);
    argsBlock.appendChild(argsPre);
    panel.appendChild(argsBlock);

    if (result) {
        const resBlock = document.createElement("details");
        resBlock.open = true;
        const resSum = document.createElement("summary");
        resSum.textContent = result.error ? "error" : "result";
        resBlock.appendChild(resSum);
        const resPre = document.createElement("pre");
        resPre.textContent = result.error
            ? result.error
            : JSON.stringify(result.result, null, 2);
        resBlock.appendChild(resPre);
        panel.appendChild(resBlock);
    }

    wrap.appendChild(panel);
}

function appendBubble(kind, who, text, attachments = []) {
    const div = document.createElement("div");
    div.className = `bubble ${kind}`;
    if (who) {
        const w = document.createElement("span");
        w.className = "who";
        w.textContent = who;
        div.appendChild(w);
    }
    if (text) {
        const body = document.createElement("span");
        body.textContent = text;
        div.appendChild(body);
    }
    appendAttachmentCards(div, attachments);
    conv.appendChild(div);
    conv.scrollTop = conv.scrollHeight;
    return div;
}

function shortPeer(peerId) {
    if (!peerId) return "?";
    const trimmed = peerId.startsWith("n3:") ? peerId.slice(3) : peerId;
    return trimmed.slice(0, 12);
}

// ---------------------------------------------------------------------------
// Actions
// ---------------------------------------------------------------------------

async function newChat() {
    try {
        const r = await api("POST", "/api/v0/conversations", {});
        await loadConversations();
        await selectConversation(r.id);
    } catch (e) {
        appendBubble("error", null, `Create failed: ${e.message}`);
    }
}

async function send() {
    if (inFlight) return;
    if (!activeId) {
        await newChat();
        if (!activeId) return;
    }
    const text = promptEl.value.trim();
    const attachments = draftAttachments.map(a => ({ ...a }));
    if (!text && attachments.length === 0) return;
    promptEl.value = "";
    resizeComposerTextarea();
    clearDraftAttachments();
    inFlight = true;
    updateComposerControls();

    appendBubble("user", "you", text, attachments);
    const mode = getChatMode(activeId);
    const stepper = appendStepper(mode === "direct");

    try {
        await streamDispatch(activeId, text, attachments, stepper, mode);
        // Refresh sidebar (updated_at + auto-title). Skip conv re-render —
        // stepper stays visible alongside the streamed assistant bubble.
        await loadConversations();
    } catch (e) {
        stepper.markError(e.message);
    } finally {
        inFlight = false;
        syncComposerModeUi();
        promptEl.focus();
    }
}

// ---------------------------------------------------------------------------
// Streaming dispatch (SSE)
// ---------------------------------------------------------------------------

function appendStepper(isDirect = false) {
    const wrap = document.createElement("div");
    wrap.className = "stepper";
    if (isDirect) wrap.classList.add("direct-mode");

    const status = document.createElement("div");
    status.className = "stepper-status";
    status.textContent = isDirect ? t("composer.direct.status") : "compiling plan…";
    wrap.appendChild(status);

    const row = document.createElement("div");
    row.className = "stepper-row";
    wrap.appendChild(row);

    conv.appendChild(wrap);
    conv.scrollTop = conv.scrollHeight;

    const chips = new Map();

    function setStatus(text) {
        status.textContent = text;
    }

    function ensureChip(id, peerShort, capability) {
        if (chips.has(id)) return chips.get(id);
        const chip = document.createElement("div");
        chip.className = "chip pending";
        chip.dataset.id = id;
        const num = document.createElement("span");
        num.className = "chip-num";
        num.textContent = chips.size + 1;
        const label = document.createElement("span");
        label.className = "chip-label";
        label.textContent = peerShort && capability
            ? `${peerShort.slice(0, 6)}::${capability}`
            : id;
        chip.appendChild(num);
        chip.appendChild(label);
        row.appendChild(chip);
        chips.set(id, chip);
        return chip;
    }

    function setChipState(id, state) {
        const chip = chips.get(id);
        if (!chip) return;
        chip.classList.remove("pending", "running", "done", "error");
        chip.classList.add(state);
    }

    return {
        renderPlan(steps) {
            row.innerHTML = "";
            chips.clear();
            if (isDirect) {
                wrap.classList.add("no-plan", "direct-mode");
                setStatus(t("composer.direct.status"));
                return;
            }
            if (!steps || steps.length === 0) {
                wrap.classList.add("no-plan");
                setStatus("no plan — answering directly");
                return;
            }
            wrap.classList.remove("no-plan");
            for (const s of steps) {
                ensureChip(s.id, s.peer_short, s.capability);
            }
            setStatus(`plan ready · ${steps.length} step${steps.length > 1 ? "s" : ""}`);
        },
        startStep(id) {
            ensureChip(id);
            setChipState(id, "running");
            setStatus(`running ${id}…`);
        },
        doneStep(id, error) {
            setChipState(id, error ? "error" : "done");
        },
        reflecting() {
            setStatus("composing reply…");
        },
        markLowConfidence(confidence) {
            wrap.classList.add("degraded");
            const pct = typeof confidence === "number"
                ? ` (confidence ${Math.round(confidence * 100)}%)`
                : "";
            setStatus(`low-confidence plan${pct} — result should be checked`);
        },
        finalize(reply, model) {
            setStatus(model ? `done · ${model}` : "done");
            wrap.classList.add("complete");
            if (reply) {
                appendBubble("assistant", model ? `assistant · ${model}` : "assistant", reply);
            }
        },
        markError(msg) {
            setStatus(`error: ${msg}`);
            wrap.classList.add("complete");
            wrap.classList.add("err");
        },
    };
}

async function streamDispatch(convId, message, attachments, stepper, mode = "auto") {
    const body = { message, attachments, mode };
    if (mode === "direct") {
        const model = directModelEl?.value?.trim();
        if (model) body.model = model;
    }
    const res = await fetch(
        `/api/v0/conversations/${encodeURIComponent(convId)}/messages/stream`,
        {
            method: "POST",
            credentials: "same-origin",
            headers: { "content-type": "application/json", accept: "text/event-stream" },
            body: JSON.stringify(body),
        }
    );
    if (!res.ok || !res.body) {
        const text = await res.text().catch(() => "");
        throw new Error(text || `HTTP ${res.status}`);
    }
    const reader = res.body.getReader();
    const decoder = new TextDecoder("utf-8");
    let buf = "";
    while (true) {
        const { value, done } = await reader.read();
        if (done) break;
        buf += decoder.decode(value, { stream: true });
        // Frames are separated by blank lines.
        let idx;
        while ((idx = buf.indexOf("\n\n")) !== -1) {
            const frame = buf.slice(0, idx);
            buf = buf.slice(idx + 2);
            handleSseFrame(frame, stepper);
        }
    }
    // Flush any trailing frame.
    if (buf.trim()) handleSseFrame(buf, stepper);
}

function handleSseFrame(frame, stepper) {
    let event = "message";
    const dataLines = [];
    for (const line of frame.split("\n")) {
        if (line.startsWith("event:")) event = line.slice(6).trim();
        else if (line.startsWith("data:")) dataLines.push(line.slice(5).trim());
        // ignore comments / id / retry
    }
    if (dataLines.length === 0) return;
    let payload;
    try {
        payload = JSON.parse(dataLines.join("\n"));
    } catch {
        return;
    }
    switch (event) {
        case "plan_ready":
            stepper.renderPlan(payload.steps || []);
            break;
        case "low_confidence":
            stepper.markLowConfidence(payload.confidence);
            break;
        case "step_start":
            stepper.startStep(payload.id);
            break;
        case "step_done":
            stepper.doneStep(payload.id, payload.error);
            break;
        case "reflecting":
            stepper.reflecting();
            break;
        case "final":
            stepper.finalize(payload.reply, payload.model);
            break;
        case "error":
            stepper.markError(payload.message || "dispatch failed");
            break;
    }
}

async function renameActive() {
    if (!activeId) return;
    const current = titleEl.textContent;
    const next = window.prompt("Rename conversation:", current === "(untitled)" ? "" : current);
    if (next === null) return;
    try {
        await api("PATCH", `/api/v0/conversations/${encodeURIComponent(activeId)}`, { title: next });
        await loadConversations();
        await renderActive();
    } catch (e) {
        appendBubble("error", null, `Rename failed: ${e.message}`);
    }
}

async function deleteActive() {
    if (!activeId) return;
    if (!(await confirmModal(`Delete this conversation?`, { title: "Delete conversation", okLabel: "Delete", danger: true }))) return;
    try {
        await api("DELETE", `/api/v0/conversations/${encodeURIComponent(activeId)}`);
        localStorage.removeItem(LS_CURRENT);
        activeId = null;
        await loadConversations();
        await renderActive();
    } catch (e) {
        appendBubble("error", null, `Delete failed: ${e.message}`);
    }
}

// ---------------------------------------------------------------------------
// Wire-up
// ---------------------------------------------------------------------------

newBtn.addEventListener("click", newChat);
renameBtn.addEventListener("click", renameActive);
deleteBtn.addEventListener("click", deleteActive);
sendBtn.addEventListener("click", send);
promptEl.addEventListener("input", () => {
    resizeComposerTextarea();
    updateComposerControls();
});
promptEl.addEventListener("keydown", (e) => {
    if (e.key === "Enter" && !e.shiftKey && !e.isComposing) {
        e.preventDefault();
        send();
    }
});
composerMenuBtn?.addEventListener("click", (e) => {
    e.stopPropagation();
    toggleComposerMenu();
});
composerAttachBtn?.addEventListener("click", (e) => {
    e.stopPropagation();
    toggleComposerMenu(false);
    composerFileInput?.click();
});
document.addEventListener("click", () => toggleComposerMenu(false));
composerMenuPopover?.addEventListener("click", (e) => e.stopPropagation());

(async () => {
    await loadConversations();
    if (activeId) {
        await renderActive();
    }
})();

// ---------------------------------------------------------------------------
// Sidebar tabs (Chats / Network / Skills) + Inspector overlay
// ---------------------------------------------------------------------------
//
// Network + Skills panels show compact 1-line entries with a text filter.
// Clicking an entry opens an Inspector pane that slides over the chat in
// the main pane (chat state is preserved underneath). Cross-links: a peer
// detail lists its caps as chips → click → cap detail; a cap detail lists
// every peer exposing it → click → peer detail.

function shortId(id) {
    if (!id) return "?";
    const trimmed = id.startsWith("n3:") ? id.slice(3) : id;
    return trimmed.slice(0, 12);
}

/** Strip mistaken /api/generate or /v1 suffixes from OpenAI-compat base URLs. */
function normalizeOpenaiBaseUrl(url) {
    let s = url.trim().replace(/\/+$/, "");
    for (;;) {
        const before = s;
        for (const suffix of ["/v1/chat/completions", "/api/generate", "/v1"]) {
            if (s.endsWith(suffix)) {
                s = s.slice(0, -suffix.length).replace(/\/+$/, "");
            }
        }
        if (s === before) break;
    }
    return s;
}

function escapeHtml(s) {
    if (s === null || s === undefined) return "";
    return String(s)
        .replace(/&/g, "&amp;")
        .replace(/</g, "&lt;")
        .replace(/>/g, "&gt;")
        .replace(/"/g, "&quot;");
}

// Match a cap declaration against the combined "type" filter dropdown.
// Filter value is `""` (no filter), `mode:<wire-literal>`, or
// `bind:<prompt|mcp|http>`. Binding-type filters skip caps without a
// known `binding_type` (i.e. remote-only entries) since we can't tell.
function matchesTypeFilter(decl, filterValue) {
    if (!filterValue) return true;
    if (filterValue.startsWith("mode:")) {
        return decl.mode === filterValue.slice(5);
    }
    if (filterValue.startsWith("bind:")) {
        const want = filterValue.slice(5);
        return decl.binding_type === want;
    }
    return true;
}

// UI label for an AccessMode wire literal. Wire stays "free" for backward
// compat; users see "Public". Returns empty string for unknown / missing
// modes so the sidebar sub-line doesn't render a bare "?" placeholder.
function modeLabel(mode) {
    if (mode === "free") return t("cap.badge.public");
    if (mode === "restricted") return t("cap.badge.restricted");
    if (mode === "private") return t("cap.badge.private");
    return "";
}

// Caches kept in memory so the inspector can cross-link without re-fetching.
let _peersCache = { self: null, peers: [] };
let _capsCache = { self: null, caps: [] };

async function refreshNetwork() {
    try {
        const d = await api("GET", "/api/v0/peers");
        _peersCache = { self: d.self || "?", peers: d.peers || [] };
    } catch (e) {
        _peersCache = { self: "?", peers: [] };
        document.getElementById("network-stats").textContent = `error: ${e.message}`;
        document.getElementById("network-list").innerHTML = "";
        return;
    }
    renderNetworkList();
}

function renderNetworkList() {
    const filter = (document.getElementById("network-filter")?.value || "").toLowerCase();
    const list = document.getElementById("network-list");
    const stats = document.getElementById("network-stats");
    const peers = _peersCache.peers;

    const filtered = peers.filter(p => {
        if (!filter) return true;
        const hay = [
            p.instance_id,
            p.endpoint,
            p.alias || "",
            ...(p.capabilities || []).flatMap(c => [c.name, c.description || ""]),
        ].join(" ").toLowerCase();
        return hay.includes(filter);
    });

    const uniqueCaps = new Set();
    peers.forEach(p => (p.capabilities || []).forEach(c => uniqueCaps.add(c.name)));
    stats.textContent = `${peers.length} peers · ${uniqueCaps.size} unique caps · self ${shortId(_peersCache.self)}`;

    let html = "";
    if (filtered.length === 0) {
        html = '<li class="empty">no match</li>';
    } else {
        html = filtered.map(p => {
            const caps = (p.capabilities || []).length;
            const sub = `${p.endpoint}${p.alias ? " · " + p.alias : ""}`;
            return `
                <li data-peer="${escapeHtml(p.instance_id)}">
                    <div class="row-main">
                        <span class="name">${escapeHtml(shortId(p.instance_id))}</span>
                        <span class="row-sub">${escapeHtml(sub)}</span>
                    </div>
                    <span class="row-count" title="${caps} cap${caps !== 1 ? "s" : ""}">${caps}</span>
                </li>
            `;
        }).join("");
    }
    list.innerHTML = html;
    list.querySelectorAll("li[data-peer]").forEach(li => {
        li.addEventListener("click", () => openPeerInspector(li.dataset.peer));
    });
}

async function refreshSkills() {
    // Skills tab shows EVERY cap reachable from this node: the local
    // registry + every peer's cached describe_self. Pulls both endpoints
    // in parallel and merges into a unified view.
    try {
        const [caps, peers] = await Promise.all([
            api("GET", "/api/v0/caps"),
            api("GET", "/api/v0/peers"),
        ]);
        _capsCache = { self: caps.self || "?", caps: caps.caps || [] };
        _peersCache = { self: peers.self || "?", peers: peers.peers || [] };
    } catch (e) {
        document.getElementById("skills-stats").textContent = `error: ${e.message}`;
        document.getElementById("skills-list").innerHTML = "";
        return;
    }
    renderSkillsList();
}

/// Build the merged catalog: union of local caps + every peer's caps,
/// deduplicated by name. Each entry records which peers expose it.
function buildMergedCatalog() {
    const merged = new Map(); // name → { decl, sources: Array<{kind, peer_id, endpoint}> }

    // Local caps first — preserves the local descriptor as canonical.
    for (const c of _capsCache.caps) {
        merged.set(c.name, {
            decl: c,
            sources: [{
                kind: c.has_binding ? "manifest" : "legacy",
                peer_id: _capsCache.self,
                endpoint: "local",
            }],
        });
    }

    // Then peers. If the cap name is new → use the remote decl as
    // canonical. If already in the map → just append the peer as a
    // source.
    for (const p of _peersCache.peers) {
        for (const remote of (p.capabilities || [])) {
            const entry = merged.get(remote.name);
            if (entry) {
                entry.sources.push({
                    kind: "remote",
                    peer_id: p.instance_id,
                    endpoint: p.endpoint,
                });
            } else {
                merged.set(remote.name, {
                    decl: remote,
                    sources: [{
                        kind: "remote",
                        peer_id: p.instance_id,
                        endpoint: p.endpoint,
                    }],
                });
            }
        }
    }

    return Array.from(merged.values());
}

function renderSkillsList() {
    const filter = (document.getElementById("skills-filter")?.value || "").toLowerCase();
    const typeFilter = document.getElementById("skills-type-filter")?.value || "";
    const list = document.getElementById("skills-list");
    const stats = document.getElementById("skills-stats");

    const all = buildMergedCatalog();
    const filtered = all.filter(entry => {
        const c = entry.decl;
        if (filter) {
            const hay = [
                c.name,
                c.description || "",
                ...(c.tags || []),
                ...(c.languages || []),
                ...(c.countries || []),
                ...entry.sources.map(s => s.endpoint),
            ].join(" ").toLowerCase();
            if (!hay.includes(filter)) return false;
        }
        return matchesTypeFilter(c, typeFilter);
    });

    const localCount = all.filter(e => e.sources.some(s => s.kind !== "remote")).length;
    const remoteOnly = all.length - localCount;
    stats.textContent = `${all.length} skills total · ${localCount} local · ${remoteOnly} remote-only`;

    let html = "";
    if (filtered.length === 0) {
        html = '<li class="empty">no match</li>';
    } else {
        html = filtered.map(entry => {
            const c = entry.decl;
            const localSource = entry.sources.find(s => s.kind !== "remote");
            const badgeKind = localSource
                ? (localSource.kind === "manifest" ? "binding" : "legacy")
                : "";
            const badgeText = localSource
                ? (localSource.kind === "manifest" ? "M" : "L")
                : `R${entry.sources.length}`;
            const badgeTitle = localSource
                ? (localSource.kind === "manifest" ? "manifest binding (local)" : "legacy backend (local)")
                : `remote-only · seen on ${entry.sources.length} peer${entry.sources.length !== 1 ? "s" : ""}`;
            const sub = [
                c.version ? `v${c.version}` : "",
                modeLabel(c.mode),
                ...(c.languages || []),
                ...(c.tags || []).slice(0, 3),
            ].filter(Boolean).join(" · ");
            const modeClass = c.mode === "private" ? "mode-private"
                            : c.mode === "restricted" ? "mode-restricted"
                            : "mode-public";
            return `
                <li data-cap="${escapeHtml(c.name)}" class="${modeClass}">
                    <div class="row-main">
                        <span class="name">${escapeHtml(c.name)}</span>
                        <span class="row-sub">${escapeHtml(sub)}</span>
                    </div>
                    <span class="row-count ${badgeKind}" title="${escapeHtml(badgeTitle)}">${escapeHtml(badgeText)}</span>
                </li>
            `;
        }).join("");
    }
    list.innerHTML = html;
    list.querySelectorAll("li[data-cap]").forEach(li => {
        li.addEventListener("click", () => openCapInspector(li.dataset.cap));
    });
}

// ---------------------------------------------------------------------------
// Inspector overlay (replaces chat view temporarily)
// ---------------------------------------------------------------------------

function openInspector(title, html) {
    const overlay = document.getElementById("inspector");
    document.getElementById("inspector-title").textContent = title;
    document.getElementById("inspector-body").innerHTML = html;
    overlay.classList.remove("hidden");
    overlay.setAttribute("aria-hidden", "false");
}

function closeInspector() {
    const overlay = document.getElementById("inspector");
    overlay.classList.add("hidden");
    overlay.setAttribute("aria-hidden", "true");
}

function openPeerInspector(peerId) {
    const peer = _peersCache.peers.find(p => p.instance_id === peerId);
    if (!peer) {
        openInspector("Peer not found", `<div class="section">${escapeHtml(peerId)}</div>`);
        return;
    }
    const caps = peer.capabilities || [];
    const capChips = caps.length
        ? caps.map(c => `<span class="badge" data-cap="${escapeHtml(c.name)}">${escapeHtml(c.name)}</span>`).join("")
        : '<span class="row-sub">no cached caps</span>';

    const html = `
        <section class="section">
            <h3>identity</h3>
            <dl class="kv">
                <dt>instance_id</dt><dd><code>${escapeHtml(peer.instance_id)}</code></dd>
                <dt>endpoint</dt><dd><code>${escapeHtml(peer.endpoint)}</code></dd>
                <dt>alias</dt><dd>${peer.alias ? escapeHtml(peer.alias) : "<em>none</em>"}</dd>
            </dl>
        </section>
        <section class="section">
            <h3>capabilities (${caps.length})</h3>
            <div class="chip-list">${capChips}</div>
        </section>
        ${caps.length ? `
        <section class="section">
            <h3>cap descriptions (cached)</h3>
            ${caps.map(c => `
                <details>
                    <summary><strong>${escapeHtml(c.name)}</strong></summary>
                    <p>${escapeHtml(c.description || "")}</p>
                    <pre>${escapeHtml(JSON.stringify(c.schema_in || {}, null, 2))}</pre>
                </details>
            `).join("")}
        </section>` : ""}
    `;
    openInspector(`peer · ${shortId(peer.instance_id)}`, html);

    // Cross-link: clicking a cap chip jumps to the cap inspector if the
    // cap exists locally (Skills cache). Falls back to a "remote cap"
    // detail rendered from the cached describe_self entry.
    document.querySelectorAll("#inspector-body .badge[data-cap]").forEach(b => {
        b.addEventListener("click", () => {
            const name = b.dataset.cap;
            const local = _capsCache.caps.find(c => c.name === name);
            if (local) {
                openCapInspector(name);
            } else {
                const remote = (peer.capabilities || []).find(c => c.name === name);
                if (remote) openRemoteCapInspector(remote, peer);
            }
        });
    });
}

function openCapInspector(capName) {
    // First look in local cap registry; otherwise pull the first remote
    // declaration we have cached for this name. This makes remote-only
    // caps inspectable from the merged Skills view.
    let cap = _capsCache.caps.find(c => c.name === capName);
    let isLocal = !!cap;
    if (!cap) {
        for (const p of _peersCache.peers) {
            const remote = (p.capabilities || []).find(c => c.name === capName);
            if (remote) {
                cap = remote;
                break;
            }
        }
    }
    if (!cap) {
        openInspector("Skill not found", `<div class="section">${escapeHtml(capName)}</div>`);
        return;
    }
    const peersWithCap = _peersCache.peers.filter(p =>
        (p.capabilities || []).some(c => c.name === cap.name)
    );
    const badgeClass = isLocal
        ? (cap.has_binding ? "binding" : "legacy")
        : "";
    const badgeText = isLocal
        ? (cap.has_binding ? "manifest (local)" : "legacy (local)")
        : `remote · ${peersWithCap.length} peer${peersWithCap.length !== 1 ? "s" : ""}`;

    const html = `
        <section class="section">
            <h3>${escapeHtml(cap.name)} <span class="row-count ${badgeClass}">${escapeHtml(badgeText)}</span></h3>
            <dl class="kv">
                <dt>version</dt><dd>${escapeHtml(cap.version || "?")}</dd>
                <dt>mode</dt><dd>${escapeHtml(modeLabel(cap.mode))}</dd>
                <dt>languages</dt><dd>${(cap.languages || []).join(", ") || "<em>any</em>"}</dd>
                <dt>countries</dt><dd>${(cap.countries || []).join(", ") || "<em>any</em>"}</dd>
                <dt>tags</dt><dd>${(cap.tags || []).join(", ") || "<em>none</em>"}</dd>
                <dt>lobes</dt><dd>${(cap.lobe_ids || []).join(", ") || "<em>none</em>"}</dd>
            </dl>
        </section>
        <section class="section">
            <h3>description</h3>
            <p>${escapeHtml(cap.description || "")}</p>
            ${cap.output_semantic ? `<p><strong>output means:</strong> ${escapeHtml(cap.output_semantic)}</p>` : ""}
            ${cap.disambiguation ? `<p><strong>disambiguation:</strong> ${escapeHtml(cap.disambiguation)}</p>` : ""}
        </section>
        ${(cap.examples || []).length ? `
        <section class="section">
            <h3>examples</h3>
            ${cap.examples.map(ex => `
                <details>
                    <summary>"${escapeHtml(ex.user_intent)}"</summary>
                    <pre>${escapeHtml(JSON.stringify({args: ex.args, expected_output: ex.expected_output}, null, 2))}</pre>
                </details>
            `).join("")}
        </section>` : ""}
        ${(cap.negative_examples || []).length ? `
        <section class="section">
            <h3>do NOT use for</h3>
            <ul>${cap.negative_examples.map(ne =>
                `<li><strong>"${escapeHtml(ne.user_intent)}"</strong> — ${escapeHtml(ne.why_not)}</li>`
            ).join("")}</ul>
        </section>` : ""}
        <section class="section">
            <h3>schemas</h3>
            <details><summary>schema_in</summary><pre>${escapeHtml(JSON.stringify(cap.schema_in || {}, null, 2))}</pre></details>
            <details><summary>schema_out</summary><pre>${escapeHtml(JSON.stringify(cap.schema_out || {}, null, 2))}</pre></details>
        </section>
        <section class="section">
            <h3>exposed by ${peersWithCap.length} peer${peersWithCap.length !== 1 ? "s" : ""} (network view)</h3>
            <div class="chip-list">
                ${peersWithCap.length
                    ? peersWithCap.map(p => `<span class="badge" data-peer="${escapeHtml(p.instance_id)}">${escapeHtml(shortId(p.instance_id))}</span>`).join("")
                    : '<span class="row-sub">no peers cached with this cap</span>'}
            </div>
        </section>
    `;
    openInspector(`skill · ${cap.name}`, html);
    document.querySelectorAll("#inspector-body .badge[data-peer]").forEach(b => {
        b.addEventListener("click", () => openPeerInspector(b.dataset.peer));
    });
}

function openRemoteCapInspector(cap, peer) {
    const html = `
        <section class="section">
            <h3>${escapeHtml(cap.name)} <span class="row-count">remote</span></h3>
            <dl class="kv">
                <dt>seen on</dt><dd><code>${escapeHtml(peer.instance_id)}</code> · ${escapeHtml(peer.endpoint)}</dd>
            </dl>
        </section>
        <section class="section">
            <h3>description</h3>
            <p>${escapeHtml(cap.description || "")}</p>
        </section>
        <section class="section">
            <h3>schema_in (cached)</h3>
            <pre>${escapeHtml(JSON.stringify(cap.schema_in || {}, null, 2))}</pre>
        </section>
    `;
    openInspector(`skill · ${cap.name} @ ${shortId(peer.instance_id)}`, html);
}

let _lastNonSettingsSection = "chats";

function activateSection(section) {
    if (section !== "settings") _lastNonSettingsSection = section;

    document.body.dataset.section = section;

    document.querySelectorAll("#app-rail .rail-btn").forEach(btn =>
        btn.classList.toggle("active", btn.dataset.section === section)
    );

    document.querySelectorAll(".context-section").forEach(el => {
        const ctx = el.dataset.context;
        if (section === "settings") {
            el.classList.toggle("hidden", ctx !== "settings");
        } else {
            el.classList.toggle("hidden", ctx !== section);
        }
    });

    const chatView = document.getElementById("chat-view");
    const filesPage = document.getElementById("files-page");
    const settingsPage = document.getElementById("settings-page");
    const workspaceEmpty = document.getElementById("workspace-empty");
    const workspaceHint = document.getElementById("workspace-empty-hint");

    if (section === "chats") {
        chatView?.classList.remove("hidden");
        filesPage?.classList.add("hidden");
        filesPage?.setAttribute("aria-hidden", "true");
        settingsPage?.classList.add("hidden");
        settingsPage?.setAttribute("aria-hidden", "true");
        workspaceEmpty?.classList.add("hidden");
        workspaceEmpty?.setAttribute("aria-hidden", "true");
        closeInspector();
    } else if (section === "files") {
        chatView?.classList.add("hidden");
        filesPage?.classList.remove("hidden");
        filesPage?.setAttribute("aria-hidden", "false");
        settingsPage?.classList.add("hidden");
        settingsPage?.setAttribute("aria-hidden", "true");
        workspaceEmpty?.classList.add("hidden");
        workspaceEmpty?.setAttribute("aria-hidden", "true");
        closeInspector();
        refreshFiles();
    } else if (section === "settings") {
        chatView?.classList.add("hidden");
        filesPage?.classList.add("hidden");
        filesPage?.setAttribute("aria-hidden", "true");
        settingsPage?.classList.remove("hidden");
        settingsPage?.setAttribute("aria-hidden", "false");
        workspaceEmpty?.classList.add("hidden");
        workspaceEmpty?.setAttribute("aria-hidden", "true");
        closeInspector();
    } else if (section === "network" || section === "skills") {
        chatView?.classList.add("hidden");
        filesPage?.classList.add("hidden");
        filesPage?.setAttribute("aria-hidden", "true");
        settingsPage?.classList.add("hidden");
        settingsPage?.setAttribute("aria-hidden", "true");
        workspaceEmpty?.classList.remove("hidden");
        workspaceEmpty?.setAttribute("aria-hidden", "false");
        if (workspaceHint) {
            workspaceHint.innerHTML = section === "network"
                ? t("workspace.hint.network")
                : t("workspace.hint.skills");
        }
        if (section === "network") refreshNetwork();
        if (section === "skills") refreshSkills();
    }
}

let _filesCache = [];
let _filesCategory = "all";

function setFilesCategory(category) {
    _filesCategory = category;
    document.querySelectorAll("#files-nav .files-nav-item").forEach(el =>
        el.classList.toggle("active", el.dataset.category === category)
    );
    renderFilesList();
}

/** Blob class A/B/D per n3ur0n-blob-protocol-v0 §2.4 (C excluded from user panel). */
function blobClass(f) {
    const anchor = f.anchor_kind || "";
    const prov = f.provenance || "";
    const role = f.role || "";
    if (anchor === "local_cache") return "D";
    if (prov === "outbound" && role === "input" && anchor === "user_session") return "A";
    if (prov === "inbound" && role === "output" && anchor === "user_session") return "B";
    return null;
}

function formatBlobClass(f) {
    const cls = blobClass(f);
    if (cls) return t(`files.class.${cls}`);
    return formatProvenance(f.provenance);
}

function matchesFileCategory(f, category) {
    if (category === "all") return true;
    const cls = blobClass(f);
    if (category === "class_a") return cls === "A";
    if (category === "class_b") return cls === "B";
    if (category === "class_d") return cls === "D";
    return true;
}

async function refreshFiles() {
    try {
        const d = await api("GET", "/api/v0/files");
        _filesCache = d?.files || [];
    } catch (e) {
        _filesCache = [];
        const body = document.getElementById("files-page-body");
        if (body) {
            body.innerHTML = `<div class="empty-state"><div class="empty-icon">${iconHtml("alert-triangle", { size: 28 })}</div><p class="empty-title">load failed</p><p class="empty-body">${escapeHtml(e.message)}</p></div>`;
        }
        return;
    }
    renderFilesList();
}

function formatProvenance(provenance) {
    if (provenance === "outbound") return t("files.direction.sent");
    if (provenance === "inbound") return t("files.direction.received");
    return provenance || "—";
}

function formatExpires(expiresAt) {
    if (!expiresAt) return "—";
    try {
        const d = new Date(expiresAt);
        return d.toLocaleString(undefined, { dateStyle: "short", timeStyle: "short" });
    } catch {
        return expiresAt;
    }
}

function formatBytes(n) {
    if (n < 1024) return `${n} B`;
    if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
    return `${(n / (1024 * 1024)).toFixed(1)} MB`;
}

function fileCard(f) {
    const mime = f.mime || "application/octet-stream";
    const short = f.hash?.replace(/^sha256:/, "").slice(0, 12) + "…";
    const status = f.processing_status || "—";
    const cap = f.capability || "—";
    return `
        <article class="card" data-hash="${escapeHtml(f.hash)}">
            <div class="card-head">
                <div class="card-icon">${iconHtml(mimeIcon(mime), { size: 18 })}</div>
                <span class="card-title">${escapeHtml(mime)}</span>
                <span class="card-kind">${escapeHtml(formatBytes(f.size || 0))}</span>
            </div>
            <div class="card-meta">
                <code>${escapeHtml(short)}</code>
            </div>
            <div class="card-meta" style="color: var(--text); font-size: 0.78rem;">
                ${escapeHtml(formatBlobClass(f))} · ${escapeHtml(status)}
                ${cap !== "—" ? ` · ${escapeHtml(cap)}` : ""}
            </div>
            ${f.expires_at ? `<div class="card-meta">${escapeHtml(t("files.col.expires"))}: ${escapeHtml(formatExpires(f.expires_at))}</div>` : ""}
            <div class="card-actions">
                <button type="button" data-action="download">${escapeHtml(t("sidebar.files.download"))}</button>
                ${f.user_deletable !== false ? `<button type="button" data-action="delete" class="danger">${escapeHtml(t("sidebar.files.delete"))}</button>` : ""}
            </div>
        </article>
    `;
}

function renderFilesList() {
    const filter = (document.getElementById("files-filter")?.value || "").toLowerCase();
    const body = document.getElementById("files-page-body");
    const subtitle = document.getElementById("files-page-subtitle");
    if (!body) return;

    const filtered = _filesCache.filter(f => {
        if (!matchesFileCategory(f, _filesCategory)) return false;
        if (!filter) return true;
        const hay = [
            f.hash, f.mime, f.provenance, f.role, f.processing_status || "",
            f.capability || "", f.expires_at || "",
        ].join(" ").toLowerCase();
        return hay.includes(filter);
    });

    if (subtitle) {
        const catKey = `files.nav.${_filesCategory}`;
        const catLabel = t(catKey);
        subtitle.textContent = `${catLabel} · ${t("files.subtitle_count", {
            shown: filtered.length,
            total: _filesCache.length,
        })}`;
    }

    if (filtered.length === 0) {
        body.innerHTML = `
            <div class="empty-state">
                <div class="empty-icon">${iconHtml("folder", { size: 28 })}</div>
                <p class="empty-title">${escapeHtml(t("sidebar.files.empty"))}</p>
                <p class="empty-body">${escapeHtml(t("files.empty.body"))}</p>
                <button class="primary" id="empty-files-upload">${escapeHtml(t("sidebar.files.upload"))}</button>
            </div>
        `;
        document.getElementById("empty-files-upload")?.addEventListener("click", () => {
            document.getElementById("files-input")?.click();
        });
        return;
    }

    body.innerHTML = `<div class="card-grid">${filtered.map(fileCard).join("")}</div>`;
    body.querySelectorAll('.card [data-action="download"]').forEach(btn => {
        btn.addEventListener("click", async (e) => {
            e.stopPropagation();
            const hash = btn.closest(".card")?.dataset.hash;
            if (!hash) return;
            try {
                await downloadFile(hash);
            } catch (err) {
                alert(err.message);
            }
        });
    });
    body.querySelectorAll('.card [data-action="delete"]').forEach(btn => {
        btn.addEventListener("click", async (e) => {
            e.stopPropagation();
            const hash = btn.closest(".card")?.dataset.hash;
            if (!hash) return;
            if (!confirm(t("sidebar.files.deleteConfirm"))) return;
            try {
                await api("DELETE", `/api/v0/files/${encodeURIComponent(hash)}`);
                await refreshFiles();
            } catch (err) {
                alert(err.message);
            }
        });
    });
}

async function downloadFile(hash) {
    const res = await fetch(`/api/v0/files/${encodeURIComponent(hash)}`, { credentials: "same-origin" });
    if (!res.ok) throw new Error(`${res.status}`);
    const blob = await res.blob();
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = url;
    a.download = hash.replace(/^sha256:/, "").slice(0, 16);
    a.click();
    URL.revokeObjectURL(url);
}

async function uploadFileBlob(file) {
    const res = await fetch("/api/v0/files", {
        method: "POST",
        credentials: "same-origin",
        headers: { "content-type": file.type || "application/octet-stream" },
        body: file,
    });
    if (!res.ok) {
        const text = await res.text();
        let msg = text;
        try { msg = JSON.parse(text)?.error || text; } catch { /* ignore */ }
        throw new Error(msg);
    }
    return res.json();
}

async function uploadFiles(fileList) {
    for (const file of fileList) {
        await uploadFileBlob(file);
    }
    await refreshFiles();
}

async function uploadDraftFiles(fileList) {
    for (const file of fileList) {
        const meta = await uploadFileBlob(file);
        const att = {
            hash: meta.hash,
            mime: meta.mime || file.type || "application/octet-stream",
            size: meta.size ?? file.size,
            name: file.name,
        };
        if (!draftAttachments.some(a => a.hash === att.hash)) {
            draftAttachments.push(att);
        }
    }
    renderComposerDraft();
}

// ---------------------------------------------------------------------------
// Settings — master/detail: sidebar lists sections, main pane renders the
// selected section as a rich page (card grids, friendly empty states).
// ---------------------------------------------------------------------------

function openSettings() {
    activateSection("settings");
    activateSettingsSection("backends");
}

function closeSettings() {
    activateSection(_lastNonSettingsSection || "chats");
}

function activateSettingsSection(name) {
    document.querySelectorAll("#settings-nav .settings-nav-item").forEach(el =>
        el.classList.toggle("active", el.dataset.section === name)
    );
    const title = document.getElementById("settings-page-title");
    const sub = document.getElementById("settings-page-subtitle");
    const actions = document.getElementById("settings-page-actions");
    const body = document.getElementById("settings-page-body");
    title.textContent = "";
    sub.textContent = "";
    actions.innerHTML = "";
    body.innerHTML = '<div class="empty-state"><div class="empty-icon">⏳</div></div>';

    if (name === "backends") {
        title.textContent = t("settings.backends.title");
        sub.textContent = t("settings.backends.subtitle");
        actions.innerHTML = `<button class="primary" id="settings-add-backend">${escapeHtml(t("settings.backends.add"))}</button>`;
        document.getElementById("settings-add-backend")?.addEventListener("click", () => openBackendForm());
        renderBackendsCards();
    } else if (name === "planner") {
        title.textContent = t("settings.planner.title");
        sub.textContent = t("settings.planner.subtitle");
        renderPlannerPage();
    } else if (name === "caps") {
        title.textContent = t("settings.caps.title");
        sub.textContent = t("settings.caps.subtitle");
        actions.innerHTML = `<button class="primary" id="settings-add-cap">${escapeHtml(t("settings.caps.add"))}</button>`;
        document.getElementById("settings-add-cap")?.addEventListener("click", () => openCapTemplatePicker());
        renderCapsCards();
    } else if (name === "gateways") {
        title.textContent = t("settings.gateways.title");
        sub.textContent = t("settings.gateways.subtitle");
        actions.innerHTML = `<button class="primary" id="settings-add-gateway">${escapeHtml(t("settings.gateways.add"))}</button>`;
        document.getElementById("settings-add-gateway")?.addEventListener("click", openGatewayForm);
        renderGatewaysCards();
    } else if (name === "ui") {
        renderUiPage();
    } else if (name === "users") {
        renderUsersPage();
    } else if (name === "identity") {
        renderIdentityPage();
    } else if (name === "about") {
        renderAboutPage();
    }
}

const KIND_ICON = {
    openai_compat: "✦",
    mcp_server: "⌨",
    http_base: "↗",
};
const KIND_LABEL = {
    openai_compat: "LLM endpoint",
    mcp_server: "MCP server",
    http_base: "HTTP API",
};

// ---- Backends section ----

async function renderBackendsCards() {
    const body = document.getElementById("settings-page-body");
    let data;
    try {
        data = await api("GET", "/api/v0/backends");
    } catch (e) {
        body.innerHTML = `<div class="empty-state"><div class="empty-icon">⚠</div><p class="empty-title">/api/v0/backends not available</p><p class="empty-body">${escapeHtml(e.message)}</p></div>`;
        return;
    }
    const backends = data.backends || [];
    if (backends.length === 0) {
        body.innerHTML = `
            <div class="empty-state">
                <div class="empty-icon">⚡</div>
                <p class="empty-title">${escapeHtml(t("settings.backends.empty.title"))}</p>
                <p class="empty-body">${escapeHtml(t("settings.backends.empty.body"))}</p>
                <button class="primary" id="empty-add-backend">${escapeHtml(t("settings.backends.add"))}</button>
            </div>
        `;
        document.getElementById("empty-add-backend")?.addEventListener("click", () => openBackendForm());
        return;
    }
    body.innerHTML = `<div class="card-grid">${backends.map(backendCard).join("")}</div>`;
    body.querySelectorAll('.card[data-backend]').forEach(card => {
        card.addEventListener("click", (e) => {
            if (e.target.closest("[data-action]")) return;
            openBackendForm(card.dataset.backend);
        });
    });
    body.querySelectorAll('.card [data-action="edit"]').forEach(btn => {
        btn.addEventListener("click", (e) => {
            e.stopPropagation();
            openBackendForm(btn.closest(".card").dataset.backend);
        });
    });
    body.querySelectorAll('.card [data-action="delete"]').forEach(btn => {
        btn.addEventListener("click", async (e) => {
            e.stopPropagation();
            const name = btn.closest(".card").dataset.backend;
            if (!(await confirmModal(t("backend.form.delete_confirm", { name }), { title: t("backend.form.delete_title"), okLabel: t("button.delete"), danger: true }))) return;
            try {
                await api("DELETE", `/api/v0/backends/${encodeURIComponent(name)}`);
                await renderBackendsCards();
            } catch (err) {
                await alertModal(`delete failed: ${err.message}`, { title: "Error" });
            }
        });
    });
}

function backendCard(b) {
    if (b.error) {
        return `
            <article class="card" style="border-color: var(--warn);">
                <div class="card-head">
                    <div class="card-icon" style="color: var(--warn);">⚠</div>
                    <span class="card-title">malformed manifest</span>
                </div>
                <div class="card-meta">${escapeHtml(b.error)}</div>
            </article>
        `;
    }
    const d = b.details || {};
    const icon = KIND_ICON[b.kind] || "•";
    const label = KIND_LABEL[b.kind] || b.kind;
    let meta = "";
    if (b.kind === "openai_compat") {
        meta = `
            <div class="card-meta">
                model · <code>${escapeHtml(d.default_model || "?")}</code><br>
                endpoint · <code>${escapeHtml(d.base_url || "?")}</code><br>
                api key · ${d.has_api_key ? "configured 🔑" : '<span style="color: var(--muted);">none</span>'}
            </div>`;
    } else if (b.kind === "mcp_server") {
        meta = `
            <div class="card-meta">
                transport · ${escapeHtml(d.transport || "?")}<br>
                command · <code>${escapeHtml(d.command || "?")}</code><br>
                args · ${d.args_count || 0}
            </div>`;
    } else if (b.kind === "http_base") {
        meta = `
            <div class="card-meta">
                base · <code>${escapeHtml(d.base_url || "?")}</code><br>
                headers · ${d.header_count || 0}
            </div>`;
    }
    return `
        <article class="card" data-backend="${escapeHtml(b.name)}" style="cursor: pointer;">
            <div class="card-head">
                <div class="card-icon">${icon}</div>
                <span class="card-title">${escapeHtml(b.name)}</span>
                <span class="card-kind">${escapeHtml(label)}</span>
            </div>
            ${meta}
            <div class="card-actions">
                <button data-action="edit">Edit</button>
                <button data-action="delete" class="danger">Delete</button>
            </div>
        </article>
    `;
}

// ---- Skills section (read-only cards; composer TBD) ----

async function renderCapsCards() {
    const body = document.getElementById("settings-page-body");
    try {
        const d = await api("GET", "/api/v0/caps");
        const caps = d.caps || [];
        // Keep the inspector cache in sync so clicking a freshly-created
        // skill in this view doesn't hit "Skill not found".
        _capsCache = { self: d.self || _capsCache.self, caps };
        if (caps.length === 0) {
            body.innerHTML = `
                <div class="empty-state">
                    <div class="empty-icon">✦</div>
                    <p class="empty-title">${escapeHtml(t("settings.caps.empty.title"))}</p>
                    <p class="empty-body">${escapeHtml(t("settings.caps.empty.body"))}</p>
                    <button class="primary" id="empty-add-cap">${escapeHtml(t("settings.caps.add"))}</button>
                </div>
            `;
            document.getElementById("empty-add-cap")?.addEventListener("click", () => openCapTemplatePicker());
            return;
        }
        const typeFilterValue = body.dataset.typeFilter || "";
        const filteredCaps = caps.filter(c => matchesTypeFilter(c, typeFilterValue));
        const filterOpts = [
            ["", "filter.type.all"],
            ["mode:free", "filter.type.public"],
            ["mode:restricted", "filter.type.restricted"],
            ["mode:private", "filter.type.private"],
            ["bind:prompt", "filter.type.prompt"],
            ["bind:mcp", "filter.type.mcp"],
            ["bind:http", "filter.type.http"],
        ];
        body.innerHTML = `
            <div class="caps-toolbar">
                <select id="caps-type-filter" class="filter type-filter" title="${escapeHtml(t("filter.type.tooltip"))}">
                    ${filterOpts.map(([v, k]) => `<option value="${escapeHtml(v)}"${v === typeFilterValue ? " selected" : ""}>${escapeHtml(t(k))}</option>`).join("")}
                </select>
                <span class="row-sub">${filteredCaps.length}/${caps.length}</span>
            </div>
            <div class="card-grid">${filteredCaps.map(capCard).join("")}</div>
        `;
        document.getElementById("caps-type-filter")?.addEventListener("change", (e) => {
            body.dataset.typeFilter = e.target.value;
            renderCapsCards();
        });
        body.querySelectorAll(".card[data-cap]").forEach(c => {
            // Card click opens inspector unless the click came from an action button.
            c.addEventListener("click", (e) => {
                if (e.target.closest("[data-action]")) return;
                openCapInspector(c.dataset.cap);
            });
        });
        body.querySelectorAll('.card [data-action="edit"]').forEach(btn => {
            btn.addEventListener("click", async (e) => {
                e.stopPropagation();
                openCapForm(btn.closest(".card").dataset.cap);
            });
        });
        body.querySelectorAll('.card [data-action="delete"]').forEach(btn => {
            btn.addEventListener("click", async (e) => {
                e.stopPropagation();
                const name = btn.closest(".card").dataset.cap;
                if (!(await confirmModal(t("cap.form.delete_confirm", { name }), { title: t("cap.form.delete_title"), okLabel: t("button.delete"), danger: true }))) return;
                try {
                    await api("DELETE", `/api/v0/caps/manifests/${encodeURIComponent(name)}`);
                    await renderCapsCards();
                } catch (err) {
                    await alertModal(`delete failed: ${err.message}`, { title: "Error" });
                }
            });
        });
    } catch (e) {
        body.innerHTML = `<div class="empty-state"><div class="empty-icon">⚠</div><p class="empty-title">load failed</p><p class="empty-body">${escapeHtml(e.message)}</p></div>`;
    }
}

function capCard(c) {
    const label = c.has_binding ? "manifest" : "legacy";
    const canEdit = c.has_binding; // legacy backend caps can't be edited via cap.toml CRUD
    const modeClass = c.mode === "private" ? "mode-private"
                    : c.mode === "restricted" ? "mode-restricted"
                    : "mode-public";
    const modeBadgeKey = c.mode === "private" ? "cap.badge.private"
                       : c.mode === "restricted" ? "cap.badge.restricted"
                       : "cap.badge.public";
    return `
        <article class="card ${modeClass}" data-cap="${escapeHtml(c.name)}" style="cursor: pointer;">
            <div class="card-head">
                <div class="card-icon">${c.mode === "private" ? "🔒" : "✦"}</div>
                <span class="card-title">${escapeHtml(c.name)}</span>
                <span class="card-kind mode-badge ${modeClass}">${escapeHtml(t(modeBadgeKey))}</span>
            </div>
            <div class="card-meta">
                v${escapeHtml(c.version || "?")} · ${escapeHtml(label)}
                ${(c.languages || []).length ? ` · ${escapeHtml(c.languages.join(", "))}` : ""}
            </div>
            <div class="card-meta" style="color: var(--text); font-size: 0.84rem; line-height: 1.45;">
                ${escapeHtml((c.description || "").slice(0, 140))}${(c.description || "").length > 140 ? "…" : ""}
            </div>
            ${canEdit ? `
            <div class="card-actions">
                <button data-action="edit">Edit</button>
                <button data-action="delete" class="danger">Delete</button>
            </div>` : ""}
        </article>
    `;
}

// ---- Gateways section ----

async function renderGatewaysCards() {
    const body = document.getElementById("settings-page-body");
    try {
        const d = await api("GET", "/api/v0/peers");
        const peers = d.peers || [];
        if (peers.length === 0) {
            body.innerHTML = `
                <div class="empty-state">
                    <div class="empty-icon">${iconHtml("arrow-left-right", { size: 28 })}</div>
                    <p class="empty-title">${escapeHtml(t("settings.gateways.empty.title"))}</p>
                    <p class="empty-body">${escapeHtml(t("settings.gateways.empty.body"))}</p>
                    <button class="primary" id="empty-add-gateway">${escapeHtml(t("settings.gateways.add"))}</button>
                </div>
            `;
            document.getElementById("empty-add-gateway")?.addEventListener("click", openGatewayForm);
            return;
        }
        body.innerHTML = `<div class="card-grid">${peers.map(gatewayCard).join("")}</div>`;
        body.querySelectorAll(".card[data-peer]").forEach(c => {
            c.addEventListener("click", () => openPeerInspector(c.dataset.peer));
        });
        body.querySelectorAll('.card [data-action="refresh"]').forEach(btn => {
            btn.addEventListener("click", async (e) => {
                e.stopPropagation();
                const card = btn.closest(".card");
                const peerId = card.dataset.peer;
                const peer = peers.find(p => p.instance_id === peerId);
                if (!peer) return;
                btn.textContent = "…";
                try {
                    await api("POST", "/api/v0/peers/refresh", { endpoint: peer.endpoint });
                    await renderGatewaysCards();
                } catch (err) {
                    await alertModal(`refresh failed: ${err.message}`, { title: "Error" });
                    btn.textContent = "Refresh";
                }
            });
        });
    } catch (e) {
        body.innerHTML = `<div class="empty-state"><div class="empty-icon">${iconHtml("alert-triangle", { size: 28 })}</div><p class="empty-title">load failed</p><p class="empty-body">${escapeHtml(e.message)}</p></div>`;
    }
}

function gatewayCard(p) {
    const caps = (p.capabilities || []).length;
    const capNames = (p.capabilities || []).map(c => c.name).slice(0, 4).join(" · ") +
        (caps > 4 ? ` · +${caps - 4}` : "");
    return `
        <article class="card" data-peer="${escapeHtml(p.instance_id)}" style="cursor: pointer;">
            <div class="card-head">
                <div class="card-icon">${iconHtml("arrow-left-right", { size: 18 })}</div>
                <span class="card-title">${escapeHtml(shortId(p.instance_id))}</span>
                <span class="card-kind">${caps} skill${caps !== 1 ? "s" : ""}</span>
            </div>
            <div class="card-meta">
                <code>${escapeHtml(p.endpoint)}</code>
                ${p.alias ? `<br><span style="color:var(--muted);">${escapeHtml(p.alias)}</span>` : ""}
            </div>
            ${capNames ? `<div class="card-meta" style="color: var(--text); font-size: 0.78rem;">${escapeHtml(capNames)}</div>` : ""}
            <div class="card-actions">
                <button data-action="refresh">Refresh</button>
            </div>
        </article>
    `;
}

// ---- Identity + About sections ----

async function renderIdentityPage() {
    const body = document.getElementById("settings-page-body");
    document.getElementById("settings-page-title").textContent = "Identity";
    document.getElementById("settings-page-subtitle").textContent =
        "Your cryptographic identity. Used to sign every call you make. Treat the private key like a password — it lives in the app config dir.";
    let me = { instance_id: "?" };
    try { me = await api("GET", "/api/v0/whoami"); } catch { /* ignore */ }
    body.innerHTML = `
        <article class="card" style="max-width: 720px;">
            <div class="card-head">
                <div class="card-icon">⊙</div>
                <span class="card-title">Instance ID</span>
            </div>
            <div class="card-meta">
                <code style="word-break: break-all; display: block; padding: 8px;">${escapeHtml(me.instance_id || "?")}</code>
            </div>
            <p class="card-meta">
                Derived from your public key. Anyone receiving a signed call from you sees this id.
                Keys live in <code>keys.json</code> in your config dir (file mode 0600).
            </p>
        </article>
    `;
}

async function renderUsersPage() {
    const body = document.getElementById("settings-page-body");
    document.getElementById("settings-page-title").textContent = t("settings.users.title");
    document.getElementById("settings-page-subtitle").textContent = t("settings.users.subtitle");
    let users = [];
    try {
        const r = await api("GET", "/api/v0/users");
        users = r.users || [];
    } catch (e) {
        body.innerHTML = `<div class="empty-state"><div class="empty-icon">⚠</div><p class="empty-title">load failed</p><p class="empty-body">${escapeHtml(e.message)}</p></div>`;
        return;
    }
    const me = auth.state();
    body.innerHTML = `
        <div class="caps-toolbar">
            <button class="primary" id="users-add">${escapeHtml(t("settings.users.add"))}</button>
        </div>
        <div class="card-grid">
            ${users.map(u => `
                <article class="card" data-user="${u.id}">
                    <div class="card-head">
                        <div class="card-icon">👤</div>
                        <span class="card-title">${escapeHtml(u.username)}</span>
                        <span class="card-kind mode-badge mode-${u.role === "admin" ? "private" : u.role === "operator" ? "restricted" : "public"}">${escapeHtml(t("role." + u.role))}</span>
                    </div>
                    <div class="card-meta">id ${u.id}${u.last_login ? " · last login " + new Date(u.last_login*1000).toISOString().slice(0,10) : ""}</div>
                    <div class="card-actions">
                        <button data-action="role">${escapeHtml(t("button.edit"))}</button>
                        ${me.id !== u.id ? `<button data-action="delete" class="danger">${escapeHtml(t("button.delete"))}</button>` : ""}
                    </div>
                </article>
            `).join("")}
        </div>
    `;
    document.getElementById("users-add")?.addEventListener("click", () => openUserCreateForm());
    body.querySelectorAll('.card [data-action="delete"]').forEach(btn => {
        btn.addEventListener("click", async () => {
            const id = btn.closest(".card").dataset.user;
            if (!(await confirmModal(`Delete user?`, { title: t("button.delete"), okLabel: t("button.delete"), danger: true }))) return;
            try {
                await api("DELETE", `/api/v0/users/${id}`);
                await renderUsersPage();
            } catch (e) {
                await alertModal(e.message, { title: "Error" });
            }
        });
    });
    body.querySelectorAll('.card [data-action="role"]').forEach(btn => {
        btn.addEventListener("click", async () => {
            const id = btn.closest(".card").dataset.user;
            const u = users.find(x => String(x.id) === String(id));
            openUserEditForm(u);
        });
    });
}

function openUserCreateForm() {
    const overlay = document.getElementById("inspector");
    document.getElementById("inspector-title").textContent = t("settings.users.add");
    document.getElementById("inspector-body").innerHTML = `
        <section class="section">
            <form class="kv" onsubmit="return false;">
                <label for="uf-name">${escapeHtml(t("auth.field.username"))}</label>
                <input id="uf-name" type="text" required pattern="[A-Za-z0-9._-]{3,32}" />
                <label for="uf-pw">${escapeHtml(t("auth.field.password_new"))}</label>
                <input id="uf-pw" type="password" required minlength="6" />
                <label for="uf-role">Role</label>
                <select id="uf-role">
                    <option value="user">${escapeHtml(t("role.user"))}</option>
                    <option value="operator">${escapeHtml(t("role.operator"))}</option>
                    <option value="admin">${escapeHtml(t("role.admin"))}</option>
                </select>
            </form>
            <div style="display:flex; gap:8px; margin-top:12px; justify-content:flex-end;">
                <button class="icon-btn" id="uf-cancel">${escapeHtml(t("button.cancel"))}</button>
                <button class="primary" id="uf-save">${escapeHtml(t("button.save"))}</button>
            </div>
            <p class="row-sub" id="uf-status"></p>
        </section>
    `;
    overlay.classList.remove("hidden");
    document.getElementById("uf-cancel").addEventListener("click", closeInspector);
    document.getElementById("uf-save").addEventListener("click", async () => {
        const username = document.getElementById("uf-name").value.trim();
        const password = document.getElementById("uf-pw").value;
        const role = document.getElementById("uf-role").value;
        const status = document.getElementById("uf-status");
        try {
            await api("POST", "/api/v0/users", { username, password, role });
            closeInspector();
            await renderUsersPage();
        } catch (e) {
            status.textContent = e.message;
        }
    });
}

function openUserEditForm(u) {
    const overlay = document.getElementById("inspector");
    document.getElementById("inspector-title").textContent = `${t("button.edit")} · ${u.username}`;
    document.getElementById("inspector-body").innerHTML = `
        <section class="section">
            <form class="kv" onsubmit="return false;">
                <label for="ue-role">Role</label>
                <select id="ue-role">
                    <option value="user"${u.role === "user" ? " selected" : ""}>${escapeHtml(t("role.user"))}</option>
                    <option value="operator"${u.role === "operator" ? " selected" : ""}>${escapeHtml(t("role.operator"))}</option>
                    <option value="admin"${u.role === "admin" ? " selected" : ""}>${escapeHtml(t("role.admin"))}</option>
                </select>
                <label for="ue-pw">${escapeHtml(t("auth.field.password_new"))} (optional)</label>
                <input id="ue-pw" type="password" minlength="6" />
            </form>
            <div style="display:flex; gap:8px; margin-top:12px; justify-content:flex-end;">
                <button class="icon-btn" id="ue-cancel">${escapeHtml(t("button.cancel"))}</button>
                <button class="primary" id="ue-save">${escapeHtml(t("button.save_changes"))}</button>
            </div>
            <p class="row-sub" id="ue-status"></p>
        </section>
    `;
    overlay.classList.remove("hidden");
    document.getElementById("ue-cancel").addEventListener("click", closeInspector);
    document.getElementById("ue-save").addEventListener("click", async () => {
        const role = document.getElementById("ue-role").value;
        const pw = document.getElementById("ue-pw").value;
        const status = document.getElementById("ue-status");
        const payload = { role };
        if (pw) payload.password = pw;
        try {
            await api("PATCH", `/api/v0/users/${u.id}`, payload);
            closeInspector();
            await renderUsersPage();
        } catch (e) {
            status.textContent = e.message;
        }
    });
}

async function renderPlannerPage() {
    const body = document.getElementById("settings-page-body");
    const actions = document.getElementById("settings-page-actions");
    actions.innerHTML = "";
    body.innerHTML = '<div class="empty-state"><div class="empty-icon">⏳</div></div>';

    let data;
    try {
        data = await api("GET", "/api/v0/planner");
    } catch (e) {
        body.innerHTML = `<div class="empty-state"><div class="empty-icon">⚠</div><p class="empty-title">${escapeHtml(e.message)}</p></div>`;
        return;
    }

    if (!data.enabled) {
        body.innerHTML = `
            <article class="card" style="max-width: 720px;">
                <p class="row-sub">${escapeHtml(t("settings.planner.disabled"))}</p>
            </article>`;
        return;
    }

    const backends = data.available_backends || [];
    const cfg = data.config || {};
    const active = data.active || {};
    const env = data.env_default || {};
    const selectedBackend = cfg.backend || "";

    const backendOpts = [
        `<option value="">${escapeHtml(t("settings.planner.backend.env"))}</option>`,
        ...backends.map(b => {
            const label = `${b.name} · ${b.default_model}`;
            const sel = b.name === selectedBackend ? " selected" : "";
            return `<option value="${escapeHtml(b.name)}"${sel}>${escapeHtml(label)}</option>`;
        }),
    ].join("");

    const activeLine = active.model
        ? `${active.model}${active.backend ? ` · ${active.backend}` : ""} (${active.source})`
        : "—";

    body.innerHTML = `
        <article class="card" style="max-width: 720px;">
            <div class="card-head">
                <div class="card-icon">${iconHtml("sparkles", { size: 18 })}</div>
                <span class="card-title">${escapeHtml(t("settings.planner.active"))}</span>
            </div>
            <p class="row-sub" style="margin-top: 8px;">${escapeHtml(activeLine)}</p>
            ${env.model ? `<p class="row-sub">${escapeHtml(t("settings.planner.env_default"))}: ${escapeHtml(env.model)}</p>` : ""}
        </article>
        <article class="card" style="max-width: 720px; margin-top: 12px;">
            <form id="planner-form" class="kv" style="margin-top: 8px;">
                <label for="planner-backend">${escapeHtml(t("settings.planner.backend"))}</label>
                <select id="planner-backend" class="select-control">${backendOpts}</select>
                <label for="planner-model">${escapeHtml(t("settings.planner.model"))}</label>
                <input type="text" id="planner-model" class="form-control"
                    value="${escapeHtml(cfg.model || "")}"
                    placeholder="${escapeHtml(t("composer.direct.model_placeholder"))}" />
                <p class="row-sub">${escapeHtml(t("settings.planner.model.help"))}</p>
                ${backends.length === 0 ? `<p class="row-sub">${escapeHtml(t("settings.planner.no_backends"))}</p>` : ""}
                <button type="submit" class="primary" id="planner-save">${escapeHtml(t("settings.planner.save"))}</button>
            </form>
        </article>
    `;

    document.getElementById("planner-form")?.addEventListener("submit", async (e) => {
        e.preventDefault();
        const backend = document.getElementById("planner-backend")?.value || null;
        const model = document.getElementById("planner-model")?.value?.trim() || null;
        try {
            await api("PUT", "/api/v0/planner", {
                backend: backend || null,
                model,
            });
            await alertModal(t("settings.planner.saved"), {
                title: t("settings.planner.title"),
                okLabel: t("button.confirm"),
            });
            await renderPlannerPage();
        } catch (err) {
            await alertModal(err.message, {
                title: "Error",
                okLabel: t("button.confirm"),
            });
        }
    });
    applyIcons();
}

async function renderUiPage() {
    const body = document.getElementById("settings-page-body");
    document.getElementById("settings-page-title").textContent = t("settings.ui.title");
    document.getElementById("settings-page-subtitle").textContent = t("settings.ui.subtitle");

    const themePref = currentThemePref();
    const themeOpts = [
        ["dark",   "settings.ui.theme.dark"],
        ["light",  "settings.ui.theme.light"],
        ["system", "settings.ui.theme.system"],
    ];

    body.innerHTML = `
        <article class="card" style="max-width: 720px;">
            <div class="card-head">
                <div class="card-icon">🌐</div>
                <span class="card-title">${escapeHtml(t("settings.ui.language"))}</span>
            </div>
            <form class="kv" onsubmit="return false;" style="margin-top: 8px;">
                <label for="lang-select">${escapeHtml(t("settings.ui.language"))}</label>
                <select id="lang-select"></select>
            </form>
            <p class="row-sub" style="margin-top: 8px;">${t("settings.ui.language.help")}</p>
        </article>
        <article class="card" style="max-width: 720px; margin-top: 12px;">
            <div class="card-head">
                <div class="card-icon">◐</div>
                <span class="card-title">${escapeHtml(t("settings.ui.theme"))}</span>
            </div>
            <div class="theme-switch" id="theme-switch" role="radiogroup" aria-label="${escapeHtml(t("settings.ui.theme"))}">
                ${themeOpts.map(([val, key]) => `
                    <button type="button" class="theme-opt${themePref === val ? " active" : ""}" data-theme-pref="${val}" role="radio" aria-checked="${themePref === val}">
                        ${escapeHtml(t(key))}
                    </button>
                `).join("")}
            </div>
            <p class="row-sub" style="margin-top: 8px;">${t("settings.ui.theme.help")}</p>
        </article>
    `;

    const sel = document.getElementById("lang-select");
    const locales = await listLocales();
    const cur = currentLocale();
    sel.innerHTML = locales.map(l => {
        const label = l.native_name && l.name && l.native_name !== l.name
            ? `${l.native_name} (${l.code})`
            : `${l.native_name || l.name || l.code} (${l.code})`;
        return `<option value="${escapeHtml(l.code)}"${l.code === cur ? " selected" : ""}>${escapeHtml(label)}</option>`;
    }).join("");
    sel.addEventListener("change", () => setLocale(sel.value));

    document.querySelectorAll("#theme-switch [data-theme-pref]").forEach(btn => {
        btn.addEventListener("click", () => {
            setThemePref(btn.dataset.themePref);
            // Re-render so the active highlight updates without a full
            // section reload.
            document.querySelectorAll("#theme-switch [data-theme-pref]").forEach(b => {
                const on = b.dataset.themePref === btn.dataset.themePref;
                b.classList.toggle("active", on);
                b.setAttribute("aria-checked", on ? "true" : "false");
            });
        });
    });
}

function renderAboutPage() {
    const body = document.getElementById("settings-page-body");
    document.getElementById("settings-page-title").textContent = "About N3UR0N";
    document.getElementById("settings-page-subtitle").textContent =
        "Federated AI gateway. One manifest per skill, signed protocol, optional peer network.";
    body.innerHTML = `
        <article class="card" style="max-width: 720px;">
            <div class="card-head">
                <div class="card-icon">ⓘ</div>
                <span class="card-title">N3UR0N desktop</span>
                <span class="card-kind">v0.3 alpha</span>
            </div>
            <div class="card-meta">
                A consumer client for N3UR0N — connects to local + remote LLMs,
                MCP servers, HTTP APIs and remote n3ur0n peers under a single
                Ed25519-signed protocol. Capabilities live as TOML manifests
                on disk; they can be invoked locally, shared with peers, or
                consumed from peers.
            </div>
            <div class="card-meta">
                Protocol: <code>n3ur0n/0.3</code><br>
                License: Apache-2.0 (planned)<br>
                Source: <code>github.com/&lt;tbd&gt;</code>
            </div>
        </article>
    `;
}

function openGatewayForm() {
    const overlay = document.getElementById("inspector");
    document.getElementById("inspector-title").textContent = "Add gateway (remote n3ur0n peer)";
    document.getElementById("inspector-body").innerHTML = `
        <section class="section">
            <p>Pull <code>describe_self</code> from a remote n3ur0n endpoint and add it to
            your peer directory. The endpoint will be signed-pinged immediately to verify it
            is reachable.</p>
            <form class="kv" onsubmit="return false;">
                <label for="gf-url">endpoint</label>
                <input id="gf-url" type="url" required placeholder="http://node-a:4242 · https://peer.example.com:4242" />
            </form>
            <div style="display: flex; gap: 8px; margin-top: 12px; justify-content: flex-end;">
                <button id="gf-cancel" type="button" class="icon-btn">Cancel</button>
                <button id="gf-save" type="button" class="primary" style="margin: 0;">Add</button>
            </div>
            <div id="gf-status" class="row-sub" style="margin-top: 8px;"></div>
        </section>
    `;
    overlay.classList.remove("hidden");
    overlay.setAttribute("aria-hidden", "false");
    document.getElementById("gf-cancel")?.addEventListener("click", closeInspector);
    document.getElementById("gf-save")?.addEventListener("click", async () => {
        const url = document.getElementById("gf-url").value.trim();
        const status = document.getElementById("gf-status");
        if (!url) { status.textContent = "endpoint required"; return; }
        status.textContent = "adding…";
        try {
            const r = await api("POST", "/api/v0/peers/refresh", { endpoint: url });
            status.textContent = `added · ${r.instance_id || "ok"}`;
            await renderGatewaysList();
            setTimeout(closeInspector, 600);
        } catch (e) {
            status.textContent = `failed: ${e.message}`;
        }
    });
}

// Cap templates: starter presets the user can pick from. Each is a full
// `prefill` shape; the form is then editable.
const CAP_TEMPLATES = {
    blank_prompt: {
        title: "Blank · prompt (LLM)",
        summary: "Empty LLM-backed skill. Fill in everything yourself.",
        binding_kind: "prompt",
        data: {
            name: "", version: "0.1.0", description: "", mode: "free",
            tags: "", languages: "", countries: "",
            disambiguation: "", output_semantic: "",
            schema_in: `{
  "type": "object",
  "required": ["text"],
  "properties": { "text": { "type": "string" } }
}`,
            schema_out: `{
  "type": "object",
  "required": ["result"],
  "properties": { "result": { "type": "string" } }
}`,
            example_intent: "", example_args: '{"text":"hello"}', example_output: '{"result":"…"}',
            prompt: { system: "", user: "", parser: "text", temperature: 0.0, model: "" },
        },
    },
    translator_fr_en: {
        title: "Translator · FR → EN",
        summary: "LLM French→English translator with strict JSON output.",
        binding_kind: "prompt",
        data: {
            name: "translator-fr-en", version: "0.1.0",
            description: "Translate French text into English.",
            mode: "free",
            tags: "translation, language",
            languages: "fr, en", countries: "",
            disambiguation: "Use only for FR→EN translation of natural-language text. Do NOT use for code, table data, or other language pairs.",
            output_semantic: "An English translation faithful to the source register.",
            schema_in: `{
  "type": "object",
  "required": ["text"],
  "properties": { "text": { "type": "string", "minLength": 1 } }
}`,
            schema_out: `{
  "type": "object",
  "required": ["result"],
  "properties": { "result": { "type": "string" } }
}`,
            example_intent: "translate 'bonjour le monde' to English",
            example_args: '{"text":"bonjour le monde"}',
            example_output: '{"result":"hello world"}',
            prompt: {
                system: "You are a French→English translator. Respond strictly as JSON: {\"result\": \"<english translation>\"}. No commentary.",
                user: "Translate to English:\n{{args.text}}",
                parser: "json", temperature: 0.0, model: "",
            },
        },
    },
    text_summarizer: {
        title: "Text summarizer (free-text)",
        summary: "LLM summary in plain text, 2–3 sentences.",
        binding_kind: "prompt",
        data: {
            name: "text-summarizer", version: "0.1.0",
            description: "Summarize a body of text in 2–3 sentences.",
            mode: "free", tags: "summary, writing",
            languages: "", countries: "",
            disambiguation: "Use for ad-hoc text summarization of articles, transcripts, notes. Do NOT use when structured extraction is needed — pick a JSON-output skill instead.",
            output_semantic: "A short prose summary of the input.",
            schema_in: `{
  "type": "object",
  "required": ["text"],
  "properties": { "text": { "type": "string", "minLength": 1 } }
}`,
            schema_out: `{
  "type": "object",
  "required": ["text"],
  "properties": { "text": { "type": "string" } }
}`,
            example_intent: "summarize this article",
            example_args: '{"text":"Long article body here..."}',
            example_output: '{"text":"Two-to-three sentence summary..."}',
            prompt: {
                system: "You write concise 2–3 sentence summaries. Output plain text only.",
                user: "Summarize:\n{{args.text}}",
                parser: "text", temperature: 0.2, model: "",
            },
        },
    },
    fact_extractor_json: {
        title: "Fact extractor (JSON)",
        summary: "LLM-backed structured extraction into JSON.",
        binding_kind: "prompt",
        data: {
            name: "fact-extractor", version: "0.1.0",
            description: "Extract structured facts (entities, dates, amounts) from raw text.",
            mode: "free", tags: "extraction, structured",
            languages: "", countries: "",
            disambiguation: "Use when the caller wants typed fields back from prose. Do NOT use for summarization.",
            output_semantic: "Structured facts pulled from the input, with empty arrays when none found.",
            schema_in: `{
  "type": "object",
  "required": ["text"],
  "properties": { "text": { "type": "string" } }
}`,
            schema_out: `{
  "type": "object",
  "required": ["people", "dates", "amounts"],
  "properties": {
    "people":  { "type": "array", "items": { "type": "string" } },
    "dates":   { "type": "array", "items": { "type": "string" } },
    "amounts": { "type": "array", "items": { "type": "string" } }
  }
}`,
            example_intent: "extract names and dates from this paragraph",
            example_args: '{"text":"Alice met Bob on 2024-03-12 about a $500 invoice."}',
            example_output: '{"people":["Alice","Bob"],"dates":["2024-03-12"],"amounts":["$500"]}',
            prompt: {
                system: "You are an information-extraction engine. Respond ONLY with JSON matching the schema {\"people\":[],\"dates\":[],\"amounts\":[]}. No prose.",
                user: "Extract from:\n{{args.text}}",
                parser: "json", temperature: 0.0, model: "",
            },
        },
    },
    weather_http_get: {
        title: "Weather lookup (HTTP GET)",
        summary: "HTTP-binding skill calling an open-meteo style endpoint.",
        binding_kind: "http",
        data: {
            name: "weather-now", version: "0.1.0",
            description: "Current weather for a lat/lon pair via Open-Meteo.",
            mode: "free", tags: "weather, http",
            languages: "", countries: "",
            disambiguation: "Use only for current weather at known coordinates. Do NOT use for forecasts beyond now or for geocoding.",
            output_semantic: "Current weather block (temperature, wind, time) for the given coordinates.",
            schema_in: `{
  "type": "object",
  "required": ["lat", "lon"],
  "properties": {
    "lat": { "type": "number" },
    "lon": { "type": "number" }
  }
}`,
            schema_out: `{
  "type": "object",
  "required": ["temperature", "windspeed", "time"],
  "properties": {
    "temperature": { "type": "number" },
    "windspeed":   { "type": "number" },
    "time":        { "type": "string" }
  }
}`,
            example_intent: "weather at 48.85, 2.35",
            example_args: '{"lat":48.85,"lon":2.35}',
            example_output: '{"temperature":12.4,"windspeed":7.1,"time":"2026-05-14T12:00"}',
            http: {
                url_template: "/v1/forecast?latitude={{args.lat}}&longitude={{args.lon}}&current_weather=true",
                method: "GET",
                response_path: "$.current_weather",
                timeout_ms: 5000,
                headers: "",
                body_template: "",
            },
        },
    },
    mcp_filesystem_read: {
        title: "MCP filesystem read",
        summary: "MCP-binding skill calling a `read_file` tool on a local MCP server.",
        binding_kind: "mcp",
        data: {
            name: "fs-read", version: "0.1.0",
            description: "Read the contents of a file via an MCP filesystem server.",
            mode: "restricted", tags: "filesystem, mcp",
            languages: "", countries: "",
            disambiguation: "Use only when you need raw file contents from a known path. Restricted by default — caller must be authorized.",
            output_semantic: "Raw text contents of the requested file.",
            schema_in: `{
  "type": "object",
  "required": ["path"],
  "properties": { "path": { "type": "string" } }
}`,
            schema_out: `{
  "type": "object",
  "required": ["content"],
  "properties": { "content": { "type": "string" } }
}`,
            example_intent: "read /etc/hostname",
            example_args: '{"path":"/etc/hostname"}',
            example_output: '{"content":"my-host\\n"}',
            mcp: {
                tool_name: "read_file",
                arg_mapping: '{ "path": "{{args.path}}" }',
                result_mapping: '{ "content": "{{result.content[0].text}}" }',
            },
        },
    },
};

function defaultPromptData() { return CAP_TEMPLATES.blank_prompt.data; }
function defaultHttpData() {
    return {
        ...CAP_TEMPLATES.blank_prompt.data,
        http: {
            url_template: "/path?q={{args.text}}",
            method: "GET",
            response_path: "",
            timeout_ms: 10000,
            headers: "",
            body_template: "",
        },
    };
}
function defaultMcpData() {
    return {
        ...CAP_TEMPLATES.blank_prompt.data,
        mcp: {
            tool_name: "",
            arg_mapping: '{ }',
            result_mapping: '{ }',
        },
    };
}

function openCapTemplatePicker() {
    const overlay = document.getElementById("inspector");
    document.getElementById("inspector-title").textContent = t("cap.template.picker.title");
    document.getElementById("inspector-body").innerHTML = `
        <section class="section">
            <p class="row-sub" style="margin-top: 0;">${escapeHtml(t("cap.template.picker.intro"))}</p>
            <div class="card-grid" id="cf-templates">
                <article class="card" data-template="__blank__" style="cursor: pointer; border-style: dashed;">
                    <div class="card-head">
                        <div class="card-icon">＋</div>
                        <span class="card-title">${escapeHtml(t("cap.template.blank"))}</span>
                        <span class="card-kind">${escapeHtml(t("cap.template.blank.kind"))}</span>
                    </div>
                    <div class="card-meta">${escapeHtml(t("cap.template.blank.summary"))}</div>
                </article>
                ${Object.entries(CAP_TEMPLATES).filter(([k]) => k !== "blank_prompt").map(([key, tpl]) => `
                    <article class="card" data-template="${escapeHtml(key)}" style="cursor: pointer;">
                        <div class="card-head">
                            <div class="card-icon">✦</div>
                            <span class="card-title">${escapeHtml(tpl.title)}</span>
                            <span class="card-kind">${escapeHtml(tpl.binding_kind)}</span>
                        </div>
                        <div class="card-meta">${escapeHtml(tpl.summary)}</div>
                    </article>
                `).join("")}
            </div>
        </section>
        <div style="display: flex; gap: 8px; justify-content: flex-end;">
            <button id="cf-picker-cancel" type="button" class="icon-btn">${escapeHtml(t("button.cancel"))}</button>
        </div>
    `;
    overlay.classList.remove("hidden");
    overlay.setAttribute("aria-hidden", "false");
    document.getElementById("cf-picker-cancel")?.addEventListener("click", closeInspector);
    document.querySelectorAll("#cf-templates [data-template]").forEach(card => {
        card.addEventListener("click", () => {
            const key = card.dataset.template;
            openCapForm(null, key === "__blank__" ? null : key);
        });
    });
}

async function openCapForm(existingName, templateKey) {
    let backends = [];
    let prefill = null;
    let bindingKind = "prompt";
    try {
        const b = await api("GET", "/api/v0/backends");
        backends = (b.backends || []).filter(x => !x.error);
    } catch { /* leave empty */ }

    if (existingName) {
        // Load the raw cap.toml so we can preserve all binding-kind data.
        try {
            const [listResp, rawResp] = await Promise.all([
                api("GET", "/api/v0/caps"),
                api("GET", `/api/v0/caps/manifests/${encodeURIComponent(existingName)}`).catch(() => null),
            ]);
            const cap = (listResp.caps || []).find(c => c.name === existingName);
            if (cap) {
                const parsedToml = rawResp?.toml ? parseCapTomlBindings(rawResp.toml) : null;
                bindingKind = parsedToml?.kind || "prompt";
                prefill = {
                    name: cap.name,
                    version: cap.version || "0.1.0",
                    description: cap.description || "",
                    mode: cap.mode || "free",
                    tags: (cap.tags || []).join(", "),
                    languages: (cap.languages || []).join(", "),
                    countries: (cap.countries || []).join(", "),
                    disambiguation: cap.disambiguation || "",
                    output_semantic: cap.output_semantic || "",
                    schema_in: JSON.stringify(cap.schema_in || {}, null, 2),
                    schema_out: JSON.stringify(cap.schema_out || {}, null, 2),
                    example_intent: (cap.examples && cap.examples[0]?.user_intent) || "",
                    example_args: JSON.stringify((cap.examples && cap.examples[0]?.args) || {}, null, 2),
                    example_output: JSON.stringify((cap.examples && cap.examples[0]?.expected_output) || {}, null, 2),
                    backend: parsedToml?.backend || "",
                    prompt: parsedToml?.prompt || defaultPromptData().prompt,
                    http: parsedToml?.http || defaultHttpData().http,
                    mcp: parsedToml?.mcp || defaultMcpData().mcp,
                };
            }
        } catch { /* ignore */ }
    }
    if (!prefill) {
        const tpl = templateKey ? CAP_TEMPLATES[templateKey] : null;
        if (tpl) {
            const d = tpl.data;
            bindingKind = tpl.binding_kind;
            prefill = {
                name: d.name, version: d.version, description: d.description, mode: d.mode,
                tags: d.tags, languages: d.languages, countries: d.countries,
                disambiguation: d.disambiguation, output_semantic: d.output_semantic,
                schema_in: d.schema_in, schema_out: d.schema_out,
                example_intent: d.example_intent, example_args: d.example_args, example_output: d.example_output,
                backend: "",
                prompt: d.prompt || defaultPromptData().prompt,
                http: d.http || defaultHttpData().http,
                mcp: d.mcp || defaultMcpData().mcp,
            };
        } else {
            prefill = {
                ...defaultPromptData(),
                backend: "",
                http: defaultHttpData().http,
                mcp: defaultMcpData().mcp,
            };
        }
    }

    const overlay = document.getElementById("inspector");
    document.getElementById("inspector-title").textContent = existingName
        ? t("cap.form.edit_title", { name: existingName })
        : (templateKey && CAP_TEMPLATES[templateKey]
            ? t("cap.form.add_title.template", { title: CAP_TEMPLATES[templateKey].title })
            : t("cap.form.add_title"));

    document.getElementById("inspector-body").innerHTML = `
        ${existingName ? "" : `
        <div style="margin-bottom: 10px;">
            <button id="cf-back-templates" type="button" class="icon-btn">${escapeHtml(t("cap.form.back_to_templates"))}</button>
        </div>`}
        <section class="section">
            <h3>${escapeHtml(t("cap.form.section.basics"))}</h3>
            <form class="kv" onsubmit="return false;">
                <label for="cf-name">${escapeHtml(t("cap.form.field.name"))}</label>
                <input id="cf-name" type="text" required pattern="[a-zA-Z0-9_-]+"
                       value="${escapeHtml(prefill.name)}"
                       ${existingName ? "readonly" : ""}
                       placeholder="translator-fr-en, weather-now, legal-summarizer-fr…" />
                <label for="cf-version">${escapeHtml(t("cap.form.field.version"))}</label>
                <input id="cf-version" type="text" required value="${escapeHtml(prefill.version)}"
                       placeholder="semver: 0.1.0" />
                <label for="cf-desc">${escapeHtml(t("cap.form.field.description"))}</label>
                <input id="cf-desc" type="text" required value="${escapeHtml(prefill.description)}"
                       placeholder="One sentence: what does this skill do?" />
                <label for="cf-mode">${escapeHtml(t("cap.form.field.mode"))}</label>
                <select id="cf-mode" title="${escapeHtml(t("cap.form.field.mode.tooltip"))}">
                    <option value="free"${prefill.mode === "free" ? " selected" : ""}>${escapeHtml(t("cap.form.field.mode.public"))}</option>
                    <option value="restricted"${prefill.mode === "restricted" ? " selected" : ""}>${escapeHtml(t("cap.form.field.mode.restricted"))}</option>
                    <option value="private"${prefill.mode === "private" ? " selected" : ""}>${escapeHtml(t("cap.form.field.mode.private"))}</option>
                </select>
                <label for="cf-langs">${escapeHtml(t("cap.form.field.languages"))}</label>
                <input id="cf-langs" type="text" value="${escapeHtml(prefill.languages)}"
                       placeholder="fr, en  (BCP 47, comma-separated)" />
                <label for="cf-countries">${escapeHtml(t("cap.form.field.countries"))}</label>
                <input id="cf-countries" type="text" value="${escapeHtml(prefill.countries)}"
                       placeholder="FR, BE, CH  (ISO 3166-1 alpha-2)" />
                <label for="cf-tags">${escapeHtml(t("cap.form.field.tags"))}</label>
                <input id="cf-tags" type="text" value="${escapeHtml(prefill.tags)}"
                       placeholder="translation, language, fr…" />
            </form>
        </section>
        <section class="section">
            <h3>${escapeHtml(t("cap.form.section.disambig"))}</h3>
            <textarea id="cf-disambig" class="form-textarea" rows="3"
                      placeholder="${escapeHtml(t("cap.form.field.disambig.placeholder"))}">${escapeHtml(prefill.disambiguation)}</textarea>
            <label for="cf-outsem" style="color: var(--muted); font-size: 0.72rem; display: block; margin-top: 10px;">${escapeHtml(t("cap.form.field.output_semantic"))}</label>
            <textarea id="cf-outsem" class="form-textarea" rows="2">${escapeHtml(prefill.output_semantic)}</textarea>
        </section>
        <section class="section">
            <h3>${escapeHtml(t("cap.form.section.binding"))}</h3>
            <form class="kv" onsubmit="return false;">
                <label for="cf-bindkind">${escapeHtml(t("cap.form.field.binding_type"))}</label>
                <select id="cf-bindkind" ${existingName ? `disabled title="${escapeHtml(t("cap.form.field.binding.locked"))}"` : ""}>
                    <option value="prompt"${bindingKind === "prompt" ? " selected" : ""}>${escapeHtml(t("cap.form.field.binding.prompt"))}</option>
                    <option value="mcp"${bindingKind === "mcp" ? " selected" : ""}>${escapeHtml(t("cap.form.field.binding.mcp"))}</option>
                    <option value="http"${bindingKind === "http" ? " selected" : ""}>${escapeHtml(t("cap.form.field.binding.http"))}</option>
                </select>
                <label for="cf-backend">${escapeHtml(t("cap.form.field.backend"))}</label>
                <select id="cf-backend" required></select>
            </form>
            <div id="cf-bind-prompt" class="hidden" style="margin-top: 10px;">
                <form class="kv" onsubmit="return false;">
                    <label for="cf-parser">${escapeHtml(t("cap.form.field.parser"))}</label>
                    <select id="cf-parser">
                        <option value="text">${escapeHtml(t("cap.form.field.parser.text"))}</option>
                        <option value="json">${escapeHtml(t("cap.form.field.parser.json"))}</option>
                    </select>
                    <label for="cf-temp">${escapeHtml(t("cap.form.field.temperature"))}</label>
                    <input id="cf-temp" type="number" step="0.1" min="0" max="2" value="${prefill.prompt?.temperature ?? 0.0}" />
                    <label for="cf-model">${escapeHtml(t("cap.form.field.model"))}</label>
                    <input id="cf-model" type="text" value="${escapeHtml(prefill.prompt?.model || "")}" placeholder="${escapeHtml(t("cap.form.field.model.placeholder"))}" />
                </form>
                <label for="cf-sysprompt" style="color: var(--muted); font-size: 0.72rem; display: block; margin-top: 10px;">${escapeHtml(t("cap.form.field.system_prompt"))}</label>
                <textarea id="cf-sysprompt" class="form-textarea" rows="6"
                          placeholder="You are a French→English translator. Respond strictly as JSON: {&quot;result&quot;: &quot;...&quot;}.">${escapeHtml(prefill.prompt?.system || "")}</textarea>
                <label for="cf-usertpl" style="color: var(--muted); font-size: 0.72rem; display: block; margin-top: 10px;">${t("cap.form.field.user_template")}</label>
                <textarea id="cf-usertpl" class="form-textarea" rows="3"
                          placeholder="Translate to English: {{args.text}}">${escapeHtml(prefill.prompt?.user || "")}</textarea>
            </div>
            <div id="cf-bind-http" class="hidden" style="margin-top: 10px;">
                <form class="kv" onsubmit="return false;">
                    <label for="cf-http-method">${escapeHtml(t("cap.form.field.http.method"))}</label>
                    <select id="cf-http-method">
                        ${["GET","POST","PUT","DELETE"].map(m => `<option value="${m}"${(prefill.http?.method || "GET") === m ? " selected" : ""}>${m}</option>`).join("")}
                    </select>
                    <label for="cf-http-url">${escapeHtml(t("cap.form.field.http.url_template"))}</label>
                    <input id="cf-http-url" type="text" value="${escapeHtml(prefill.http?.url_template || "")}" placeholder="/v1/items/{{args.id}}" />
                    <label for="cf-http-timeout">${escapeHtml(t("cap.form.field.http.timeout_ms"))}</label>
                    <input id="cf-http-timeout" type="number" min="0" value="${prefill.http?.timeout_ms ?? ""}" placeholder="(default 30000)" />
                    <label for="cf-http-respath">${escapeHtml(t("cap.form.field.http.response_path"))}</label>
                    <input id="cf-http-respath" type="text" value="${escapeHtml(prefill.http?.response_path || "")}" placeholder="$.data.result" />
                </form>
                <label for="cf-http-headers" style="color: var(--muted); font-size: 0.72rem; display: block; margin-top: 10px;">${escapeHtml(t("cap.form.field.http.headers"))}</label>
                <textarea id="cf-http-headers" class="form-textarea code" rows="3" placeholder="Accept: application/json">${escapeHtml(prefill.http?.headers || "")}</textarea>
                <label for="cf-http-body" style="color: var(--muted); font-size: 0.72rem; display: block; margin-top: 10px;">${t("cap.form.field.http.body_template")}</label>
                <textarea id="cf-http-body" class="form-textarea code" rows="4" placeholder='{"query":"{{args.q}}"}'>${escapeHtml(prefill.http?.body_template || "")}</textarea>
            </div>
            <div id="cf-bind-mcp" class="hidden" style="margin-top: 10px;">
                <form class="kv" onsubmit="return false;">
                    <label for="cf-mcp-tool">${escapeHtml(t("cap.form.field.mcp.tool_name"))}</label>
                    <input id="cf-mcp-tool" type="text" value="${escapeHtml(prefill.mcp?.tool_name || "")}" placeholder="read_file" />
                </form>
                <label for="cf-mcp-argmap" style="color: var(--muted); font-size: 0.72rem; display: block; margin-top: 10px;">${escapeHtml(t("cap.form.field.mcp.arg_mapping"))}</label>
                <textarea id="cf-mcp-argmap" class="form-textarea code" rows="4">${escapeHtml(prefill.mcp?.arg_mapping || "{}")}</textarea>
                <label for="cf-mcp-resmap" style="color: var(--muted); font-size: 0.72rem; display: block; margin-top: 10px;">${escapeHtml(t("cap.form.field.mcp.result_mapping"))}</label>
                <textarea id="cf-mcp-resmap" class="form-textarea code" rows="4">${escapeHtml(prefill.mcp?.result_mapping || "{}")}</textarea>
            </div>
        </section>
        <section class="section">
            <h3>${escapeHtml(t("cap.form.section.schemas"))}</h3>
            <label for="cf-schemain" style="color: var(--muted); font-size: 0.72rem;">${escapeHtml(t("cap.form.field.schema_in"))}</label>
            <textarea id="cf-schemain" class="form-textarea code" rows="7">${escapeHtml(prefill.schema_in)}</textarea>
            <label for="cf-schemaout" style="color: var(--muted); font-size: 0.72rem; margin-top: 10px; display: block;">${escapeHtml(t("cap.form.field.schema_out"))}</label>
            <textarea id="cf-schemaout" class="form-textarea code" rows="7">${escapeHtml(prefill.schema_out)}</textarea>
        </section>
        <section class="section">
            <h3>${escapeHtml(t("cap.form.section.example"))}</h3>
            <form class="kv" onsubmit="return false;">
                <label for="cf-ex-intent">${escapeHtml(t("cap.form.field.example.user_intent"))}</label>
                <input id="cf-ex-intent" type="text" value="${escapeHtml(prefill.example_intent)}"
                       placeholder="translate 'bonjour' to English" />
            </form>
            <label for="cf-ex-args" style="color: var(--muted); font-size: 0.72rem; display: block; margin-top: 8px;">${escapeHtml(t("cap.form.field.example.args"))}</label>
            <textarea id="cf-ex-args" class="form-textarea code" rows="3">${escapeHtml(prefill.example_args)}</textarea>
            <label for="cf-ex-out" style="color: var(--muted); font-size: 0.72rem; display: block; margin-top: 8px;">${escapeHtml(t("cap.form.field.example.output"))}</label>
            <textarea id="cf-ex-out" class="form-textarea code" rows="3">${escapeHtml(prefill.example_output)}</textarea>
        </section>
        <div style="display: flex; gap: 8px; justify-content: flex-end;">
            <button id="cf-cancel" type="button" class="icon-btn">${escapeHtml(t("button.cancel"))}</button>
            <button id="cf-save" type="button" class="primary" style="margin: 0;">${escapeHtml(existingName ? t("cap.form.button.save_changes") : t("cap.form.button.create"))}</button>
        </div>
        <div id="cf-status" class="row-sub" style="margin-top: 8px;"></div>
    `;
    overlay.classList.remove("hidden");
    overlay.setAttribute("aria-hidden", "false");

    // Wire prompt-binding "parser" select since `selected` attribute is
    // not honored via a static string when the value contains quotes.
    const parserSel = document.getElementById("cf-parser");
    if (parserSel) parserSel.value = prefill.prompt?.parser || "text";

    document.getElementById("cf-back-templates")?.addEventListener("click", openCapTemplatePicker);

    const kindSel = document.getElementById("cf-bindkind");
    const backendSel = document.getElementById("cf-backend");
    const blocks = {
        prompt: document.getElementById("cf-bind-prompt"),
        http:   document.getElementById("cf-bind-http"),
        mcp:    document.getElementById("cf-bind-mcp"),
    };
    const refreshBackendOptions = () => {
        const kind = kindSel.value;
        const wanted = kind === "prompt" ? "openai_compat" : kind === "http" ? "http_base" : "mcp_server";
        const filtered = backends.filter(b => b.kind === wanted);
        backendSel.innerHTML = filtered.length
            ? filtered.map(b => `<option value="${escapeHtml(b.name)}">${escapeHtml(b.name)}</option>`).join("")
            : `<option value="" disabled>${escapeHtml(t("cap.form.field.backend.empty", { kind: wanted }))}</option>`;
        if (prefill.backend && filtered.some(b => b.name === prefill.backend)) {
            backendSel.value = prefill.backend;
        }
    };
    const refreshBlocks = () => {
        const k = kindSel.value;
        Object.entries(blocks).forEach(([key, el]) => el.classList.toggle("hidden", key !== k));
    };
    kindSel.addEventListener("change", () => { refreshBackendOptions(); refreshBlocks(); });
    refreshBackendOptions();
    refreshBlocks();

    document.getElementById("cf-cancel")?.addEventListener("click", closeInspector);
    document.getElementById("cf-save")?.addEventListener("click", () => saveCapForm(existingName));
}


async function saveCapForm(existingName) {
    const status = document.getElementById("cf-status");
    const name = document.getElementById("cf-name").value.trim();
    const version = document.getElementById("cf-version").value.trim();
    const description = document.getElementById("cf-desc").value.trim();
    const mode = document.getElementById("cf-mode").value;
    const languages = csvList(document.getElementById("cf-langs").value);
    const countries = csvList(document.getElementById("cf-countries").value);
    const tags = csvList(document.getElementById("cf-tags").value);
    const disambig = document.getElementById("cf-disambig").value.trim();
    const outsem = document.getElementById("cf-outsem").value.trim();
    const bindKind = document.getElementById("cf-bindkind").value;
    const backend = document.getElementById("cf-backend").value;

    if (!name || !version || !description) {
        status.textContent = "name, version, description required";
        return;
    }
    if (!backend) {
        status.textContent = "select a backend (or add one first)";
        return;
    }

    let schemaIn, schemaOut, exArgs, exOutput;
    try {
        schemaIn = JSON.parse(document.getElementById("cf-schemain").value);
        schemaOut = JSON.parse(document.getElementById("cf-schemaout").value);
    } catch (e) {
        status.textContent = `schemas must be valid JSON: ${e.message}`;
        return;
    }
    try {
        exArgs = JSON.parse(document.getElementById("cf-ex-args").value);
        exOutput = JSON.parse(document.getElementById("cf-ex-out").value);
    } catch (e) {
        status.textContent = `example args/output must be valid JSON: ${e.message}`;
        return;
    }
    const exIntent = document.getElementById("cf-ex-intent").value.trim();
    if (!exIntent) { status.textContent = "example user_intent required"; return; }

    let binding;
    if (bindKind === "prompt") {
        const systemPrompt = document.getElementById("cf-sysprompt").value.trim();
        const userTemplate = document.getElementById("cf-usertpl").value;
        const parser = document.getElementById("cf-parser").value;
        const temperature = parseFloat(document.getElementById("cf-temp").value || "0");
        const model = document.getElementById("cf-model").value.trim();
        if (!systemPrompt) { status.textContent = "system_prompt is required for prompt binding"; return; }
        binding = {
            type: "prompt", backend,
            system_prompt: systemPrompt,
            user_template: userTemplate.trim() ? userTemplate : null,
            parameters: temperature ? { temperature } : {},
            output_parser: parser,
            model: model || null,
        };
    } else if (bindKind === "http") {
        const url_template = document.getElementById("cf-http-url").value.trim();
        const method = document.getElementById("cf-http-method").value;
        const respath = document.getElementById("cf-http-respath").value.trim();
        const timeoutRaw = document.getElementById("cf-http-timeout").value.trim();
        if (!url_template) { status.textContent = "url_template is required for http binding"; return; }
        const headers = {};
        for (const line of document.getElementById("cf-http-headers").value.split("\n")) {
            const t = line.trim();
            if (!t) continue;
            const colon = t.indexOf(":");
            if (colon <= 0) { status.textContent = `bad header line: "${t}" (expected Key: value)`; return; }
            headers[t.slice(0, colon).trim()] = t.slice(colon + 1).trim();
        }
        let body_template = null;
        const bodyRaw = document.getElementById("cf-http-body").value.trim();
        if (bodyRaw) {
            try { body_template = JSON.parse(bodyRaw); }
            catch (e) { status.textContent = `body_template must be valid JSON: ${e.message}`; return; }
        }
        binding = {
            type: "http", backend,
            url_template, method, headers,
            body_template,
            response_path: respath || null,
            timeout_ms: timeoutRaw ? parseInt(timeoutRaw, 10) : null,
        };
    } else if (bindKind === "mcp") {
        const tool_name = document.getElementById("cf-mcp-tool").value.trim();
        if (!tool_name) { status.textContent = "tool_name is required for mcp binding"; return; }
        let arg_mapping, result_mapping;
        try { arg_mapping = JSON.parse(document.getElementById("cf-mcp-argmap").value || "{}"); }
        catch (e) { status.textContent = `arg_mapping must be valid JSON: ${e.message}`; return; }
        try { result_mapping = JSON.parse(document.getElementById("cf-mcp-resmap").value || "{}"); }
        catch (e) { status.textContent = `result_mapping must be valid JSON: ${e.message}`; return; }
        binding = { type: "mcp", backend, tool_name, arg_mapping, result_mapping };
    } else {
        status.textContent = `unknown binding kind: ${bindKind}`; return;
    }

    const payload = {
        name, version, description, mode,
        languages, countries, tags, lobe_ids: [],
        disambiguation: disambig || null,
        output_semantic: outsem || null,
        schema_in: schemaIn,
        schema_out: schemaOut,
        examples: [{ user_intent: exIntent, args: exArgs, expected_output: exOutput }],
        binding,
    };
    status.textContent = "saving…";
    try {
        const r = await api("POST", "/api/v0/caps/manifests", payload);
        status.textContent = `saved — ${r.registered} skill${r.registered !== 1 ? "s" : ""} active.`;
        await renderCapsCards();
        setTimeout(closeInspector, 700);
    } catch (e) {
        status.textContent = `save failed: ${e.message}`;
    }
}

// Best-effort parse of cap.toml to detect binding kind + extract per-kind
// fields for the edit form. Falls back to "prompt" + empty fields on any
// trouble; the user sees blank fields rather than a broken form.
function parseCapTomlBindings(raw) {
    const out = { kind: "prompt", backend: "" };
    const bindMatch = raw.match(/\[binding\][^\[]*?type\s*=\s*"([^"]+)"[^\[]*?backend\s*=\s*"([^"]+)"/);
    if (bindMatch) {
        out.kind = bindMatch[1];
        out.backend = bindMatch[2];
    }
    const grab = (section, key) => {
        const re = new RegExp(`\\[${section}\\][\\s\\S]*?${key}\\s*=\\s*("""[\\s\\S]*?"""|"[^"]*")`);
        const m = raw.match(re);
        if (!m) return "";
        let v = m[1];
        if (v.startsWith('"""')) v = v.slice(3, -3).replace(/^\n/, "");
        else v = v.slice(1, -1);
        return v.replace(/\\"/g, '"').replace(/\\\\/g, "\\");
    };
    const grabRaw = (section, key) => {
        const re = new RegExp(`\\[${section}\\][\\s\\S]*?^${key}\\s*=\\s*(.+)$`, "m");
        const m = raw.match(re);
        return m ? m[1].trim() : "";
    };
    if (out.kind === "prompt") {
        out.prompt = {
            system: grab("binding\\.prompt", "system_prompt"),
            user: grab("binding\\.prompt", "user_template"),
            parser: (grab("binding\\.prompt", "output_parser") || "text"),
            model: grab("binding\\.prompt", "model"),
            temperature: (() => {
                const m = raw.match(/\[binding\.prompt\][\s\S]*?temperature\s*=\s*([0-9.]+)/);
                return m ? parseFloat(m[1]) : 0;
            })(),
        };
    } else if (out.kind === "http") {
        out.http = {
            url_template: grab("binding\\.http", "url_template"),
            method: grab("binding\\.http", "method") || "GET",
            response_path: grab("binding\\.http", "response_path"),
            timeout_ms: (() => {
                const m = raw.match(/\[binding\.http\][\s\S]*?timeout_ms\s*=\s*(\d+)/);
                return m ? parseInt(m[1], 10) : null;
            })(),
            headers: (() => {
                const m = raw.match(/\[binding\.http\.headers\]([\s\S]*?)(?=\n\[|$)/);
                if (!m) return "";
                return m[1].trim().split("\n")
                    .map(l => l.trim()).filter(Boolean)
                    .map(l => {
                        const kv = l.match(/^([^=]+)=\s*"([^"]*)"\s*$/);
                        return kv ? `${kv[1].trim().replace(/^"|"$/g, "")}: ${kv[2]}` : l;
                    }).join("\n");
            })(),
            body_template: (() => {
                const r = grabRaw("binding\\.http", "body_template");
                return r || "";
            })(),
        };
    } else if (out.kind === "mcp") {
        out.mcp = {
            tool_name: grab("binding\\.mcp", "tool_name"),
            arg_mapping: grabRaw("binding\\.mcp", "arg_mapping") || "{}",
            result_mapping: grabRaw("binding\\.mcp", "result_mapping") || "{}",
        };
    }
    return out;
}

function csvList(s) {
    return (s || "").split(",").map(x => x.trim()).filter(Boolean);
}

async function openBackendForm(existingName) {
    let prefill = null;
    if (existingName) {
        try {
            prefill = await api("GET", `/api/v0/backends/${encodeURIComponent(existingName)}`);
        } catch (e) {
            await alertModal(`Could not load backend: ${e.message}`, { title: "Error" });
            return;
        }
    }
    const kindInit = prefill?.kind || "openai_compat";

    const overlay = document.getElementById("inspector");
    document.getElementById("inspector-title").textContent =
        existingName
            ? t("backend.form.edit_title", { name: existingName })
            : t("backend.form.add_title");
    document.getElementById("inspector-body").innerHTML = `
        <section class="section">
            <p>${existingName ? t("backend.form.intro.edit") : t("backend.form.intro.create")}</p>
            <form class="kv" onsubmit="return false;">
                <label for="bf-name">${escapeHtml(t("backend.form.name"))}</label>
                <input id="bf-name" type="text" required pattern="[a-zA-Z0-9_-]+"
                       value="${escapeHtml(prefill?.name || "")}"
                       ${existingName ? "readonly" : ""}
                       placeholder="openai_main · llama_local · fs_mcp · weather_api" />
                <label for="bf-kind">${escapeHtml(t("backend.form.kind"))}</label>
                <select id="bf-kind" ${existingName ? `disabled title="${escapeHtml(t("backend.form.kind.locked"))}"` : ""}>
                    <option value="openai_compat"${kindInit === "openai_compat" ? " selected" : ""}>openai_compat — OpenAI / Ollama / vLLM / llama.cpp</option>
                    <option value="mcp_server"${kindInit === "mcp_server" ? " selected" : ""}>mcp_server — local MCP tool server</option>
                    <option value="http_base"${kindInit === "http_base" ? " selected" : ""}>http_base — generic HTTP API</option>
                </select>
            </form>
        </section>
        <section class="section" id="bf-openai">
            <h3>${escapeHtml(t("backend.form.openai.title"))}</h3>
            <form class="kv" onsubmit="return false;">
                <label for="bf-base">${escapeHtml(t("backend.form.openai.base_url"))}</label>
                <input id="bf-base" type="url" value="${escapeHtml(prefill?.base_url || "")}" placeholder="https://api.openai.com · http://192.168.4.101:11434" />
                <p class="row-sub">${escapeHtml(t("backend.form.openai.base_url.help"))}</p>
                <label for="bf-model">${escapeHtml(t("backend.form.openai.default_model"))}</label>
                <input id="bf-model" type="text" value="${escapeHtml(prefill?.default_model || "")}" placeholder="gpt-4o-mini · llama3.1:8b · qwen2.5:7b" />
                <label for="bf-key">${escapeHtml(t("backend.form.openai.api_key"))}</label>
                <input id="bf-key" type="password" placeholder="${escapeHtml(existingName && prefill?.has_api_key ? t("backend.form.openai.api_key.placeholder.keep") : t("backend.form.openai.api_key.placeholder.new"))}" />
            </form>
        </section>
        <section class="section hidden" id="bf-mcp">
            <h3>${escapeHtml(t("backend.form.mcp.title"))}</h3>
            <form class="kv" onsubmit="return false;">
                <label for="bf-mcp-transport">${escapeHtml(t("backend.form.mcp.transport"))}</label>
                <select id="bf-mcp-transport">
                    <option value="stdio"${prefill?.transport === "stdio" ? " selected" : ""}>stdio (local exec)</option>
                    <option value="httpsse"${prefill?.transport === "httpsse" ? " selected" : ""}>http_sse (remote)</option>
                </select>
                <label for="bf-mcp-cmd">${escapeHtml(t("backend.form.mcp.command"))}</label>
                <input id="bf-mcp-cmd" type="text" value="${escapeHtml(prefill?.command || "")}" placeholder="stdio: /path/to/server-binary · http_sse: https://mcp.example.com" />
                <label for="bf-mcp-args">${escapeHtml(t("backend.form.mcp.args"))}</label>
                <textarea id="bf-mcp-args" class="form-textarea code" rows="3" placeholder="--root&#10;/data">${escapeHtml((prefill?.args || []).join("\n"))}</textarea>
                <label for="bf-mcp-env">${escapeHtml(t("backend.form.mcp.env"))}</label>
                <textarea id="bf-mcp-env" class="form-textarea code" rows="3" placeholder="LOG_LEVEL=info">${escapeHtml(Object.entries(prefill?.env || {}).map(([k,v])=>`${k}=${v}`).join("\n"))}</textarea>
            </form>
        </section>
        <section class="section hidden" id="bf-http">
            <h3>${escapeHtml(t("backend.form.http.title"))}</h3>
            <form class="kv" onsubmit="return false;">
                <label for="bf-http-base">base_url</label>
                <input id="bf-http-base" type="url" value="${escapeHtml(prefill?.base_url || "")}" placeholder="https://api.example.com" />
                <label for="bf-http-headers">${escapeHtml(t("backend.form.http.headers"))}</label>
                <textarea id="bf-http-headers" class="form-textarea code" rows="4" placeholder="Authorization: Bearer YOUR_TOKEN&#10;User-Agent: n3ur0n/0.3">${escapeHtml(Object.entries(prefill?.headers || {}).map(([k,v])=>`${k}: ${v}`).join("\n"))}</textarea>
            </form>
        </section>
        <div style="display: flex; gap: 8px; margin-top: 12px; justify-content: flex-end;">
            <button id="bf-back" type="button" class="icon-btn">${escapeHtml(t("button.back"))}</button>
            <button id="bf-save" type="button" class="primary" style="margin: 0;">${escapeHtml(existingName ? t("button.save_changes") : t("button.save"))}</button>
        </div>
        <div id="bf-status" class="row-sub" style="margin-top: 8px;"></div>
    `;
    overlay.classList.remove("hidden");
    overlay.setAttribute("aria-hidden", "false");

    const kindSel = document.getElementById("bf-kind");
    const sections = {
        openai_compat: document.getElementById("bf-openai"),
        mcp_server: document.getElementById("bf-mcp"),
        http_base: document.getElementById("bf-http"),
    };
    const showKind = (k) => {
        Object.entries(sections).forEach(([key, el]) => el.classList.toggle("hidden", key !== k));
    };
    kindSel.addEventListener("change", () => showKind(kindSel.value));
    showKind(kindInit);

    document.getElementById("bf-back")?.addEventListener("click", closeInspector);
    document.getElementById("bf-save")?.addEventListener("click", async () => {
        const status = document.getElementById("bf-status");
        const name = existingName || document.getElementById("bf-name").value.trim();
        const kind = kindSel.value;
        if (!name) { status.textContent = "name required"; return; }
        const payload = { name, kind };
        if (kind === "openai_compat") {
            payload.base_url = normalizeOpenaiBaseUrl(document.getElementById("bf-base").value);
            payload.default_model = document.getElementById("bf-model").value.trim();
            const key = document.getElementById("bf-key").value;
            if (!payload.base_url || !payload.default_model) {
                status.textContent = "base_url + default_model required"; return;
            }
            // On edit with an existing key, blank means "keep". Send a sentinel
            // the server can detect — here we just omit api_key, and the
            // server's existing upsert behavior overwrites the file. So if
            // editing and the user left it blank, preload it from the (now
            // unknown) value? We can't — api_key is masked on GET. Compromise:
            // if existing + blank, we keep the file's previous value by
            // re-reading the existing manifest server-side. Simpler: include
            // api_key only if non-empty, then patch server to preserve on
            // empty.
            if (key) payload.api_key = key;
            else if (existingName && prefill?.has_api_key) payload.api_key_keep = true;
            else payload.api_key = "";
        } else if (kind === "mcp_server") {
            payload.transport = document.getElementById("bf-mcp-transport").value;
            payload.command = document.getElementById("bf-mcp-cmd").value.trim();
            if (!payload.command) { status.textContent = "command required"; return; }
            payload.args = document.getElementById("bf-mcp-args").value
                .split("\n").map(s => s.trim()).filter(Boolean);
            const env = {};
            for (const line of document.getElementById("bf-mcp-env").value.split("\n")) {
                const t = line.trim();
                if (!t) continue;
                const eq = t.indexOf("=");
                if (eq <= 0) { status.textContent = `bad env line: "${t}" (expected KEY=value)`; return; }
                env[t.slice(0, eq).trim()] = t.slice(eq + 1);
            }
            payload.env = env;
        } else if (kind === "http_base") {
            payload.base_url = document.getElementById("bf-http-base").value.trim();
            if (!payload.base_url) { status.textContent = "base_url required"; return; }
            const headers = {};
            for (const line of document.getElementById("bf-http-headers").value.split("\n")) {
                const t = line.trim();
                if (!t) continue;
                const colon = t.indexOf(":");
                if (colon <= 0) { status.textContent = `bad header line: "${t}" (expected Key: value)`; return; }
                headers[t.slice(0, colon).trim()] = t.slice(colon + 1).trim();
            }
            payload.headers = headers;
        }
        status.textContent = "saving…";
        try {
            const r = await api("POST", "/api/v0/backends", payload);
            if (r.planner_reload_warning) {
                status.textContent = `saved, but planner reload failed: ${r.planner_reload_warning}`;
            } else if (r.planner_reloaded && r.planner_active) {
                status.textContent = `saved — planner updated (${r.planner_active.base_url}, ${r.planner_active.model}).`;
            } else if (r.reload_warning) {
                status.textContent = `saved, but cap reload failed: ${r.reload_warning}`;
            } else {
                status.textContent = `saved — ${r.backends_loaded} backend${r.backends_loaded !== 1 ? "s" : ""} live, ${r.caps_loaded} cap${r.caps_loaded !== 1 ? "s" : ""} rebound.`;
            }
            await renderBackendsCards();
            setTimeout(closeInspector, 800);
        } catch (e) {
            status.textContent = `save failed: ${e.message}`;
        }
    });
}

document.querySelectorAll("#settings-nav .settings-nav-item").forEach(el => {
    el.addEventListener("click", () => activateSettingsSection(el.dataset.section));
});

document.querySelectorAll("#app-rail .rail-btn").forEach(btn => {
    btn.addEventListener("click", () => {
        const section = btn.dataset.section;
        if (section === "settings") {
            openSettings();
        } else {
            activateSection(section);
        }
    });
});

applyIcons();
activateSection("chats");
document.getElementById("network-refresh")?.addEventListener("click", refreshNetwork);
document.getElementById("skills-refresh")?.addEventListener("click", refreshSkills);
document.getElementById("files-refresh")?.addEventListener("click", refreshFiles);
document.querySelectorAll("#files-nav .files-nav-item").forEach(el => {
    el.addEventListener("click", () => setFilesCategory(el.dataset.category));
});
document.getElementById("files-filter")?.addEventListener("input", renderFilesList);
document.getElementById("files-upload")?.addEventListener("click", () => {
    document.getElementById("files-input")?.click();
});
document.getElementById("files-input")?.addEventListener("change", async (e) => {
    const files = e.target.files;
    if (!files?.length) return;
    try {
        await uploadFiles(files);
    } catch (err) {
        alert(err.message);
    }
    e.target.value = "";
});

composerFileInput?.addEventListener("change", async (e) => {
    const files = e.target.files;
    if (!files?.length) return;
    try {
        await uploadDraftFiles(files);
    } catch (err) {
        alert(t("composer.upload.err", { msg: err.message }));
    }
    e.target.value = "";
});
document.getElementById("network-filter")?.addEventListener("input", renderNetworkList);
document.getElementById("skills-filter")?.addEventListener("input", renderSkillsList);
document.getElementById("skills-type-filter")?.addEventListener("change", renderSkillsList);
document.getElementById("inspector-back")?.addEventListener("click", closeInspector);
document.getElementById("inspector-close")?.addEventListener("click", closeInspector);
document.addEventListener("keydown", (e) => {
    if (e.key === "Escape") closeInspector();
});
