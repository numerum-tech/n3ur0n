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
