const $ = (id) => document.getElementById(id);
const conv = $("conversation");
const peerSelect = $("peer-select");
const capSelect = $("cap-select");
const peerCap = $("peer-cap");
const composer = $("composer");
const refreshBtn = $("refresh-peers");
const discoverBtn = $("discover-chat");
const clearBtn = $("clear-conv");
const selfId = $("self-id");

let knownPeers = [];
const history = new Map(); // key = peer_endpoint::capability -> [{role, content}]

function key() {
    return `${peerSelect.value}::${capSelect.value}`;
}
function getHistory() {
    if (!history.has(key())) history.set(key(), []);
    return history.get(key());
}

// ---------------------------------------------------------------------------
// Conversation rendering
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Peer + capability selection
// ---------------------------------------------------------------------------

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
            peerSelect.appendChild(new Option("(no peers — click `discover chat`)", ""));
            updateCapabilities();
            return;
        }
        let firstChatIdx = -1;
        knownPeers.forEach((p, idx) => {
            const opt = document.createElement("option");
            opt.value = p.endpoint;
            const caps = (p.capabilities || []).map(c => typeof c === "string" ? { name: c } : c);
            p._caps = caps;
            const names = caps.map(c => c.name).filter(Boolean);
            const tag = names.length ? ` [${names.join(",")}]` : "";
            opt.textContent = `${p.alias || p.instance_id.slice(0, 18) + "…"}  ${p.endpoint}${tag}`;
            opt.dataset.idx = idx;
            peerSelect.appendChild(opt);
            if (firstChatIdx === -1 && names.includes("chat")) firstChatIdx = idx;
        });
        if (firstChatIdx >= 0) peerSelect.selectedIndex = firstChatIdx;
        updateCapabilities();
    } catch (e) {
        bubble("error", null, `Failed to load peers: ${e.message}`);
    }
}

function updateCapabilities() {
    capSelect.innerHTML = "";
    const peer = selectedPeer();
    if (!peer) {
        capSelect.disabled = true;
        capSelect.appendChild(new Option("(no peer)", ""));
        peerCap.textContent = "";
        renderComposer(null);
        return;
    }
    const caps = peer._caps || [];
    if (caps.length === 0) {
        capSelect.disabled = true;
        capSelect.appendChild(new Option("(no cached caps)", ""));
        peerCap.textContent = "no cached capabilities — try `refresh list`";
        peerCap.style.color = "var(--muted)";
        renderComposer(null);
        return;
    }
    capSelect.disabled = false;
    for (const c of caps) capSelect.appendChild(new Option(c.name, c.name));
    if (caps.find(c => c.name === "chat")) capSelect.value = "chat";
    onCapabilityChange();
}

function onCapabilityChange() {
    const decl = selectedCapability();
    if (!decl) {
        peerCap.textContent = "";
        renderComposer(null);
        return;
    }
    if (decl.description) {
        peerCap.textContent = decl.description.length > 140
            ? decl.description.slice(0, 140) + "…"
            : decl.description;
    } else {
        peerCap.textContent = "";
    }
    peerCap.style.color = "";
    renderComposer(decl);
    renderConversation();
}

// ---------------------------------------------------------------------------
// Composer: form generated from capability schema_in
// ---------------------------------------------------------------------------

let collectArgs = () => ({}); // overwritten by renderComposer

function renderComposer(decl) {
    composer.innerHTML = "";
    if (!decl) {
        collectArgs = () => null;
        return;
    }

    if (decl.name === "chat") {
        renderChatComposer();
        return;
    }

    // Generic capability — try to render form from schema_in.
    const fields = document.createElement("div");
    fields.className = "composer-fields";

    const ctx = pickSchemaBranch(decl.schema_in);
    if (!ctx) {
        // Schema unusable — fall back to raw JSON textarea.
        const ta = document.createElement("textarea");
        ta.rows = 4;
        ta.className = "json-fallback";
        ta.placeholder = 'JSON args, e.g. {"x":1}';
        ta.value = "{}";
        fields.appendChild(ta);
        collectArgs = () => parseJsonOrThrow(ta.value);
    } else {
        const inputs = renderObjectFields(ctx);
        fields.appendChild(inputs.root);
        collectArgs = () => inputs.collect();
    }

    const sendBtn = document.createElement("button");
    sendBtn.type = "button";
    sendBtn.id = "send";
    sendBtn.textContent = `Send`;
    sendBtn.addEventListener("click", send);

    composer.appendChild(fields);
    composer.appendChild(sendBtn);
}

function renderChatComposer() {
    const ta = document.createElement("textarea");
    ta.id = "prompt";
    ta.rows = 3;
    ta.placeholder = "Type a prompt and press Enter (Shift+Enter = newline)…";
    ta.addEventListener("keydown", (e) => {
        if (e.key === "Enter" && !e.shiftKey && !e.isComposing) {
            e.preventDefault();
            send();
        }
    });

    const sendBtn = document.createElement("button");
    sendBtn.type = "button";
    sendBtn.id = "send";
    sendBtn.textContent = "Send";
    sendBtn.addEventListener("click", send);

    composer.appendChild(ta);
    composer.appendChild(sendBtn);

    collectArgs = () => {
        const text = ta.value.trim();
        if (!text) return null;
        ta.value = "";
        return { prompt: text };
    };
}

// Pick a usable schema branch. Handles {oneOf:[...]} and plain objects.
function pickSchemaBranch(schema) {
    if (!schema || typeof schema !== "object") return null;
    if (Array.isArray(schema.oneOf) && schema.oneOf.length > 0) {
        return schema.oneOf[0];
    }
    if (schema.type === "object" || schema.properties) return schema;
    if (!schema.type) return schema; // permissive
    return null;
}

function renderObjectFields(schema) {
    const root = document.createElement("div");
    root.className = "composer-fields";

    const props = schema.properties || {};
    const required = new Set(schema.required || []);
    const entries = Object.entries(props);

    if (entries.length === 0) {
        const note = document.createElement("div");
        note.className = "field";
        note.innerHTML = `<span class="desc">No declared fields. Click Send to invoke with empty args.</span>`;
        root.appendChild(note);
        return { root, collect: () => ({}) };
    }

    const widgets = [];
    for (const [name, prop] of entries) {
        const w = renderField(name, prop, required.has(name));
        widgets.push(w);
        root.appendChild(w.el);
    }
    return {
        root,
        collect() {
            const out = {};
            for (const w of widgets) {
                const v = w.read();
                if (v !== undefined) out[w.name] = v;
            }
            return out;
        }
    };
}

function renderField(name, prop, isRequired) {
    const el = document.createElement("div");
    el.className = "field";
    const lbl = document.createElement("label");
    lbl.innerHTML = `<span>${name}</span>${isRequired ? '<span class="req">*</span>' : ""}<span class="desc">${typeLabel(prop)}</span>`;
    el.appendChild(lbl);

    const t = Array.isArray(prop.type) ? prop.type[0] : prop.type;

    if (Array.isArray(prop.enum) && prop.enum.length > 0) {
        const sel = document.createElement("select");
        if (!isRequired) sel.appendChild(new Option("(unset)", ""));
        for (const v of prop.enum) sel.appendChild(new Option(String(v), String(v)));
        el.appendChild(sel);
        return { name, el, read: () => sel.value === "" ? undefined : sel.value };
    }

    if (t === "boolean") {
        const cb = document.createElement("input");
        cb.type = "checkbox";
        el.appendChild(cb);
        return { name, el, read: () => cb.checked };
    }

    if (t === "integer" || t === "number") {
        const inp = document.createElement("input");
        inp.type = "number";
        if (t === "integer") inp.step = "1";
        if (prop.example !== undefined) inp.placeholder = String(prop.example);
        el.appendChild(inp);
        return {
            name, el,
            read() {
                if (inp.value === "") return undefined;
                const n = t === "integer" ? parseInt(inp.value, 10) : parseFloat(inp.value);
                return Number.isFinite(n) ? n : undefined;
            }
        };
    }

    if (t === "array" || t === "object") {
        const ta = document.createElement("textarea");
        ta.rows = 2;
        ta.className = "json-fallback";
        ta.placeholder = t === "array" ? "JSON array, e.g. [1,2,3]" : 'JSON object, e.g. {"k":"v"}';
        el.appendChild(ta);
        return {
            name, el,
            read() {
                const txt = ta.value.trim();
                if (txt === "") return undefined;
                try { return JSON.parse(txt); }
                catch (e) { throw new Error(`field "${name}" is not valid JSON: ${e.message}`); }
            }
        };
    }

    // Default: string. Use textarea if name suggests long content.
    const long = /prompt|message|content|text|body/i.test(name);
    const inp = long ? document.createElement("textarea") : document.createElement("input");
    if (long) inp.rows = 3;
    else inp.type = "text";
    if (prop.example !== undefined) inp.placeholder = String(prop.example);
    el.appendChild(inp);
    return {
        name, el,
        read() {
            const v = inp.value;
            return v === "" ? undefined : v;
        }
    };
}

function typeLabel(prop) {
    if (Array.isArray(prop.enum)) return `enum`;
    const t = Array.isArray(prop.type) ? prop.type.join("|") : prop.type;
    return t || "any";
}

function parseJsonOrThrow(s) {
    if (s.trim() === "") return {};
    try { return JSON.parse(s); }
    catch (e) { throw new Error(`Invalid JSON: ${e.message}`); }
}

// ---------------------------------------------------------------------------
// Send
// ---------------------------------------------------------------------------

async function send() {
    const peer = peerSelect.value;
    const cap = capSelect.value;
    if (!peer || !cap) {
        bubble("error", null, "Pick a peer and a capability first.");
        return;
    }
    let args;
    try {
        args = collectArgs();
    } catch (e) {
        bubble("error", null, e.message);
        return;
    }
    if (args === null) return; // empty chat prompt

    const sendBtn = composer.querySelector("button");
    if (sendBtn) sendBtn.disabled = true;
    try {
        if (cap === "chat") {
            await sendChat(args.prompt);
        } else {
            await sendInvoke(args);
        }
    } finally {
        if (sendBtn) sendBtn.disabled = false;
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

async function sendInvoke(args) {
    const peer = peerSelect.value;
    const cap = capSelect.value;
    const hist = getHistory();
    const pretty = JSON.stringify(args);
    hist.push({ role: "user", content: pretty });
    bubble("user", "you", pretty);

    const pending = bubble("assistant", `${peer} · ${cap}`, "…invoking…");
    try {
        const res = await fetch("/api/v0/invoke", {
            method: "POST",
            headers: { "content-type": "application/json" },
            body: JSON.stringify({ peer_endpoint: peer, capability: cap, args }),
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

// ---------------------------------------------------------------------------
// Discovery / clear
// ---------------------------------------------------------------------------

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
refreshBtn.addEventListener("click", loadPeers);
discoverBtn.addEventListener("click", discoverChat);
clearBtn.addEventListener("click", clearConversation);

loadPeers();
