const $ = (id) => document.getElementById(id);
const conv = $("conversation");
const peerSelect = $("peer-select");
const capSelect = $("cap-select");
const peerCap = $("peer-cap");
const promptEl = $("prompt");
const sendBtn = $("send");
const refreshBtn = $("refresh-peers");
const discoverBtn = $("discover-chat");
const clearBtn = $("clear-conv");
const selfId = $("self-id");

let knownPeers = [];
// Conversation history — per (peer_endpoint, capability) pair.
const history = new Map();

function key() {
    return `${peerSelect.value}::${capSelect.value}`;
}

function getHistory() {
    if (!history.has(key())) history.set(key(), []);
    return history.get(key());
}

function bubble(kind, who, text) {
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

function renderConversation() {
    conv.innerHTML = "";
    for (const m of getHistory()) {
        const kind = m.role === "user" ? "user" : (m.role === "assistant" ? "assistant" : "system");
        bubble(kind, m.role === "user" ? "you" : (m.who || m.role), m.content);
    }
}

async function loadPeers() {
    peerSelect.innerHTML = "<option value=''>(loading…)</option>";
    try {
        const res = await fetch("/api/v0/peers");
        if (!res.ok) throw new Error(`HTTP ${res.status}`);
        const body = await res.json();
        selfId.textContent = body.self || "?";
        knownPeers = body.peers || [];

        peerSelect.innerHTML = "";
        if (knownPeers.length === 0) {
            const opt = document.createElement("option");
            opt.value = "";
            opt.textContent = "(no peers — click `discover chat`)";
            peerSelect.appendChild(opt);
            sendBtn.disabled = true;
            updateCapabilities();
            return;
        }
        let firstChatIdx = -1;
        knownPeers.forEach((p, idx) => {
            const opt = document.createElement("option");
            opt.value = p.endpoint;
            const caps = (p.capabilities || []).map(c => typeof c === "string" ? { name: c } : c);
            p._caps = caps; // cache for capability dropdown
            const names = caps.map(c => c.name).filter(Boolean);
            const tag = names.length ? ` [${names.join(",")}]` : "";
            opt.textContent = `${p.alias || p.instance_id.slice(0, 18) + "…"}  ${p.endpoint}${tag}`;
            opt.dataset.idx = idx;
            peerSelect.appendChild(opt);
            if (firstChatIdx === -1 && names.includes("chat")) {
                firstChatIdx = idx;
            }
        });
        if (firstChatIdx >= 0) peerSelect.selectedIndex = firstChatIdx;
        updateCapabilities();
    } catch (e) {
        bubble("error", null, `Failed to load peers: ${e.message}`);
    }
}

function selectedPeer() {
    const opt = peerSelect.selectedOptions[0];
    if (!opt || !opt.value) return null;
    const idx = parseInt(opt.dataset.idx, 10);
    return Number.isFinite(idx) ? knownPeers[idx] : null;
}

function selectedCapability() {
    const peer = selectedPeer();
    if (!peer) return null;
    return (peer._caps || []).find(c => c.name === capSelect.value) || null;
}

function updateCapabilities() {
    capSelect.innerHTML = "";
    const peer = selectedPeer();
    if (!peer) {
        capSelect.disabled = true;
        capSelect.appendChild(new Option("(no peer)", ""));
        peerCap.textContent = "";
        sendBtn.disabled = true;
        return;
    }
    const caps = peer._caps || [];
    if (caps.length === 0) {
        capSelect.disabled = true;
        capSelect.appendChild(new Option("(no cached caps — refresh)", ""));
        peerCap.textContent = "no cached capabilities — try `refresh list`";
        peerCap.style.color = "var(--muted)";
        sendBtn.disabled = true;
        return;
    }
    capSelect.disabled = false;
    for (const c of caps) capSelect.appendChild(new Option(c.name, c.name));
    if (caps.find(c => c.name === "chat")) capSelect.value = "chat";
    onCapabilityChange();
}

// Last JSON skeleton we auto-prefilled — used to detect "user hasn't edited
// it" so we can safely overwrite on cap change.
let lastAutoPrefill = "";

function onCapabilityChange() {
    const cap = capSelect.value;
    const decl = selectedCapability();

    // If the textarea still holds the previous auto-prefill, clear it before
    // the cap-specific branches run.
    if (lastAutoPrefill && promptEl.value === lastAutoPrefill) {
        promptEl.value = "";
        lastAutoPrefill = "";
    }

    if (cap === "chat") {
        peerCap.textContent = decl?.description
            ? `${decl.description.slice(0, 80)}${decl.description.length > 80 ? "…" : ""}`
            : "multi-turn chat (history kept per peer+cap)";
        peerCap.style.color = "";
        promptEl.placeholder = "Type a prompt and press Enter (Shift+Enter = newline)…";
        sendBtn.disabled = false;
    } else if (cap) {
        const desc = decl?.description ? ` · ${decl.description.slice(0, 60)}` : "";
        peerCap.textContent = `invoke "${cap}" — JSON args required${desc}`;
        peerCap.style.color = "";
        const skel = jsonSkeleton(decl?.schema_in);
        promptEl.placeholder = `JSON args, e.g. ${skel}`;
        if (promptEl.value.trim() === "") {
            promptEl.value = skel;
            lastAutoPrefill = skel;
        }
        sendBtn.disabled = false;
    } else {
        peerCap.textContent = "";
        sendBtn.disabled = true;
    }
    renderConversation();
}

// Best-effort JSON skeleton from a JSON Schema fragment. Always returns valid
// JSON (`{}` as fallback). Handles top-level `oneOf` by picking the first
// branch's required props.
function jsonSkeleton(schema) {
    if (!schema || typeof schema !== "object") return "{}";
    const branch = Array.isArray(schema.oneOf) && schema.oneOf.length > 0
        ? schema.oneOf[0]
        : schema;
    const props = branch.properties || {};
    const required = branch.required || Object.keys(props).slice(0, 1);
    const out = {};
    for (const k of required) {
        const p = props[k] || {};
        out[k] = exampleFor(p);
    }
    return JSON.stringify(out);
}

function exampleFor(prop) {
    if (!prop || typeof prop !== "object") return null;
    if (prop.example !== undefined) return prop.example;
    const t = Array.isArray(prop.type) ? prop.type[0] : prop.type;
    if (Array.isArray(prop.enum) && prop.enum.length > 0) return prop.enum[0];
    switch (t) {
        case "string": return "";
        case "integer":
        case "number": return 0;
        case "boolean": return false;
        case "array": return [];
        case "object": return {};
        default: return null;
    }
}

async function sendChat(prompt) {
    const peer = peerSelect.value;
    const hist = getHistory();
    hist.push({ role: "user", content: prompt });
    bubble("user", "you", prompt);

    const pending = bubble("assistant", peer, "…thinking…");
    try {
        const res = await fetch("/api/v0/chat", {
            method: "POST",
            headers: { "content-type": "application/json" },
            body: JSON.stringify({
                peer_endpoint: peer,
                messages: hist.map(({ role, content }) => ({ role, content })),
            }),
        });
        const body = await res.json();
        if (!res.ok) throw new Error(body.error || JSON.stringify(body));
        const result = body.reply?.result ?? body.reply ?? {};
        const content = result.message?.content
            ?? (typeof result === "string" ? result : JSON.stringify(result, null, 2));
        const model = result.model ? ` · ${result.model}` : "";
        const who = `${(body.peer_id || peer).slice(0, 24)}…${model}`;
        pending.querySelector(".who").textContent = who;
        pending.querySelector("span:last-child").textContent = content;
        hist.push({ role: "assistant", content, who });
    } catch (e) {
        pending.classList.replace("assistant", "error");
        pending.querySelector(".who").textContent = "error";
        pending.querySelector("span:last-child").textContent = e.message;
    }
}

async function sendInvoke(rawArgs) {
    const peer = peerSelect.value;
    const cap = capSelect.value;
    const hist = getHistory();

    let args;
    try {
        args = rawArgs.trim() === "" ? {} : JSON.parse(rawArgs);
    } catch (e) {
        bubble("error", null, `Invalid JSON: ${e.message}`);
        return;
    }

    hist.push({ role: "user", content: rawArgs });
    bubble("user", "you", rawArgs);

    const pending = bubble("assistant", `${peer} · ${cap}`, "…invoking…");
    try {
        const res = await fetch("/api/v0/invoke", {
            method: "POST",
            headers: { "content-type": "application/json" },
            body: JSON.stringify({
                peer_endpoint: peer,
                capability: cap,
                args,
            }),
        });
        const body = await res.json();
        if (!res.ok) throw new Error(body.error || JSON.stringify(body));
        const result = body.reply?.result ?? body.reply ?? body;
        const display = JSON.stringify(result, null, 2);
        pending.querySelector(".who").textContent = `${(body.peer_id || peer).slice(0, 24)}…`;
        pending.querySelector("span:last-child").textContent = display;
        hist.push({ role: "assistant", content: display });
    } catch (e) {
        pending.classList.replace("assistant", "error");
        pending.querySelector(".who").textContent = "error";
        pending.querySelector("span:last-child").textContent = e.message;
    }
}

async function send() {
    if (!peerSelect.value) {
        bubble("error", null, "Pick a peer first.");
        return;
    }
    if (!capSelect.value) {
        bubble("error", null, "Pick a capability first.");
        return;
    }
    const text = promptEl.value.trim();
    if (!text) return;
    promptEl.value = "";
    sendBtn.disabled = true;
    try {
        if (capSelect.value === "chat") {
            await sendChat(text);
        } else {
            await sendInvoke(text);
        }
    } finally {
        sendBtn.disabled = false;
        promptEl.focus();
    }
}

async function discoverChat() {
    discoverBtn.disabled = true;
    bubble("system", null, "Discovering peers offering `chat`…");
    try {
        const res = await fetch("/api/v0/peers/discover", {
            method: "POST",
            headers: { "content-type": "application/json" },
            body: JSON.stringify({ capability: "chat" }),
        });
        const body = await res.json();
        if (!res.ok) throw new Error(body.error || JSON.stringify(body));
        bubble("system", null, `Discovery added ${body.added} new peer(s).`);
        await loadPeers();
    } catch (e) {
        bubble("error", null, `Discover failed: ${e.message}`);
    } finally {
        discoverBtn.disabled = false;
    }
}

function clearConversation() {
    history.set(key(), []);
    renderConversation();
}

peerSelect.addEventListener("change", updateCapabilities);
capSelect.addEventListener("change", onCapabilityChange);
sendBtn.addEventListener("click", send);
refreshBtn.addEventListener("click", loadPeers);
discoverBtn.addEventListener("click", discoverChat);
clearBtn.addEventListener("click", clearConversation);
promptEl.addEventListener("keydown", (e) => {
    if (e.key === "Enter" && !e.shiftKey && !e.isComposing) {
        e.preventDefault();
        send();
    }
});

loadPeers();
