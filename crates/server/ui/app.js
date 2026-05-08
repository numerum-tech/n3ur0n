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
            const caps = p.capabilities || [];
            const tag = caps.length ? ` [${caps.join(",")}]` : "";
            opt.textContent = `${p.alias || p.instance_id.slice(0, 18) + "…"}  ${p.endpoint}${tag}`;
            opt.dataset.caps = caps.join(",");
            peerSelect.appendChild(opt);
            if (firstChatIdx === -1 && caps.includes("chat")) {
                firstChatIdx = idx;
            }
        });
        if (firstChatIdx >= 0) peerSelect.selectedIndex = firstChatIdx;
        updateCapabilities();
    } catch (e) {
        bubble("error", null, `Failed to load peers: ${e.message}`);
    }
}

function updateCapabilities() {
    const opt = peerSelect.selectedOptions[0];
    capSelect.innerHTML = "";
    if (!opt || !opt.value) {
        capSelect.disabled = true;
        capSelect.appendChild(new Option("(no peer)", ""));
        peerCap.textContent = "";
        sendBtn.disabled = true;
        return;
    }
    const caps = (opt.dataset.caps || "").split(",").filter(Boolean);
    if (caps.length === 0) {
        capSelect.disabled = true;
        capSelect.appendChild(new Option("(no cached caps — refresh)", ""));
        peerCap.textContent = "no cached capabilities — try `refresh list`";
        peerCap.style.color = "var(--muted)";
        sendBtn.disabled = true;
        return;
    }
    capSelect.disabled = false;
    for (const c of caps) capSelect.appendChild(new Option(c, c));
    if (caps.includes("chat")) capSelect.value = "chat";
    onCapabilityChange();
}

function onCapabilityChange() {
    const cap = capSelect.value;
    if (cap === "chat") {
        peerCap.textContent = "multi-turn chat (history kept per peer+cap)";
        peerCap.style.color = "";
        promptEl.placeholder = "Type a prompt and press Enter (Shift+Enter = newline)…";
        sendBtn.disabled = false;
    } else if (cap) {
        peerCap.textContent = `invoke ${cap} — payload below treated as JSON args`;
        peerCap.style.color = "";
        promptEl.placeholder = `JSON args for capability "${cap}", e.g. {"x":1}`;
        sendBtn.disabled = false;
    } else {
        peerCap.textContent = "";
        sendBtn.disabled = true;
    }
    renderConversation();
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
