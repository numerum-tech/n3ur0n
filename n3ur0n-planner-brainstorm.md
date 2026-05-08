# N3UR0N — Brainstorm planner & multi-conversation

*Brainstorm 2026-05-08. Décisions arrêtées avant implémentation v0.1 du
capacity planner minimal. Document de référence ; spec officielle reste
`n3ur0n-architecture-v0.md`.*

---

## 1. Problème de départ

UI v0.1 force user à choisir manuellement peer + capability + form JSON.
Mauvais modèle mental : **le user devrait dialoguer uniquement avec son
instance**. L'instance lit le besoin en langage naturel, choisit où
dispatcher, compose les args, exécute, agrège, répond.

Spec §11.4 ("Localisation du planner") posait la question, reportée à
v0.2 ; on l'attaque maintenant.

## 2. Décision structurante : planner = capability `plan`

Le planner n'est **pas un singleton** dans l'architecture. C'est une
capability protocolaire (`plan`) qu'un nœud peut **exposer** ou
**consommer** comme n'importe quelle autre.

Conséquence : 3 modes utilisateur cohérents.

| Mode | Description | Privacy | Footprint |
|---|---|---|---|
| **A — Planner local** | Nœud a un LLM accessible, `/api/v0/converse` pipe direct | Max | Lourd |
| **B — Planner distant** | Nœud léger sans LLM, invoque `plan` chez un peer | Compromis | Zéro |
| **C — Manual** | Pas de planner accessible (ou désactivé). UI dropdown peer/cap | Max | Zéro |

Architecture spec §11.4 listait exactement ces options ("backend local",
"instance spécialisée", "service externe"). Spec §10 reste contraignante :
"Pas de pipelines orchestrés côté protocole" — le planner est intégré au
nœud (le sien ou un peer), jamais imposé par le protocole.

## 3. 4 niveaux d'implémentation du planner

Sous la même cap `plan`, plusieurs impls possibles :

| Niveau | Description | Footprint | Routing |
|---|---|---|---|
| 1 | `TrivialPlanner` | <1 KB | Forward direct vers premier peer offrant cap |
| 2 | `RuleBasedPlanner` | ~100 KB | Regex/keywords → match `cap.tags` |
| 3 | `LLMPlanner` (tiny) | ~400 MB | qwen2.5:0.5b + JSON prompt-eng + retry |
| 4 | `LLMPlanner` (standard) | ~1 GB+ | qwen2.5:7b/llama3.1:8b + tool-calling natif |

**v0.1 cible** : niveau 4 uniquement (LLMPlanner standard, modèle 7B-8B
sur Ollama host). Niveaux 1-3 = post-MVP.

### Hardware requirements LLM standard

LLM ne demande **pas** de GPU. RAM est la vraie contrainte :

| Modèle | RAM | CPU x86 4-core | Apple Silicon | GPU 8GB |
|---|---|---|---|---|
| qwen2.5:0.5b | 400 MB | 30+ tok/s | 60+ | 200+ |
| qwen2.5:1.5b | 1.2 GB | 15-30 tok/s | 30-60 | 100+ |
| llama3.2:3b | 2.4 GB | 7-15 tok/s | 20-40 | 80+ |
| qwen2.5:7b | 5 GB | 3-7 tok/s | 15-25 | 60+ |
| llama3.1:8b | 5 GB | 2-6 tok/s | 12-20 | 50+ |

CPU OK pour dev/single-user. Multi-user concurrent → file d'attente naturelle.

Ollama supporte multi-modèles loaded simultanément (cap par
`OLLAMA_MAX_LOADED_MODELS`, LRU eviction). 1 host peut servir N nœuds n3ur0n
sur des modèles différents.

## 4. Format tool-call

**Native (champ `tools` OpenAI)** retenu pour v0.1. Modèles 7B-8B le supportent
fiablement. Ollama ≥ 0.4 relaie. Alternative JSON prompt-engineered =
fallback éventuel pour modèles tiny (post-MVP).

Forme :
```jsonc
// requête au LLM
{
  messages: [...],
  tools: [{
    type: "function",
    function: {
      name: "<peer_id>::<cap_name>",
      description: "...",
      parameters: <schema_in JSON Schema>
    }
  }, ...]
}

// réponse possible
{
  message: {
    role: "assistant",
    tool_calls: [{
      id: "call_xxx",
      type: "function",
      function: { name: "<peer_id>::<cap>", arguments: "<JSON string>" }
    }]
  }
}
```

Tool naming : `<short_peer_id>::<cap>`. Permet au LLM de viser nœud précis
ou cap générique.

`plan` exclu du catalog côté planner (éviter récursion plan → plan).

## 5. Multi-session : isolation par `client_id`

Pas d'auth (LAN trusted). Plusieurs browsers/Tauri en parallèle, chacun
ne voit que ses propres conversations.

**Mécanisme** :
- Cookie `n3ur0n_client_id` HttpOnly + SameSite=Lax + Max-Age 1 an.
- Server génère UUID v4 au premier hit, set-cookie.
- Tower middleware extrait client_id, injecte dans request extensions.
- API filtres `WHERE client_id = ?`.
- Tauri webview accepte cookies sur localhost normalement.

**Limites assumées** :
- Clear cookies = perte des conversations (DB intacte mais inaccessible).
- Pas de sync cross-device (= comptes user, v0.3).
- UUID v4 = 122 bits entropie, brute-force impraticable.
- 1 cookie partagé entre tabs même browser → bon UX.
- Mode incognito → cookie séparé → conversations isolées.

## 6. Mémoire des conversations

### Modèle 2-tiers

```
SQLite (source of truth, persistent)
   ↑
   │ write-through par turn (transaction atomique)
   │
LRU cache mémoire (hot conversations, max_active_conversations)
```

### Granularité du turn

```rust
enum Turn {
    User { content, ts },
    Assistant { content, model?, ts },
    ToolCall { peer_id, capability, args, ts },
    ToolResult { peer_id, capability, result, error?, ts },
    System { content, ts },
}
```

Chaque dispatch utilisateur = 1 User turn + 0..N pairs (ToolCall, ToolResult)
+ 1 Assistant turn final.

### Schema persistent

```sql
conversations (
  id TEXT PRIMARY KEY,
  client_id TEXT NOT NULL,
  title TEXT,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
)
INDEX (client_id, updated_at DESC)

conversation_turns (
  conversation_id TEXT NOT NULL,
  seq INTEGER NOT NULL,            -- monotone par conv
  role TEXT NOT NULL,
  payload TEXT NOT NULL,           -- JSON Turn
  created_at INTEGER NOT NULL,
  PRIMARY KEY (conversation_id, seq),
  FOREIGN KEY conversation_id ON DELETE CASCADE
)
```

### Restauration

Cache hit → use it. Cache miss → SELECT all turns → reconstruct
ConversationState → cache.put. Cold load = 10-50ms typique.

### Mapping LLM

`state.to_chat_messages(format)` convertit :
- User → `{role: user}`
- Assistant → `{role: assistant}`
- ToolCall → `{role: assistant, tool_calls: [...]}`
- ToolResult → `{role: tool, tool_call_id, content}`
- System → `{role: system}`

### Crash recovery

WAL durable. Crash mid-dispatch → dernier turn possiblement non commité.
Edge case : crash entre INSERT tool_call et INSERT tool_result → état
restoré a tool_call dangling.

`validate_state` au reload :
- Si dernier turn = ToolCall sans ToolResult appairé → injecter
  ToolResult synthétique `{error: "instance restarted before result"}`.
- Le LLM voit "appel échoué" et peut retenter.

### Pruning context window

LLM context fini : qwen2.5:7b ~32k, llama3.1:8b ~128k. Long historique
dépasse.

Stratégie v0.1 : **drop oldest avec préservation paires**.
- Prendre N derniers turns (constante `MAX_CONTEXT_TURNS = 16`).
- Si découpage tombe entre ToolCall et ToolResult, ajuster pour garder
  la paire (ou la couper toute entière).
- UI affiche tout l'historique (depuis DB). Seul le LLM voit la fenêtre.

v0.2 = summarisation périodique. v0.3 = embeddings sémantique.

### Title auto-généré

`first_user_message.split_whitespace().take(N).join(" ")`. Constante
`title_auto_max_words = 8`. Stocké à création. PATCH pour rename manuel.

## 7. Concurrency

```
request → middleware client_id
       → ownership check (404 si conv pas au client)
       → conv_lock[id].lock()           # mutex per conversation
       → planner_slots.acquire()         # semaphore global
       → load state (cache or repo)
       → planner.dispatch(state, message)
       → for each turn produced: persist atomique
       → cache.put
       → release locks
       → return reply + trace
```

**Mutex per conversation** : 2 onglets sur même conv → sérialisé. Onglet 2
attend l'achèvement de la dispatch en cours sur l'onglet 1.

**Sémaphore planner global** : `max_concurrent_planners` (default 4).
Backpressure naturel sous load LLM. File d'attente si saturé.

**Politique au cap** : queue avec timeout (30s default), sinon 503.

## 8. Limites configurables

```
max_concurrent_planners = 4         # parallel dispatches LLM
max_active_conversations = 50       # cache LRU hot
max_total_conversations = 10_000    # cap globale
conversation_idle_ttl_sec = 600     # éviction cache mémoire
max_context_turns = 16              # window LLM
max_tool_turns = 6                  # boucle plan→call→observe
title_auto_max_words = 8
```

## 9. API surface (locale, non signée)

```
GET    /api/v0/whoami                  → instance_id local
GET    /api/v0/peers                   → directory + caps cached
POST   /api/v0/peers/refresh           → signed describe_self peer + upsert
POST   /api/v0/peers/discover          → cascade depth-1 capability

POST   /api/v0/conversations           → {id, title, created_at}
GET    /api/v0/conversations           → list filtered by client_id
GET    /api/v0/conversations/:id        → ownership + turns
PATCH  /api/v0/conversations/:id        → rename
DELETE /api/v0/conversations/:id        → cascade
POST   /api/v0/conversations/:id/messages → handle_user_message → reply+trace

POST   /api/v0/chat                    # advanced/manual mode (existant)
POST   /api/v0/invoke                  # advanced/manual mode (existant)
```

Toutes (sauf cookies admin) lisent `client_id` du cookie.

## 10. UI cible

Layout 2 colonnes :
```
┌────────────────────┬─────────────────────────┐
│ + New chat         │  N3UR0N — <title>       │
│                    │ ────────────────────────│
│ • Conversation 1   │   [history bubbles]     │
│ • Conversation 2   │   user / assistant /    │
│ • Conversation 3   │   tool_call+result      │
│ ────────           │                         │
│ Older              │   [composer input]      │
│ • ...              │                         │
└────────────────────┴─────────────────────────┘
```

- Sidebar = GET /api/v0/conversations, sort updated_at DESC.
- Click item → load turns + main pane.
- "+ New chat" = POST /api/v0/conversations.
- Composer = single textarea, Enter = send.
- Trace par turn assistant = collapsible (toggle pour voir tool_call/result).
- localStorage : `current_conversation_id` per tab.
- Pas de live update entre onglets v0.1 (F5 manuel).

## 11. Limites assumées (récap)

1. Pas d'isolation forte vs malicious LAN (cookie spoofing théorique).
2. Crash mid-dispatch = dernier turn possiblement incomplet (auto-récup).
3. Pas de live update entre onglets (skip SSE v0.1).
4. Pas de sync multi-device.
5. Mode B (planner exposé au réseau) = v0.2.
6. LLM hallucination peer/cap : valider chaque tool_call contre catalog.
7. Coût opaque : un user message peut générer N appels payants. Bornes :
   `max_tool_turns=6`, caps `restricted` interdites en Mode B v0.1.
8. Latency : multi-hop séquentiel, jusqu'à ~30-60s. UI "thinking…".
9. Privacy : prompts user vont au LLM local. Si Ollama distant → fuite.
10. Conversation context grossit : pruning naïf v0.1.
11. Stateful capabilities : ré-envoi historique chaque appel.
12. Pas de search v0.1, pas d'export, pas de cross-conversation memory.
13. Pas de garbage collection conversations orphelines.

## 12. Hors scope v0.1 (post-MVP)

- PlanBackend exposant `plan` au réseau (Mode B distant)
- TrivialPlanner / RuleBasedPlanner / LLM-tiny variants
- Advanced UI toggle (peer/cap manual gardé en parallèle)
- HITL avant tool execution
- SSE/WebSocket pour live update entre onglets
- Conversation summarization périodique
- Cross-conversation long-term memory
- FTS search dans conversations
- Auth + multi-user vrai (v0.3)
- Garbage collection conversations orphelines
- Pruning sémantique
- Encryption E2E pour Mode B

## 13. Plan d'implémentation v0.1

Voir `~/.claude/plans/je-n-ai-pas-compris-synchronous-umbrella.md` pour
les phases A-I détaillées et les fichiers touchés.

Ordre :
- A. Schema 0002 + repo conversations
- B. ConversationState + Turn enum
- C. NodeRuntime (semaphore + locks + LRU)
- D. Planner trait + LLMPlanner native + catalog + tool_call
- E. OpenAIBackend extension `tools`
- F. HTTP middleware client_id + API conversations
- G. UI sidebar + composer
- H. Bootstrap flags + compose update
- I. Smoke + docs

Default modèle : **llama3.1:8b** (tool-calling solide en anglais) ou
**qwen2.5:7b** (multi-langue + tool-calling). User pull via
`ollama pull llama3.1:8b` côté host avant smoke cluster.
