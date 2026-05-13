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
            appendBubble("user", "you", t.content);
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
    const stepper = appendStepper();

    try {
        await streamDispatch(activeId, text, stepper);
        // Refresh sidebar (updated_at + auto-title). Skip conv re-render —
        // stepper stays visible alongside the streamed assistant bubble.
        await loadConversations();
    } catch (e) {
        stepper.markError(e.message);
    } finally {
        inFlight = false;
        sendBtn.disabled = false;
        promptEl.focus();
    }
}

// ---------------------------------------------------------------------------
// Streaming dispatch (SSE)
// ---------------------------------------------------------------------------

function appendStepper() {
    const wrap = document.createElement("div");
    wrap.className = "stepper";

    const status = document.createElement("div");
    status.className = "stepper-status";
    status.textContent = "compiling plan…";
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

async function streamDispatch(convId, message, stepper) {
    const res = await fetch(
        `/api/v0/conversations/${encodeURIComponent(convId)}/messages/stream`,
        {
            method: "POST",
            credentials: "same-origin",
            headers: { "content-type": "application/json", accept: "text/event-stream" },
            body: JSON.stringify({ message }),
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

function escapeHtml(s) {
    if (s === null || s === undefined) return "";
    return String(s)
        .replace(/&/g, "&amp;")
        .replace(/</g, "&lt;")
        .replace(/>/g, "&gt;")
        .replace(/"/g, "&quot;");
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
    const list = document.getElementById("skills-list");
    const stats = document.getElementById("skills-stats");

    const all = buildMergedCatalog();
    const filtered = all.filter(entry => {
        if (!filter) return true;
        const c = entry.decl;
        const hay = [
            c.name,
            c.description || "",
            ...(c.tags || []),
            ...(c.languages || []),
            ...(c.countries || []),
            ...entry.sources.map(s => s.endpoint),
        ].join(" ").toLowerCase();
        return hay.includes(filter);
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
                c.mode,
                ...(c.languages || []),
                ...(c.tags || []).slice(0, 3),
            ].filter(Boolean).join(" · ");
            return `
                <li data-cap="${escapeHtml(c.name)}">
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
                <dt>mode</dt><dd>${escapeHtml(cap.mode)}</dd>
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

function activateTab(name) {
    document.querySelectorAll(".sidebar-tabs .tab").forEach(t =>
        t.classList.toggle("active", t.dataset.tab === name)
    );
    document.querySelectorAll(".tab-panel").forEach(p =>
        p.classList.toggle("hidden", p.dataset.panel !== name)
    );
    if (name === "chats") closeInspector();
    if (name === "network") refreshNetwork();
    if (name === "skills") refreshSkills();
}

// ---------------------------------------------------------------------------
// Settings — master/detail: sidebar lists sections, main pane renders the
// selected section as a rich page (card grids, friendly empty states).
// ---------------------------------------------------------------------------

function openSettings() {
    document.getElementById("sidebar-main").classList.add("hidden");
    document.getElementById("sidebar-settings").classList.remove("hidden");
    closeInspector();
    document.getElementById("settings-page").classList.remove("hidden");
    activateSettingsSection("backends");
}

function closeSettings() {
    document.getElementById("sidebar-settings").classList.add("hidden");
    document.getElementById("sidebar-main").classList.remove("hidden");
    document.getElementById("settings-page").classList.add("hidden");
    closeInspector();
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
        title.textContent = "Backends";
        sub.textContent = "Where AI calls go. Add a local LLM (Ollama), a cloud API (OpenAI, Anthropic, Mistral), or any OpenAI-compatible endpoint.";
        actions.innerHTML = `<button class="primary" id="settings-add-backend">+ Add backend</button>`;
        document.getElementById("settings-add-backend")?.addEventListener("click", openBackendForm);
        renderBackendsCards();
    } else if (name === "caps") {
        title.textContent = "Skills";
        sub.textContent = "Capabilities (skills) declared in your manifests. They're invokable by this client and can be shared with peers.";
        actions.innerHTML = `<button class="primary" id="settings-add-cap">+ Add skill</button>`;
        document.getElementById("settings-add-cap")?.addEventListener("click", () => openCapForm(null));
        renderCapsCards();
    } else if (name === "gateways") {
        title.textContent = "Gateways";
        sub.textContent = "Remote n3ur0n peers you've added. Their skills appear in your catalog.";
        actions.innerHTML = `<button class="primary" id="settings-add-gateway">+ Add gateway</button>`;
        document.getElementById("settings-add-gateway")?.addEventListener("click", openGatewayForm);
        renderGatewaysCards();
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
                <p class="empty-title">No backends yet</p>
                <p class="empty-body">A backend tells N3UR0N where to send AI calls — your local Ollama, an OpenAI key, an MCP server, or any HTTP API. Add your first one to get started.</p>
                <button class="primary" id="empty-add-backend">+ Add backend</button>
            </div>
        `;
        document.getElementById("empty-add-backend")?.addEventListener("click", openBackendForm);
        return;
    }
    body.innerHTML = `<div class="card-grid">${backends.map(backendCard).join("")}</div>`;
    body.querySelectorAll('.card [data-action="delete"]').forEach(btn => {
        btn.addEventListener("click", async (e) => {
            e.stopPropagation();
            const name = btn.closest(".card").dataset.backend;
            if (!(await confirmModal(`Delete backend "${name}"? Restart required.`, { title: "Delete backend", okLabel: "Delete", danger: true }))) return;
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
        <article class="card" data-backend="${escapeHtml(b.name)}">
            <div class="card-head">
                <div class="card-icon">${icon}</div>
                <span class="card-title">${escapeHtml(b.name)}</span>
                <span class="card-kind">${escapeHtml(label)}</span>
            </div>
            ${meta}
            <div class="card-actions">
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
        if (caps.length === 0) {
            body.innerHTML = `
                <div class="empty-state">
                    <div class="empty-icon">✦</div>
                    <p class="empty-title">No skills yet</p>
                    <p class="empty-body">A skill is a capability you expose: a translation, a search, a structured chat. Use the composer to declare your first one.</p>
                    <button class="primary" id="empty-add-cap">+ Add skill</button>
                </div>
            `;
            document.getElementById("empty-add-cap")?.addEventListener("click", () => openCapForm(null));
            return;
        }
        body.innerHTML = `<div class="card-grid">${caps.map(capCard).join("")}</div>`;
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
                if (!(await confirmModal(`Delete skill "${name}"?`, { title: "Delete skill", okLabel: "Delete", danger: true }))) return;
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
    return `
        <article class="card" data-cap="${escapeHtml(c.name)}" style="cursor: pointer;">
            <div class="card-head">
                <div class="card-icon">✦</div>
                <span class="card-title">${escapeHtml(c.name)}</span>
                <span class="card-kind">${label}</span>
            </div>
            <div class="card-meta">
                v${escapeHtml(c.version || "?")} · ${escapeHtml(c.mode)}
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
                    <div class="empty-icon">⇄</div>
                    <p class="empty-title">No gateways yet</p>
                    <p class="empty-body">A gateway is a remote n3ur0n peer you've connected to. Their published skills will appear in your skills catalog. Add the URL of a peer to start sharing.</p>
                    <button class="primary" id="empty-add-gateway">+ Add gateway</button>
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
        body.innerHTML = `<div class="empty-state"><div class="empty-icon">⚠</div><p class="empty-title">load failed</p><p class="empty-body">${escapeHtml(e.message)}</p></div>`;
    }
}

function gatewayCard(p) {
    const caps = (p.capabilities || []).length;
    const capNames = (p.capabilities || []).map(c => c.name).slice(0, 4).join(" · ") +
        (caps > 4 ? ` · +${caps - 4}` : "");
    return `
        <article class="card" data-peer="${escapeHtml(p.instance_id)}" style="cursor: pointer;">
            <div class="card-head">
                <div class="card-icon">⇄</div>
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

async function openCapForm(existingName) {
    // Fetch backends to populate the dropdown + the existing manifest
    // (if editing) in parallel.
    let backends = [];
    let prefill = null;
    try {
        const b = await api("GET", "/api/v0/backends");
        backends = (b.backends || []).filter(x => !x.error);
    } catch { /* leave empty */ }
    if (existingName) {
        try {
            const list = await api("GET", "/api/v0/caps");
            const cap = (list.caps || []).find(c => c.name === existingName);
            if (cap) {
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
                };
            }
        } catch { /* ignore */ }
    }
    if (!prefill) {
        prefill = {
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
        };
    }

    const backendOptions = backends.length
        ? backends.map(b => `<option value="${escapeHtml(b.name)}">${escapeHtml(b.name)} · ${escapeHtml(b.kind)}</option>`).join("")
        : `<option value="" disabled>no backends — add one first</option>`;

    const overlay = document.getElementById("inspector");
    document.getElementById("inspector-title").textContent =
        existingName ? `Edit skill · ${existingName}` : "Add skill";
    document.getElementById("inspector-body").innerHTML = `
        <section class="section">
            <h3>basics</h3>
            <form class="kv" onsubmit="return false;">
                <label for="cf-name">name</label>
                <input id="cf-name" type="text" required pattern="[a-zA-Z0-9_-]+"
                       value="${escapeHtml(prefill.name)}"
                       ${existingName ? "readonly" : ""}
                       placeholder="translator-fr-en, weather-now, legal-summarizer-fr…" />
                <label for="cf-version">version</label>
                <input id="cf-version" type="text" required value="${escapeHtml(prefill.version)}"
                       placeholder="semver: 0.1.0" />
                <label for="cf-desc">description</label>
                <input id="cf-desc" type="text" required value="${escapeHtml(prefill.description)}"
                       placeholder="One sentence: what does this skill do?" />
                <label for="cf-mode">access mode</label>
                <select id="cf-mode">
                    <option value="free"${prefill.mode === "free" ? " selected" : ""}>free</option>
                    <option value="restricted"${prefill.mode === "restricted" ? " selected" : ""}>restricted</option>
                </select>
                <label for="cf-langs">languages</label>
                <input id="cf-langs" type="text" value="${escapeHtml(prefill.languages)}"
                       placeholder="fr, en  (BCP 47, comma-separated)" />
                <label for="cf-countries">countries</label>
                <input id="cf-countries" type="text" value="${escapeHtml(prefill.countries)}"
                       placeholder="FR, BE, CH  (ISO 3166-1 alpha-2)" />
                <label for="cf-tags">tags</label>
                <input id="cf-tags" type="text" value="${escapeHtml(prefill.tags)}"
                       placeholder="translation, language, fr…" />
            </form>
        </section>
        <section class="section">
            <h3>disambiguation (recommended)</h3>
            <textarea id="cf-disambig" class="form-textarea" rows="3"
                      placeholder="When to prefer this skill vs others. When NOT to use it.">${escapeHtml(prefill.disambiguation)}</textarea>
        </section>
        <section class="section">
            <h3>binding · prompt (LLM-backed)</h3>
            <form class="kv" onsubmit="return false;">
                <label for="cf-backend">backend</label>
                <select id="cf-backend" required>${backendOptions}</select>
                <label for="cf-parser">output parser</label>
                <select id="cf-parser">
                    <option value="text">text (returns {text: "..."})</option>
                    <option value="json">json (parsed, must match schema_out)</option>
                </select>
                <label for="cf-temp">temperature</label>
                <input id="cf-temp" type="number" step="0.1" min="0" max="2" value="0.0" />
                <label for="cf-model">model override</label>
                <input id="cf-model" type="text" placeholder="(leave empty to use backend default)" />
            </form>
            <label for="cf-sysprompt" style="color: var(--muted); font-size: 0.72rem; display: block; margin-top: 10px;">system prompt (required)</label>
            <textarea id="cf-sysprompt" class="form-textarea" rows="6"
                      placeholder="You are a French→English translator. Respond strictly as JSON: {&quot;result&quot;: &quot;...&quot;}."></textarea>
            <label for="cf-usertpl" style="color: var(--muted); font-size: 0.72rem; display: block; margin-top: 10px;">user template (optional)</label>
            <textarea id="cf-usertpl" class="form-textarea" rows="3"
                      placeholder="Translate to English: {{args.text}}"></textarea>
        </section>
        <section class="section">
            <h3>schemas (JSON Schema, draft 7)</h3>
            <label for="cf-schemain" style="color: var(--muted); font-size: 0.72rem;">schema_in</label>
            <textarea id="cf-schemain" class="form-textarea code" rows="7">${escapeHtml(prefill.schema_in)}</textarea>
            <label for="cf-schemaout" style="color: var(--muted); font-size: 0.72rem; margin-top: 10px; display: block;">schema_out</label>
            <textarea id="cf-schemaout" class="form-textarea code" rows="7">${escapeHtml(prefill.schema_out)}</textarea>
        </section>
        <section class="section">
            <h3>example (at least one required)</h3>
            <form class="kv" onsubmit="return false;">
                <label for="cf-ex-intent">user_intent</label>
                <input id="cf-ex-intent" type="text" value="${escapeHtml(prefill.example_intent)}"
                       placeholder="translate 'bonjour' to English" />
            </form>
            <label for="cf-ex-args" style="color: var(--muted); font-size: 0.72rem; display: block; margin-top: 8px;">args (JSON)</label>
            <textarea id="cf-ex-args" class="form-textarea code" rows="3">${escapeHtml(prefill.example_args)}</textarea>
            <label for="cf-ex-out" style="color: var(--muted); font-size: 0.72rem; display: block; margin-top: 8px;">expected_output (JSON)</label>
            <textarea id="cf-ex-out" class="form-textarea code" rows="3">${escapeHtml(prefill.example_output)}</textarea>
        </section>
        <div style="display: flex; gap: 8px; justify-content: flex-end;">
            <button id="cf-cancel" type="button" class="icon-btn">Cancel</button>
            <button id="cf-save" type="button" class="primary" style="margin: 0;">${existingName ? "Save changes" : "Create skill"}</button>
        </div>
        <div id="cf-status" class="row-sub" style="margin-top: 8px;"></div>
    `;
    overlay.classList.remove("hidden");
    overlay.setAttribute("aria-hidden", "false");

    document.getElementById("cf-cancel")?.addEventListener("click", closeInspector);
    document.getElementById("cf-save")?.addEventListener("click", async () => {
        const status = document.getElementById("cf-status");
        const name = document.getElementById("cf-name").value.trim();
        const version = document.getElementById("cf-version").value.trim();
        const description = document.getElementById("cf-desc").value.trim();
        const mode = document.getElementById("cf-mode").value;
        const languages = csvList(document.getElementById("cf-langs").value);
        const countries = csvList(document.getElementById("cf-countries").value);
        const tags = csvList(document.getElementById("cf-tags").value);
        const disambig = document.getElementById("cf-disambig").value.trim();
        const backend = document.getElementById("cf-backend").value;
        const parser = document.getElementById("cf-parser").value;
        const temperature = parseFloat(document.getElementById("cf-temp").value || "0");
        const model = document.getElementById("cf-model").value.trim();
        const systemPrompt = document.getElementById("cf-sysprompt").value.trim();
        const userTemplate = document.getElementById("cf-usertpl").value;

        if (!name || !version || !description || !backend || !systemPrompt) {
            status.textContent = "name, version, description, backend, system_prompt are all required";
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

        const payload = {
            name, version, description, mode,
            languages, countries, tags, lobe_ids: [],
            disambiguation: disambig || null,
            output_semantic: null,
            schema_in: schemaIn,
            schema_out: schemaOut,
            examples: [{ user_intent: exIntent, args: exArgs, expected_output: exOutput }],
            binding: {
                type: "prompt",
                backend,
                system_prompt: systemPrompt,
                user_template: userTemplate.trim() ? userTemplate : null,
                parameters: temperature ? { temperature } : {},
                output_parser: parser,
                model: model || null,
            },
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
    });
}

function csvList(s) {
    return (s || "").split(",").map(x => x.trim()).filter(Boolean);
}

function openBackendForm() {
    const overlay = document.getElementById("inspector");
    document.getElementById("inspector-title").textContent = "Add backend (openai_compat)";
    document.getElementById("inspector-body").innerHTML = `
        <section class="section">
            <p>Declare an OpenAI-compatible backend (Ollama, llama.cpp, vLLM, OpenAI, etc).
            The file is written to <code>backends/&lt;name&gt;.toml</code> in your config dir.
            A <strong>restart is required</strong> for it to be picked up by the runtime.</p>
            <form id="backend-form" class="kv" onsubmit="return false;">
                <label for="bf-name">name</label>
                <input id="bf-name" type="text" required pattern="[a-zA-Z0-9_-]+" placeholder="e.g. openai_main, llama_cpp_local" />
                <label for="bf-base">base_url</label>
                <input id="bf-base" type="url" required placeholder="https://api.openai.com or http://localhost:11434" />
                <label for="bf-model">default_model</label>
                <input id="bf-model" type="text" required placeholder="gpt-4o-mini · llama3.1:8b · …" />
                <label for="bf-key">api_key</label>
                <input id="bf-key" type="password" placeholder="(leave empty for Ollama / local endpoints)" />
            </form>
            <div style="display: flex; gap: 8px; margin-top: 12px; justify-content: flex-end;">
                <button id="bf-back" type="button" class="icon-btn">← Back</button>
                <button id="bf-save" type="button" class="primary" style="margin: 0;">Save</button>
            </div>
            <div id="bf-status" class="row-sub" style="margin-top: 8px;"></div>
        </section>
    `;
    overlay.classList.remove("hidden");
    overlay.setAttribute("aria-hidden", "false");
    document.getElementById("bf-back")?.addEventListener("click", closeInspector);
    document.getElementById("bf-save")?.addEventListener("click", async () => {
        const name = document.getElementById("bf-name").value.trim();
        const base = document.getElementById("bf-base").value.trim();
        const model = document.getElementById("bf-model").value.trim();
        const key = document.getElementById("bf-key").value;
        const status = document.getElementById("bf-status");
        if (!name || !base || !model) {
            status.textContent = "name, base_url, default_model are all required";
            return;
        }
        status.textContent = "saving…";
        try {
            await api("POST", "/api/v0/backends", {
                name, kind: "openai_compat",
                base_url: base, default_model: model, api_key: key,
            });
            status.textContent = "saved. restart required.";
            await renderBackendsList();
            setTimeout(closeInspector, 600);
        } catch (e) {
            status.textContent = `save failed: ${e.message}`;
        }
    });
}

document.getElementById("settings-open")?.addEventListener("click", openSettings);
document.getElementById("settings-back")?.addEventListener("click", closeSettings);
document.querySelectorAll("#settings-nav .settings-nav-item").forEach(el => {
    el.addEventListener("click", () => activateSettingsSection(el.dataset.section));
});

document.querySelectorAll(".sidebar-tabs .tab").forEach(t => {
    t.addEventListener("click", () => activateTab(t.dataset.tab));
});
document.getElementById("network-refresh")?.addEventListener("click", refreshNetwork);
document.getElementById("skills-refresh")?.addEventListener("click", refreshSkills);
document.getElementById("network-filter")?.addEventListener("input", renderNetworkList);
document.getElementById("skills-filter")?.addEventListener("input", renderSkillsList);
document.getElementById("inspector-back")?.addEventListener("click", closeInspector);
document.getElementById("inspector-close")?.addEventListener("click", closeInspector);
document.addEventListener("keydown", (e) => {
    if (e.key === "Escape") closeInspector();
});
