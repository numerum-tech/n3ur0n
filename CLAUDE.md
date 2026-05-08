# OpenWolf

@.wolf/OPENWOLF.md

This project uses OpenWolf for context management. Read and follow .wolf/OPENWOLF.md every session. Check .wolf/cerebrum.md before generating code. Check .wolf/anatomy.md before reading files.


# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## État du dépôt (8 mai 2026)

Phase pré-implémentation. **Aucun code à ce jour** — uniquement deux documents de spec :

- [n3ur0n-architecture-v0.md](n3ur0n-architecture-v0.md) — architecture v0.1 (vision, vocabulaire, couches, identité, protocole, limites assumées, questions ouvertes).
- [project-tech-stack.md](project-tech-stack.md) — stack technique v0.1 (Rust workspace, Tauri 2, SvelteKit, axum, SQLite, profils consumer/publisher).

**Règle de précédence** : si un détail de stack contredit l'architecture, l'architecture prime ; le doc de stack est mis à jour pour refléter le compromis (cf. `project-tech-stack.md` §17).

Avant tout travail d'implémentation, **lire les deux docs en entier** — ils sont opiniâtres, les choix sont fermes (pas des suggestions). Les sections "Limites assumées" (§10 archi) et "Limites et reports" (§16 stack) listent les compromis explicites de v0.1 ; ne pas réintroduire de fonctionnalité sans note datée.

## Ce que N3UR0N est (et n'est pas)

Système distribué pair-à-pair pour publier et invoquer des **capacités d'IA** sans autorité centrale. L'unité déployée est l'**instance n3ur0n** : passerelle (gateway) sans intelligence propre, qui encapsule un backend IA (LLM local, API, MCP server, script…) et l'expose au réseau.

**N'est pas** un cerveau, ni un protocole d'inférence, ni un nouveau format de tools. Le différenciateur est l'effet réseau pair-à-pair entre instances + UX cartographique sémantique (UX cartographique reportée post-v0.1).

### Discipline lexicale (impose-la dans le code et les commentaires)

- "neurone" / "n3ur0n" = **instance gateway**, jamais une IA.
- "backend" / "capacité" = l'intelligence elle-même.
- Vocabulaire métier : *atome, dendrite, soma, axone, synapse, lobe, faisceau, glie* — voir archi §3. Sont des **registres de représentation**, pas des prétentions biologiques.

## Architecture en couches (résumé)

1. **Backend IA** — externe au protocole, branché via adaptateur.
2. **Instance n3ur0n** — gateway : routage, identité crypto, signature, politique de souscription, répertoire local.
3. **Identité & autorisation** — Ed25519 par message (non négociable) + souscription optionnelle au choix du destinataire.
4. **Lobe** — fédération nommée d'instances. v0.1 : seuls les lobes **communautaires** sont supportés.
5. **Surface utilisateur** — CLI / API REST + UI Svelte (desktop Tauri ou web servie par le binaire serveur).

## Invariants protocolaires non négociables

- Identifiant canonique = `n3:` + Base32(SHA-256(clé publique Ed25519)). Auto-vérifiable, pas de registre requis.
- Tout message porte `sender_id, recipient_id, timestamp, nonce, payload, signature`. La signature couvre le **JCS** (RFC 8785, `serde_jcs`) de la concaténation des cinq premiers champs. **Sans canonicalisation, les signatures divergent silencieusement** — ne jamais sérialiser à la main pour signer.
- Vérifications obligatoires côté destinataire : signature, `recipient_id`, fenêtre timestamp ±5 min, anti-replay nonce sur 1h.
- Trois verbes méta (`describe_self`, `get_known_peers`, `ping`) sont **toujours en mode libre**. Ne jamais restreindre.
- Quatrième verbe : `invoke`. **Aucun autre verbe v0.1**. Pas de session, pas de streaming protocolaire, pas de pipeline orchestré côté protocole.
- Mode d'accès (`free` / `restricted`) déclaré **par capacité**, pas par instance.

## Asymétrie consumer / publisher (structurante pour le stack)

- **Consumer** = client final. Aucun listener public. Shell desktop Tauri 2. Pas de cert TLS, pas de NAT à traverser.
- **Publisher** = opérateur exposant des capacités. Listener `/n3ur0n/v0` HTTPS obligatoire. Headless typiquement (VPS, homelab).
- Même core Rust pour les deux modes ; ils diffèrent par le **shell** (Tauri vs axum + rust-embed) et la **config par défaut** du listener.
- Un consumer peut basculer en publisher hybride via toggle UI ("Publish to network") qui démarre le listener axum dans le même process Tauri.

## Layout cible (planifié, à créer au fur et à mesure)

```
n3ur0n/
├── Cargo.toml                 # workspace
├── crates/
│   ├── core/                  # lib : protocole, crypto, types — AUCUNE dép HTTP/SQL
│   ├── adapters/              # lib : MCP, OpenAI, HTTP, Process — dépend de core
│   ├── storage/               # lib : SQLite (rusqlite bundled) — dépend de core
│   ├── server/                # bin : axum + rust-embed + clap (publisher)
│   └── desktop/               # bin : Tauri 2 shell (consumer/hybride)
├── frontend/                  # SvelteKit static — UNIQUE codebase UI
└── .github/workflows/
```

**Discipline de dépendances** : `core` ne touche ni HTTP ni SQLite. Tout changement de shell ne doit pas toucher la logique métier. Si tu te retrouves à importer `axum` ou `rusqlite` depuis `core`, tu te trompes de crate.

## Choix de stack à respecter

| Domaine | Choix | Refus explicites (ne pas réintroduire sans discussion) |
|---|---|---|
| Langage core | Rust stable edition 2024 | Node, Python, Go |
| Async | `tokio` (full) | `async-std`, `smol` |
| HTTP serveur | `axum` 0.7+ | `actix`, `warp`, `rocket` |
| HTTP client | `reqwest` rustls | OpenSSL backend |
| TLS | `rustls` | OpenSSL |
| Crypto | `ed25519-dalek` 2.x, `sha2` | autres impls Ed25519 |
| JSON canonique | `serde_jcs` | sérialisation maison |
| Stockage | `rusqlite` bundled | Postgres, MySQL, Redis (services externes interdits v0.1) |
| Frontend | SvelteKit + `adapter-static` + Tailwind + bits-ui | React/Next, Vue/Nuxt (SSR incompatible avec embed statique) |
| Shell desktop | Tauri 2.x | Electron, Wails, Neutralino |
| CLI | `clap` v4 derive | structopt, argh |
| Logs | `tracing` + `tracing-subscriber` | `log` direct, `slog` |

## Commandes (planifiées — à valider une fois le scaffold créé)

Ces commandes sont issues du doc de stack §12. À l'heure actuelle aucune n'est exécutable car le code n'existe pas. Quand le scaffold sera posé, les valider et corriger ce fichier.

```bash
# Workspace Rust
cargo build --release -p n3ur0n-server
cargo test                                   # cible 80%+ sur core
cargo run -p n3ur0n-server -- serve

# Frontend (codebase UI unique)
pnpm --filter frontend build                 # output dans frontend/build/
pnpm --filter frontend dev                   # proxy Vite vers /api du serveur

# Desktop Tauri
pnpm tauri dev                               # hot reload UI + recompile Rust
pnpm tauri build --target <triple>           # .dmg / .msi / .AppImage / .deb

# CLI publisher
n3ur0n init                                  # paire de clés + config + SQLite
n3ur0n serve
n3ur0n peers list|add|remove|ping
n3ur0n invoke <peer> <capability> [--args ...]
```

## Test plan minimal (cible v0.1)

- Unit `cargo test` — focus `core` (signature, anti-replay, JCS, parsing).
- `proptest` — round-trip signature/vérif, idempotence JCS, déterminisme anti-replay.
- Intégration : compose 3 instances + 1 backend mock. Scénarios obligatoires :
  - Découverte par cascade profondeur 1.
  - Invocation libre / restreinte.
  - Replay rejeté.
  - Cert auto-signé TOFU.
  - Consumer (sans listener) → publisher : aller-retour propre.
- `cargo-fuzz` sur le parser de message, parser JCS, decoder Base32.
- Playwright (web) + tauri-driver (desktop) — smoke test sur chat / peers / config.

## Questions ouvertes bloquantes (archi §11)

À trancher **avant lancement public**, pas avant le code :

- Granularité de la synapse (1:1 vs 1:lobe vs 1:capability).
- Mécanisme anti-free-riding pour lobes communautaires.
- Position juridique sur les noms de marques (`@google`, `@adobe`).
- Localisation du planner pour pipelines multi-étapes.
- Modèle économique du registre par défaut.

Si une décision implémentation force la main sur l'une de ces questions, **ne pas trancher silencieusement** — remonter à l'utilisateur.

## Conventions de travail spécifiques au projet

- Tout choix de stack ou archi qui dévie des docs nécessite **note datée en tête du doc concerné** + changelog explicite. Pas de dérive silencieuse.
- Sections "décisions" (archi §3-9) vs sections "dette" (archi §10-11, stack §16) : ne pas mélanger. Glisser une dette dans une décision = mensonge à soi-même ; glisser une décision floue dans une dette = procrastination.
- Documents en français. Code, identifiants, commits : anglais standard.
