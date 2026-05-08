const $ = (id) => document.getElementById(id);
const conv = $("conversation");
const peerSelect = $("peer-select");
const peerCap = $("peer-cap");
const promptEl = $("prompt");
const sendBtn = $("send");
const refreshBtn = $("refresh-peers");
const selfId = $("self-id");

let knownPeers = [];

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
            opt.textContent = "(no peers in directory — run `n3ur0n peers refresh` or wait for bootstrap)";
            peerSelect.appendChild(opt);
            peerCap.textContent = "";
            sendBtn.disabled = true;
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
        if (firstChatIdx >= 0) {
            peerSelect.selectedIndex = firstChatIdx;
        }
        updateCapHint();
    } catch (e) {
        bubble("error", null, `Failed to load peers: ${e.message}`);
    }
}

function updateCapHint() {
    const opt = peerSelect.selectedOptions[0];
    if (!opt) {
        peerCap.textContent = "";
        sendBtn.disabled = true;
        return;
    }
    const caps = (opt.dataset.caps || "").split(",").filter(Boolean);
    if (caps.includes("chat")) {
        peerCap.textContent = `caps: ${caps.join(", ")}`;
        peerCap.style.color = "";
        sendBtn.disabled = false;
    } else if (caps.length > 0) {
        peerCap.textContent = `caps: ${caps.join(", ")} (no \`chat\` — pick another peer)`;
        peerCap.style.color = "var(--error)";
        sendBtn.disabled = true;
    } else {
        peerCap.textContent = "no cached capabilities — try `n3ur0n peers refresh`";
        peerCap.style.color = "var(--muted)";
        sendBtn.disabled = true;
    }
}

async function send() {
    const peer = peerSelect.value;
    const prompt = promptEl.value.trim();
    if (!peer) {
        bubble("error", null, "Pick a peer first.");
        return;
    }
    if (!prompt) return;

    bubble("user", "you", prompt);
    promptEl.value = "";
    sendBtn.disabled = true;
    const pending = bubble("assistant", peer, "…thinking…");

    try {
        const res = await fetch("/api/v0/chat", {
            method: "POST",
            headers: { "content-type": "application/json" },
            body: JSON.stringify({ peer_endpoint: peer, prompt }),
        });
        const body = await res.json();
        if (!res.ok) {
            pending.classList.replace("assistant", "error");
            pending.querySelector(".who").textContent = "error";
            pending.querySelector("span:last-child").textContent = body.error || JSON.stringify(body);
            return;
        }
        // invoke replies wrap the capability output in {result: ...}
        const result = body.reply?.result ?? body.reply ?? {};
        const message = result.message?.content
            ?? (typeof result === "string" ? result : JSON.stringify(result, null, 2));
        const model = result.model ? ` · ${result.model}` : "";
        pending.querySelector(".who").textContent = `${body.peer_id?.slice(0, 18) || peer}…${model}`;
        pending.querySelector("span:last-child").textContent = message;
    } catch (e) {
        pending.classList.replace("assistant", "error");
        pending.querySelector(".who").textContent = "error";
        pending.querySelector("span:last-child").textContent = e.message;
    } finally {
        sendBtn.disabled = false;
        promptEl.focus();
    }
}

peerSelect.addEventListener("change", updateCapHint);
sendBtn.addEventListener("click", send);
refreshBtn.addEventListener("click", loadPeers);
promptEl.addEventListener("keydown", (e) => {
    // Enter sends; Shift+Enter inserts a newline (default behavior).
    // Cmd/Ctrl+Enter also sends, kept for parity with other chat UIs.
    if (e.key === "Enter" && !e.shiftKey && !e.isComposing) {
        e.preventDefault();
        send();
    }
});

loadPeers();
