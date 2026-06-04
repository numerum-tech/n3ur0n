# N3UR0N — Stack technique (draft 1)

*Statut : draft de travail. Reflète les décisions de stack au 8 mai 2026, révision 1 (ajout shell desktop Tauri et distinction consumer/publisher). Doit être lu en complément de `n3ur0n-architecture-v0.md`.*

**Changements depuis draft 0** : introduction de deux profils utilisateur (consumer / publisher), adoption de Tauri 2 comme shell desktop dès v0.1, factorisation de la frontend en codebase unique alimentant trois cibles, refactor du core Rust en lib + binaires.

**Amendement 2026-05-12** : alignement docs ↔ code après releases 0.2.0 et 0.3.0. Quatre écarts à connaître :

1. **Crate `node`** ajouté au workspace (§10.1). Layout effectif : `core`, `storage`, `adapters`, `node`, `server`, `desktop`. `node` est la couche orchestration (registry, handler, planner, runtime) entre `core` et `server`/`desktop`.
2. **Backend instantiation runtime, pas compile-time** (release 0.3.0). Le trait `Backend` reste tel quel mais les `impl` ne sont plus câblés en dur — un scan TOML produit la liste des backends actifs au boot. Cf. §7.1 (à lire avec cet amendement) et `n3ur0n-capability-manifest-v0.md`.
3. **Trois bindings de capacité implémentés v0.3** : `prompt` (prompted-LLM), `mcp` (tool MCP), `http` (forward générique). §7.2 listait `MCPBackend`, `OpenAIBackend`, `HTTPBackend`, `ProcessBackend` comme aspirations v0.1 ; la réalité est plus propre — les bindings remplacent les backends figés, et le `OpenAIBackend` historique est devenu le sous-jacent du binding `prompt`. `ProcessBackend` non implémenté (reporté, le besoin est en grande partie absorbé par `mcp` stdio).
4. **Hot-reload du registry via `ArcSwap`** (release 0.3.0). `Node.registry` est `Arc<ArcSwap<CapabilityRegistry>>`. Permet la CRUD de caps sans redémarrer le serveur.

**Amendement 2026-06-02** : alignement docs ↔ code après release **0.4.0** :

5. **Hot-reload des backends** (0.4.0). `Node.backends` est `Arc<ArcSwap<BackendsRegistry>>`. POST/DELETE `/api/v0/backends` reconstruit le registry et rebind les caps sans redémarrage.
6. **RBAC phase 1** (0.4.0). Migration `0003_users_sessions.sql`, rôles User / Operator / Admin, cookie de session, routes `/api/v0` protégées par permission. `N3UR0N_AUTH_DISABLE=1` pour le dev loopback. Le protocole pair (`/n3ur0n/v0`) reste signé Ed25519, indépendant des comptes locaux.
7. **`AccessMode::Private` implémenté** (0.4.0). Filtré de `describe_self` ; invoke réseau → `UnknownCapability`.

Le code prime sur les docs. Cf. règle de précédence CLAUDE.md.

---

## 1. Préambule

Ce document fige le stack technique retenu pour l'implémentation de la v0.1. Il est opiniâtre : il choisit, plutôt que de présenter des options. Les choix peuvent être révisés, mais ils ne sont pas dilués dans des conditionnels.

Le stack obéit à quatre contraintes structurantes :

1. **Distribution simple par profil utilisateur.** Consumer = installeur desktop natif. Publisher = single-binary serveur. Pas de runtime tiers, pas de service externe obligatoire.
2. **Une seule codebase UI.** La frontend est compilée une fois et embarquée dans plusieurs cibles : shell desktop (Tauri), serveur (rust-embed), mobile à terme (Capacitor / Tauri Mobile).
3. **Asymétrie consumer/publisher assumée.** Un consumer n'expose aucun listener public ; il est purement sortant. Un publisher expose `/n3ur0n/v0` derrière HTTPS. Le même core Rust sert les deux modes via configuration.
4. **Surface réseau hostile assumée côté publisher.** Crypto sur chaque message, parsing défensif, pas de dépendance fragile sur le hot path d'invocation.

---

## 2. Profils utilisateur

L'architecture v0.1 sert deux profils. Cette section définit leur posture technique avant tout détail de stack.

### 2.1 Consumer

Utilisateur final qui consomme les capacités du réseau sans en exposer.

- **Pas de listener pair public.** Aucune capacité offerte aux autres pairs. Architecture §10 (« invocation = requête-réponse ») garantit que les réponses arrivent sur la connexion sortante elle-même.
- **Pas de NAT à traverser, pas de cert TLS à provisionner, pas de nom de domaine.** Friction de déploiement nulle.
- **Shell desktop natif** (Tauri 2) attendu : double-clic sur installeur, ouvre application, prompt de bienvenue, pair de bootstrap par défaut, prêt à invoquer.
- **Identité Ed25519** générée au premier lancement et stockée dans `$APP_DATA/n3ur0n/keys.json`.

### 2.2 Publisher

Opérateur qui héberge une instance pour exposer des capacités au réseau.

- **Listener pair public obligatoire** sur HTTPS, accessible via nom de domaine ou IP routable.
- **Headless** typiquement : VPS, homelab, conteneur. Pas d'écran graphique disponible.
- **Web UI servie par le binaire serveur** via `rust-embed` (même bundle que la frontend desktop), accessible depuis n'importe quel navigateur de l'opérateur.
- **CLI** disponible pour scripts et bootstrap initial.

### 2.3 Conséquence pour le stack

Le binaire desktop et le binaire serveur partagent le core Rust et la frontend. Ils diffèrent par :

- Le shell qui héberge l'UI (Tauri vs rust-embed + axum).
- La configuration par défaut du listener pair (off vs on).
- Le pipeline d'installation (installeur OS natif vs binaire ou Docker).

---

## 3. Vue d'ensemble

```
┌────────────────────────────────────────────────────────┐
│  Codebase Rust (workspace cargo)                       │
│                                                        │
│  ┌─────────────┐  ┌─────────────┐  ┌──────────────┐    │
│  │  core (lib) │  │  adapters   │  │   storage    │    │
│  │  protocole  │  │  backend IA │  │   SQLite     │    │
│  │  crypto     │  │             │  │              │    │
│  │  types      │  │             │  │              │    │
│  └─────────────┘  └─────────────┘  └──────────────┘    │
│         ▲                ▲                 ▲           │
│         └────────────────┴─────────────────┘           │
│                          │                             │
│   ┌──────────────────────┴───────────────────────┐     │
│   │                                              │     │
│   ▼                                              ▼     │
│ ┌──────────────────┐                    ┌──────────────┐
│ │ n3ur0n-desktop   │                    │ n3ur0n-server│
│ │ (Tauri 2 shell)  │                    │ (axum + cli) │
│ │ webview locale   │                    │ HTTPS public │
│ │ pas listener pub │                    │ /n3ur0n/v0   │
│ └──────────────────┘                    │ /api/v0      │
│         ▲                                │ /ui          │
│         │                                └──────────────┘
│         │                                       ▲       │
│         └────────────┬──────────────────────────┘       │
│                      │                                  │
│                      ▼                                  │
│           ┌─────────────────────────┐                   │
│           │ frontend/build (Svelte) │                   │
│           │  UNIQUE codebase UI     │                   │
│           └─────────────────────────┘                   │
└────────────────────────────────────────────────────────┘
```

Tauri intègre core Rust comme lib en process. axum + rust-embed servent core comme binaire serveur HTTP. La frontend est compilée une fois et consommée par les deux shells.

---

## 4. Cœur instance

### 4.1 Langage et runtime

**Rust stable (edition 2024).**

Justification : crypto Ed25519 et SHA-256 sur chaque message du protocole, vérification anti-replay sous charge potentiellement adverse, parsing JSON exposé publiquement côté publisher. Hot path serré. Mémoire safe sans GC. Cohérence avec Tauri 2 (lui-même Rust) : un seul écosystème, IPC typé natif entre UI et core sans passer par HTTP.

Refus explicites :
- Node/Bun : event-loop fragile sous charge réseau adverse, déploiement single-binary contre nature, pas de path Tauri direct.
- Python : GIL, packaging single-binary difficile, perf insuffisante.
- Go : alternative défendable mais perdrait la cohérence avec Tauri (qui orienterait vers Wails et son écosystème séparé).

### 4.2 Async runtime

**`tokio` (full features).**

Single runtime pour HTTP serveur, clients HTTP sortants vers pairs, IO disque (SQLite via thread pool dédié). Côté Tauri, le même runtime est utilisé pour les commandes async exposées au webview.

### 4.3 Serveur HTTP (publisher uniquement)

**`axum` 0.7+.**

Trois sous-routeurs indépendants montés sur le même listener :

- `/n3ur0n/v0` — protocole pair. Middleware de vérification de signature obligatoire en amont. Aucune autre auth. Path strict, pas de wildcard remontant.
- `/api/v0` — API locale consommée par UI web ou client CLI distant. Middleware d'auth par token local. CORS restreint.
- `/ui` et `/ui/*` — assets statiques embarqués via `rust-embed`. CSP strict. Pas d'auth (l'UI bootstrap son token via injection HTML loopback-only).

### 4.4 Client HTTP sortant (publisher et consumer)

**`reqwest` (rustls backend).**

Pour invocations sortantes vers d'autres pairs et adaptateurs backend HTTP. `rustls` partout pour cohérence et portabilité. Pas d'OpenSSL.

### 4.5 TLS (publisher uniquement)

**`rustls` + `rustls-pemfile`.**

Modes supportés v0.1 :

- **Cert auto-signé** (défaut). Généré au premier démarrage si absent. TOFU côté pair : la première interaction enregistre l'empreinte du cert dans le répertoire local, les suivantes la vérifient. La signature applicative protège déjà l'intégrité ; TLS sert au chiffrement transport et à la résistance aux MITM passifs.
- **Cert fourni par l'opérateur** (PEM dans le répertoire de config). Pour déploiements derrière nom de domaine.
- **ACME / Let's Encrypt** via `instant-acme`. Opt-in v0.1, défaut v0.2.

mTLS optionnel pour déploiements en environnement contrôlé. Pas de défaut.

Côté consumer : pas de listener, donc pas de cert serveur. Vérification TOFU des certs des publishers contactés en sortant.

---

## 5. Crypto et identité

### 5.1 Signatures

**`ed25519-dalek` 2.x.**

Génération de paires, signature, vérification. Choisi pour audit, doc, et stabilité.

### 5.2 Hash

**`sha2`** pour SHA-256 sur clé publique → identifiant canonique.

### 5.3 Encodage identifiant

**`data-encoding`** pour Base32 RFC 4648 sans padding. Format final : `n3:<base32>`.

### 5.4 Sérialisation canonique pour signature

**`serde_jcs`** (JSON Canonicalization Scheme, RFC 8785).

Critique : sans canonicalisation, deux serializers JSON divergent et invalident silencieusement les signatures. La signature couvre le JCS de la concaténation des cinq premiers champs du message (`sender_id`, `recipient_id`, `timestamp`, `nonce`, `payload`).

### 5.5 Stockage clé privée

- **Desktop** : `$APP_DATA/n3ur0n/keys.json` (résolu via `tauri::path::BaseDirectory::AppData`), permissions `0600` sur Unix, ACL utilisateur sur Windows.
- **Server** : `$XDG_CONFIG_HOME/n3ur0n/keys.json` (par défaut `~/.config/n3ur0n/`), permissions `0600`.

Pas de chiffrement v0.1 (compromis assumé documenté dans architecture §5.3).

---

## 6. Stockage local

### 6.1 Moteur

**SQLite via `rusqlite` (bundled).**

Embarqué (pas de service externe), single-file, transactions ACID, performance largement suffisante à l'échelle v0.1. `bundled` = sqlite linké statiquement, pas de dépendance système. Identique pour desktop et server.

Pool de connexions via `r2d2` ou approche "connection per task" simple. Migrations via `refinery` ou SQL plat versionné.

### 6.2 Schéma minimal v0.1 (mis à jour 2026-05-12)

- `peers(id, endpoint, alias, last_seen, tls_fingerprint, describe_self_cached, describe_self_fetched_at, source)` — répertoire local, plafond 1000 entrées, éviction LRU.
- `nonces(sender_id, nonce, seen_at)` — anti-replay, TTL 1h, cleanup périodique. Index unique sur `(sender_id, nonce)`. **Publisher uniquement** (consumer ne reçoit pas de messages signés entrants v0.1).
- `conversations(id, client_id, title, created_at, ...)` + `conversation_turns(...)` — historique de chat côté consumer (ajouté en 0.2.0 avec le planner). Cookie `n3ur0n_client_id` pour isolation multi-client local.

**Tables planifiées draft 1 mais finalement déplacées hors SQLite** :
- `subscriptions` — pas encore implémentée (les modes `restricted` v0.3 fonctionnent par whitelist en mémoire / config).
- `capabilities` — **supprimée du schéma**. Les caps sont désormais déclarées par fichiers TOML (`caps/*.toml` + `backends/*.toml`) scannés au boot et hot-reloadés via `ArcSwap`. SQLite n'est plus la source de vérité du registre.
- `audit_log` — pas encore implémentée v0.3 ; `tracing` + sortie JSON couvre les besoins immédiats.

### 6.3 Refus

Postgres, MySQL, Redis : exclus v0.1. Service externe = dépendance opérateur, contre la promesse single-binary et incompatible avec installeur desktop.

---

## 7. Adaptateurs backend (publisher uniquement)

### 7.1 Trait (mis à jour 2026-05-12)

Le trait `Backend` reste le contrat d'adaptation, mais depuis la release 0.3.0 les implémentations ne sont plus *câblées en dur dans le binaire* — elles sont **instanciées au runtime à partir d'un scan TOML** :

- `backends/<name>.toml` déclare un backend (type, endpoint, modèle, secrets référencés).
- `caps/<name>.toml` déclare une capacité avec son `binding.backend = "<name>"`.

Conséquence : un opérateur ajoute / retire / reconfigure des backends sans recompiler n3uron. Le registre est rechargé à chaud (`ArcSwap`) sur événement filesystem ou sur commande API.

Trois types de binding supportés v0.3 : `prompt` (prompted LLM), `mcp` (tool d'un MCP server), `http` (forward HTTP générique).

### 7.2 Implémentations effectives v0.3 (corrige version draft 1)

Le draft 1 listait quatre backends figés (`MCPBackend`, `OpenAIBackend`, `HTTPBackend`, `ProcessBackend`). L'implémentation réelle s'est restructurée :

- **Binding `prompt`** — prompted-LLM. S'appuie sur `OpenAIBackend` sous-jacent (couvre OpenAI, Ollama, vLLM, llama.cpp server). Une cap = un system prompt + une référence à un backend `openai-compatible`. Couvre 80% des cas d'usage attendus.
- **Binding `mcp`** — connexion à un MCP server (stdio ou HTTP), un tool MCP par cap. Couvre la compat MCP recherchée par architecture §9.4.
- **Binding `http`** — forward HTTP générique avec templating d'args et résolution de secrets. Pour wrap rapide d'API existantes.

Backends de support disponibles en `crates/adapters/` :
- `EchoBackend` — identité, pour tests + smoke.
- `OpenAIBackend` — implémentation concrète sous-jacente au binding `prompt`.
- `UtilityBackend` — capacités utilitaires locales (typiquement déclarées `Private`).

**Non implémenté v0.3, reporté** :
- Binding `subprocess` générique (besoin largement absorbé par `mcp` stdio, à reconsidérer si retour utilisateur).
- Binding `wasm` (sandbox WASM pour caps locales sécurisées, cible v0.4+).

### 7.3 Note sur consumer

Un consumer pur n'a pas de backend exposé, mais peut déclarer un backend local **privé** consommable uniquement depuis sa propre UI (pas via le protocole pair). Ça permet à l'utilisateur d'utiliser son ChatGPT / Ollama / Claude API depuis l'UI desktop tout en consommant aussi des capacités du réseau. Cas d'usage : mode hybride, assistant local + recours réseau.

### 7.4 Note sur MCP

L'architecture spécifie « compat MCP recherchée ». MCP est JSON-RPC 2.0 sur stdio ou SSE. Le format de message N3UR0N est custom. Le bridge réécrit l'enveloppe dans les deux sens. À surveiller : si la complexité du bridge devient importante, reconsidérer l'adoption de JSON-RPC 2.0 comme transport interne.

---

## 8. Frontend partagée

### 8.1 Framework

**SvelteKit avec `adapter-static`.**

Justification : bundle final compact (cible <250 KB gzip pour shell + chat), pas d'hydratation lourde, build pur statique → drop direct dans `rust-embed` ou `tauri.conf.json` `distDir`. Réactivité native sans VDOM, ergonomie devx élevée.

Refus :
- React + Next.js : SSR requiert runtime Node, incompatible avec embed statique.
- React + Vite static : acceptable mais bundle 3-5x plus gros à features égales.
- Solid : alternative défendable, écosystème plus jeune.
- Vue/Nuxt : même problème SSR que Next, ou downgrade vers SPA pure.

### 8.2 Styling et composants

- **Tailwind CSS** — utility-first, purge agressif, contrôle fin de la taille CSS finale.
- **`bits-ui`** — primitives headless accessibles (dialog, popover, combobox, etc.) pour Svelte.
- Pas de design system pré-fait. Identité visuelle propre au projet.

### 8.3 Abstraction transport

La frontend est consommée par deux shells (Tauri et browser via HTTP). Une couche d'abstraction unifie l'invocation de commandes côté UI :

```typescript
// frontend/src/lib/transport.ts
interface Transport {
  invoke<T>(cmd: string, args?: unknown): Promise<T>;
  stream<T>(cmd: string, args?: unknown): AsyncIterable<T>;
}

export const transport: Transport = isTauri()
  ? new TauriTransport()  // tauri::invoke + event::listen
  : new HttpTransport();  // fetch /api/v0 + EventSource SSE

function isTauri(): boolean {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
}
```

Le code des composants ignore le shell. Toute logique métier passe par `transport.invoke(...)`.

### 8.4 Streaming

- **Tauri** : événements typés (`tauri::Manager::emit_to`) écoutés côté webview via `@tauri-apps/api/event`.
- **HTTP** : Server-Sent Events (`text/event-stream`) sur `/api/v0/stream/*`, lus via `EventSource`.

Justification : unidirectionnel suffit (serveur → UI), traverse les proxies, pas de gestion de cycle de vie WebSocket. WebSocket réservé v0.2+ pour besoins push bidirectionnels.

### 8.5 Pages UI v0.1 (communes desktop et server)

Minimum viable :

1. **Setup / Welcome** (premier lancement) — génère clé, configure pair de bootstrap, alias optionnel.
2. **Chat** — prompt utilisateur → invocation locale ou pair sélectionné, streaming réponse.
3. **Pairs** — liste répertoire local, ping, ajout manuel, viewer `describe_self`.
4. **Capabilities** — liste capacités exposées (publisher) ou capacités disponibles sur le réseau (consumer).
5. **Subscriptions** — tokens actifs par pair.
6. **Config** — backend adapter (publisher), endpoint, secrets, mode publish on/off.
7. **Logs** — invocations entrantes/sortantes, erreurs signature.

Visibilité conditionnelle : pages backend/capabilities/logs minimisées en mode consumer pur.

---

## 9. Shell desktop : Tauri 2

### 9.1 Choix

**Tauri 2.x** (stable depuis octobre 2024).

Justification :
- Backend Rust : intégration directe du core comme lib, pas de pont IPC HTTP, latence minime.
- Webview système : taille binaire 8-15 MB vs 100+ MB pour Electron, RAM idle ~50 MB.
- Mobile : Tauri Mobile (iOS / Android) ouvre la porte v1.0 sans changer de pile.
- Updater intégré, signing/notarization documentés, GitHub Actions templates fournis.

Refus :
- **Electron** : taille rédhibitoire pour client gateway léger ; runtime Node redondant.
- **Wails** (Go) : excellent mais nous oblige à un core Go, perd l'écosystème crypto Rust.
- **Neutralino** : trop niche, pas de path mobile.
- **PWA seul** : pas de notifs OS fiables sur desktop, intégration tray/filesystem limitée.

### 9.2 Structure

Le binaire desktop intègre le core Rust comme **dépendance directe**. Pas de processus séparé, pas de loopback HTTP entre UI et core. Les commandes exposées au webview passent par `tauri::command` (sérialisation serde, type-checked).

```rust
#[tauri::command]
async fn invoke_peer(
    state: tauri::State<'_, AppState>,
    peer_id: String,
    capability: String,
    args: serde_json::Value,
) -> Result<serde_json::Value, String> {
    state.core.invoke(&peer_id, &capability, args).await
        .map_err(|e| e.to_string())
}
```

Côté frontend : `import { invoke } from '@tauri-apps/api/core'; invoke('invoke_peer', { ... })`.

### 9.3 Permissions Tauri

Capabilities Tauri 2 minimales (principe du moindre privilège) :

- `core:default` — invoke, event.
- `fs:allow-app-data` — accès limité à `$APP_DATA/n3ur0n/`.
- `dialog:allow-open` — pour import de fichiers de config / clé.
- `notification:default` — pour notifs entrantes (v0.2 mais activée v0.1).
- `shell:allow-open` — pour ouvrir liens externes dans navigateur OS.

Pas de capability réseau arbitraire : les requêtes sortantes passent par le core Rust, pas par `fetch` côté webview vers internet.

### 9.4 Updater

`tauri-plugin-updater` configuré avec endpoint signé Ed25519 (clé du projet). Releases publiées sur GitHub, manifeste JSON pointant vers les binaires par OS/arch.

### 9.5 Listener pair en mode desktop

Désactivé par défaut. Activable via toggle UI « Publish to network », qui :

- Génère un cert auto-signé.
- Demande un port (par défaut 4242).
- Vérifie la disponibilité (UPnP optionnel via `igd-next`, sinon affiche instructions NAT).
- Démarre le listener axum dans le même process Tauri.

Cette opération transforme un consumer en publisher hybride sur la même machine.

---

## 10. Workspace Rust

### 10.1 Layout

```
n3ur0n/
├── Cargo.toml                          # workspace root
├── crates/
│   ├── core/                           # lib : protocole, crypto, types, core engine
│   ├── adapters/                       # lib : MCP, OpenAI, HTTP, Process
│   ├── storage/                        # lib : SQLite, repos, migrations
│   ├── server/                         # bin : axum + rust-embed + cli (publisher)
│   └── desktop/                        # bin : Tauri shell (consumer/hybride)
├── frontend/                           # SvelteKit static (codebase UI unique)
│   ├── package.json
│   ├── svelte.config.js
│   └── build/                          # output, consommé par server et desktop
├── flake.nix | justfile                # orchestration build
└── .github/workflows/                  # CI : build matrix par OS
```

### 10.2 Dépendances inter-crates

- `core` ne dépend que de `serde`, `tokio`, crypto. Pas de HTTP, pas de SQLite.
- `adapters` dépend de `core`.
- `storage` dépend de `core`.
- `server` dépend de tous + `axum`, `rust-embed`.
- `desktop` dépend de tous + `tauri`.

Cette discipline garantit que le core reste testable et léger, et qu'un changement de shell ne touche pas la logique métier.

---

## 11. CLI et outillage

### 11.1 CLI (binaire server uniquement)

**`clap` v4 (derive).**

Commandes minimales v0.1 :

- `n3ur0n init` — génère paire de clés, crée config par défaut, initialise SQLite.
- `n3ur0n serve` — démarre le serveur (UI web + protocole pair + API).
- `n3ur0n peers list|add|remove|ping` — gestion répertoire local.
- `n3ur0n invoke <peer> <capability> [--args ...]` — invocation manuelle.
- `n3ur0n capability list|enable|disable` — gestion capacités exposées.
- `n3ur0n keys show` — affiche identifiant canonique et alias.

Côté desktop : pas de CLI distincte. Toute opération passe par l'UI Tauri ou par appel direct au core via la console de debug intégrée en mode dev.

### 11.2 Logs

**`tracing` + `tracing-subscriber`.**

Format JSON en prod, pretty en dev. Niveaux configurables par module via `RUST_LOG` ou config UI.

### 11.3 Métriques (optionnel v0.1)

`metrics` + exporter Prometheus si `--metrics` activé (server uniquement). Hors scope desktop.

---

## 12. Build et distribution

### 12.1 Pipeline desktop (Tauri)

Build par OS via Tauri CLI :

```
pnpm --filter frontend build
pnpm tauri build --target <triple>
```

Cibles à shipper :

- `x86_64-apple-darwin` → `.dmg` signé + notarisé
- `aarch64-apple-darwin` → `.dmg` signé + notarisé
- `x86_64-pc-windows-msvc` → `.msi` signé Authenticode
- `x86_64-unknown-linux-gnu` → `.AppImage`, `.deb`
- `aarch64-unknown-linux-gnu` → `.AppImage`, `.deb`

Taille cible binaire desktop : ~12-15 MB (webview système, pas Chromium bundlé).

### 12.2 Pipeline server

`cargo build --release --target <triple> -p n3ur0n-server` produit un binaire avec UI web embarquée.

Cibles :

- `x86_64-unknown-linux-musl` (statique)
- `aarch64-unknown-linux-musl`
- `x86_64-apple-darwin`
- `aarch64-apple-darwin`
- `x86_64-pc-windows-msvc`

Taille cible : ~13 MB stripped.

### 12.3 Conteneur

Image Docker `scratch` ou `alpine` pour le server :

```
FROM scratch
COPY n3ur0n-server /n3ur0n-server
EXPOSE 4242
ENTRYPOINT ["/n3ur0n-server", "serve"]
```

Taille cible <20 MB.

### 12.4 Workflow dev

- Backend desktop : `pnpm tauri dev` (hot reload UI + recompile Rust à la sauvegarde).
- Backend server : `cargo run -p n3ur0n-server -- serve` (lecture frontend/build sur disque via rust-embed dev mode).
- Frontend isolée : `pnpm --filter frontend dev` avec proxy Vite vers backend pour `/api`.

### 12.5 CI

GitHub Actions matrix par OS et profil. Étapes communes : lint Rust, lint TS, tests unitaires, build frontend. Étapes spécifiques : Tauri build + signing pour desktop, cross-compile + Docker push pour server.

### 12.6 Pas de Helm, pas de k8s

Out of scope v0.1. Cible publisher = self-host indie, communauté, petite organisation. Compose ou systemd suffit.

---

## 13. Sécurité

### 13.1 Surface d'attaque par profil

**Publisher** :
- Protocole pair (`/n3ur0n/v0`) : vérification de signature en premier middleware. Tout message non vérifié rejeté avant désérialisation profonde.
- API locale (`/api/v0`) : token local, CORS strict, vérif `Host` header pour mitigation DNS rebinding.
- UI (`/ui`) : statique, CSP strict, pas d'eval, pas d'inline.

**Consumer** :
- Pas de listener entrant. Surface réduite aux connexions sortantes.
- IPC Tauri : commandes typées, pas d'exposition `fetch` arbitraire au webview.
- Webview : CSP strict, pas de chargement de ressources externes hors domaine `tauri://localhost`.

### 13.2 Anti-DoS minimal v0.1 (publisher)

- Limite de taille de payload (1 MiB pour `invoke`, 16 KiB pour les méta).
- Timeout par requête (30s par défaut).
- Rate limit par `sender_id` après vérification de signature (sliding window via `governor`).

### 13.3 Anti-replay (publisher)

Nonce stocké en SQLite, fenêtre 1h. Index unique sur `(sender_id, nonce)` rejette automatiquement les rejeux. Cleanup horaire.

### 13.4 Audit

Toute requête entrante et sortante vers le protocole pair journalisée dans `audit_log`. Rotation par taille, par défaut 90 jours.

### 13.5 Signing du binaire

- **Desktop macOS** : Apple Developer ID + notarization. Obligatoire pour Gatekeeper.
- **Desktop Windows** : certificat Authenticode. Obligatoire pour SmartScreen sans warning.
- **Desktop Linux** : signature GPG du `.AppImage` et du `.deb`, hash publié.
- **Server** : signature GPG des release artefacts publiés sur GitHub.

Coût récurrent (Apple ~99 USD/an, EV cert Windows ~300 USD/an) à budgéter avant lancement public.

---

## 14. Tests

### 14.1 Unit

`cargo test` standard. Cible : 80%+ sur `core` (signature, anti-replay, JCS, parsing).

### 14.2 Property-based

`proptest` pour signature/vérification (round-trip), JCS (idempotence), anti-replay (rejet déterministe).

### 14.3 Intégration

Compose avec 3 instances + 1 backend mock. Scénarios :

- Découverte par cascade.
- Invocation libre / restreinte.
- Replay rejeté.
- Cert auto-signé TOFU.
- Consumer (sans listener) → publisher : invocation aller-retour propre.

### 14.4 Fuzz

`cargo-fuzz` sur le parser de message du protocole (cible prioritaire), sur le parser JCS, sur le decoder Base32.

### 14.5 UI

- **Playwright** pour smoke test version web (server) sur les écrans critiques (chat, peers, config).
- **WebDriver via tauri-driver** pour smoke test version desktop sur les mêmes écrans, sur au moins une cible OS.

Pas de coverage exhaustive v0.1.

---

## 15. Dépendances clés (récapitulatif)

| Domaine | Crate / paquet | Version cible |
|---|---|---|
| Async runtime | `tokio` | 1.x |
| HTTP serveur | `axum` | 0.7 |
| HTTP client | `reqwest` (rustls) | 0.12 |
| TLS | `rustls`, `rustls-pemfile` | 0.23+ |
| ACME (opt-in) | `instant-acme` | 0.7 |
| Crypto | `ed25519-dalek` | 2.x |
| Hash | `sha2` | 0.10 |
| Encoding | `data-encoding` | 2.x |
| JSON canonique | `serde_jcs` | 0.1 |
| Sérialisation | `serde`, `serde_json` | 1.x |
| Stockage | `rusqlite` (bundled) | 0.31 |
| Embed assets (server) | `rust-embed` | 8.x |
| Shell desktop | `tauri` | 2.x |
| Updater | `tauri-plugin-updater` | 2.x |
| CLI | `clap` (derive) | 4.x |
| Logs | `tracing`, `tracing-subscriber` | 0.1 / 0.3 |
| Rate limit | `governor` | 0.6 |
| NAT (optionnel) | `igd-next` | dernière stable |
| Frontend | SvelteKit + adapter-static | dernière stable |
| CSS | Tailwind CSS | 3.x |
| Composants | bits-ui | dernière stable |

---

## 16. Limites et reports

Ce stack assume les compromis listés dans `n3ur0n-architecture-v0.md` §10 et y ajoute les siens propres :

- **Pas de mises à jour automatiques v0.1 côté server.** Auto-update intégré côté desktop via `tauri-plugin-updater`. Côté server, l'opérateur upgrade manuellement (Docker pull, binaire replace, systemd restart). Auto-update server v0.2.
- **Pas de packaging Linux distro v0.1** au-delà de `.AppImage` et `.deb`. Ni `.rpm`, ni paquets Arch, ni Flatpak. À industrialiser v0.2.
- **Pas de version mobile v0.1.** Tauri Mobile reste expérimental selon les plateformes ; cible v1.0.
- **Multi-utilisateur local (0.4.0+).** RBAC phase 1 : comptes locaux + rôles sur l'API `/api/v0` et l'UI embarquée. Pas encore de fédération d'identité utilisateur sur le fil pair ; le multi-utilisateur *réseau* reste inter-instances.
- **Listener publisher en mode desktop nécessite intervention manuelle sur le NAT.** UPnP via `igd-next` proposé mais non garanti. Pas de relais TURN/WebRTC v0.1. Publisher sérieux = VPS ou homelab avec port forward.
- **Coûts de signing (Apple, Windows) non couverts par budget v0.1.** Premiers releases peuvent être non signés pour early adopters acceptant les warnings, mais signature obligatoire avant lancement grand public.

---

## 17. Méta

Ce document complète l'architecture sans la dupliquer. Si une décision technique entre en conflit avec `n3ur0n-architecture-v0.md`, l'architecture prime ; le présent document est mis à jour pour refléter le compromis.

Toute modification d'un choix de stack majeur (langage, framework HTTP, moteur de stockage, framework frontend, shell desktop) doit faire l'objet d'une note datée en tête du document, avec un changelog explicite.

---

*Fin du draft 1.*
