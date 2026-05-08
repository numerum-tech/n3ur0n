const $ = (id) => document.getElementById(id);
const sidebar = $("conv-list");
const conv = $("conversation");
const titleEl = $("conv-title");
const promptEl = $("prompt");
const sendBtn = $("send");
const newBtn = $("new-chat");
const renameBtn = $("rename");
const deleteBtn = $("delete");
const selfId = $("self-id");

const LS_CURRENT = "n3ur0n_current_conversation";
let activeId = localStorage.getItem(LS_CURRENT) || null;
let conversations = [];
let inFlight = false;

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
// Sidebar
// ---------------------------------------------------------------------------

async function loadConversations() {
    sidebar.innerHTML = "";
    try {
        const r = await api("GET", "/api/v0/whoami");
        selfId.textContent = r?.instance_id || "?";
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
    promptEl.disabled = false;
    sendBtn.disabled = false;
    promptEl.focus();
    renderTurns(data.turns || []);
}

// ---------------------------------------------------------------------------
// Conversation rendering
// ---------------------------------------------------------------------------

function renderTurns(turns) {
    conv.innerHTML = "";
    let pendingTrace = null; // gather tool_call+result pairs into one bubble
    for (const t of turns) {
        renderTurn(t, /*append=*/true);
    }
    conv.scrollTop = conv.scrollHeight;
}

function renderTurn(turn) {
    if (!turn || !turn.role) return null;
    const role = turn.role;
    if (role === "user") {
        return appendBubble("user", "you", turn.content);
    }
    if (role === "assistant") {
        const who = turn.model ? `assistant · ${turn.model}` : "assistant";
        return appendBubble("assistant", who, turn.content || "");
    }
    if (role === "system") {
        return appendBubble("system", null, turn.content);
    }
    if (role === "tool_call") {
        const div = document.createElement("div");
        div.className = "bubble tool";
        const who = document.createElement("span");
        who.className = "who";
        who.textContent = `→ ${shortPeer(turn.peer_id)}::${turn.capability}`;
        div.appendChild(who);
        const det = document.createElement("details");
        const sum = document.createElement("summary");
        sum.textContent = "args";
        det.appendChild(sum);
        const pre = document.createElement("pre");
        pre.textContent = JSON.stringify(turn.args, null, 2);
        det.appendChild(pre);
        div.appendChild(det);
        conv.appendChild(div);
        return div;
    }
    if (role === "tool_result") {
        const div = document.createElement("div");
        div.className = "bubble tool";
        const who = document.createElement("span");
        who.className = "who";
        const cap = `${shortPeer(turn.peer_id)}::${turn.capability}`;
        who.textContent = turn.error ? `← ${cap} (error)` : `← ${cap}`;
        if (turn.error) div.classList.add("warn");
        div.appendChild(who);
        const det = document.createElement("details");
        const sum = document.createElement("summary");
        sum.textContent = turn.error ? "error" : "result";
        det.appendChild(sum);
        const pre = document.createElement("pre");
        pre.textContent = turn.error
            ? turn.error
            : JSON.stringify(turn.result, null, 2);
        det.appendChild(pre);
        div.appendChild(det);
        conv.appendChild(div);
        return div;
    }
    return null;
}

function appendBubble(kind, who, text) {
    const div = document.createElement("div");
    div.className = `bubble ${kind}`;
    if (who) {
        const w = document.createElement("span");
        w.className = "who";
        w.textContent = who;
        div.appendChild(w);
    }
    const t = document.createElement("span");
    t.textContent = text;
    div.appendChild(t);
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
    if (!text) return;
    promptEl.value = "";
    inFlight = true;
    sendBtn.disabled = true;

    appendBubble("user", "you", text);
    const pending = appendBubble("assistant", "thinking…", "");

    try {
        const r = await api("POST", `/api/v0/conversations/${encodeURIComponent(activeId)}/messages`, { message: text });
        // Re-render the conversation from server (canonical order including tool turns).
        await renderActive();
        // Refresh sidebar order.
        await loadConversations();
    } catch (e) {
        pending.classList.replace("assistant", "error");
        pending.querySelector(".who").textContent = "error";
        const span = pending.querySelector("span:last-child");
        span.textContent = e.message;
    } finally {
        inFlight = false;
        sendBtn.disabled = false;
        promptEl.focus();
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
    if (!window.confirm(`Delete this conversation?`)) return;
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
promptEl.addEventListener("keydown", (e) => {
    if (e.key === "Enter" && !e.shiftKey && !e.isComposing) {
        e.preventDefault();
        send();
    }
});

(async () => {
    await loadConversations();
    if (activeId) {
        await renderActive();
    }
})();
