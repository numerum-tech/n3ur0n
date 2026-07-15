# N3UR0N use cases (samples)

Illustrative scenarios — not product commitments. They show where a **peer protocol for signed capability invoke** is doing work that a hosted chatbot or a single API gateway does not.

Common thread: the unit of trust and commerce is a **capability published on someone’s node**, invoked with Ed25519-signed envelopes. Callers keep their own identity; publishers keep their own backends.

---

## 1. Public — community skill commons (with provenance)

### Situation

Open-source, civic, and linguistic communities share AI “skills” as prompts and scripts on Discord, Gists, and forums. Nobody can verify who published them, they rot, and every consumer re-glues HTTP clients and model wrappers.

### How N3UR0N fits

Operators (libraries, NGOs, universities, hobbyists) run **publishers** and advertise TOML capabilities, for example:

- `translate-xx-yy` — prompt binding over a local or sponsored model  
- `summarize-legislation` — document in → short brief out  
- MCP tools (filesystem, search) wrapped as caps  

Consumers **build from source**, bootstrap a known seed (or peer they trust), discover caps via `describe_self` / cascade, and `invoke` with signed messages. The skill stays on the publisher’s machine; the caller never deposits keys in a central SaaS.

### Why not “just ChatGPT / a forum”?

| Need | N3UR0N |
|------|--------|
| Who offered this skill? | Cryptographic `n3:` identity + signed invoke |
| Where does code/data live? | Publisher’s node; caps are data (`caps/*.toml`) |
| Swap model without rewriting clients | Backend is behind the gateway |
| Start without a marketplace | Gossip + optional bootstrap seed |

### First mile

1. Run a public **seed** with free `echo` (proof of path).  
2. Document `--bootstrap https://seed…` for builders.  
3. Community publishers add real free/restricted caps over time.

---

## 2. Company — multi-team capability fabric

### Situation

Enterprises collapse AI into one shared gateway or one ops-owned “agent platform.” Team tools, models, and compliance boundaries get mixed. Security wants audit trails; teams want independence; legal cares which vendor saw which data.

### How N3UR0N fits

Each team or environment runs one or more **publishers**:

| Team | Example caps | Access |
|------|----------------|--------|
| Legal | `contract-redact` | restricted / private |
| Data | `query-warehouse-safe` | restricted |
| SRE | `runbook-diagnose` | restricted |
| ML | experimental chat caps | private (never in `describe_self`) |

Employees run a **consumer** (desktop or internal server). Bootstrap points at a **corp seed**, not the open internet. The local **PlanExec** planner compiles multi-step plans; each step is a signed `invoke` to the owning peer. UI RBAC covers who may configure backends locally; commercial/subscription checks stay out-of-band (IdP, tickets).

### Why not “one corporate ChatGPT”?

| Need | N3UR0N |
|------|--------|
| Team owns model keys / GPU | Publisher boundary |
| Audit who called what | Signed envelopes + peer ids |
| Same path for LLM and tools | `invoke` + bindings (`prompt` / `mcp` / `http`) |
| Sensitive skills invisible to the mesh | `AccessMode::Private` |

### First mile

1. Internal seed + `echo` / health caps.  
2. One real restricted cap from a willing team.  
3. Desktop or loopback server for planners; no public UI on publishers.

---

## 3. Hybrid — vendor / partner capability edge

### Situation

A vendor or OEM wants partners to use specialized AI (defect classification, config generation, vertical RAG) without shipping the model or building a full multi-tenant SaaS portal for every SKU. Today: API keys, rate portals, brittle OpenAPI.

### How N3UR0N fits

The vendor runs a reachable publisher (public internet or partner VPN):

- Free demo caps for evaluation  
- Restricted production caps for contracted partners  

Partners run their own neuron as **consumer**, and optionally as **publisher** of site-local caps (plant adapters, local RAG) that never leave the site. The planner can stitch **vendor cognition** + **local actuators** without the vendor seeing OT data, if payloads are designed that way.

Bootstrap = vendor seed. Commercial onboarding (contracts, rate limits, billing) stays outside the wire; the protocol only carries signed invoke and opaque subscription tokens if the operator requires them.

### Why not “partner API + SDK”?

| Need | N3UR0N |
|------|--------|
| Extensibility without a new SDK | Caps are manifests |
| Partner keeps regional / air-gapped backends | Their publishers |
| Same identity model for vendor and partner nodes | Ed25519 + `n3:` |
| Future discovery without a baked-in registry | Gossip; optional “registry as capability” later |

### First mile

1. Vendor seed with free `echo` + one demo cap.  
2. Contracted partner: bootstrap + restricted cap keying via out-of-band process.  
3. Optional on-site partner publisher for local-only caps.

---

## Mapping

| # | Context | Seed | Success looks like |
|---|---------|------|--------------------|
| 1 | Public commons | Optional public seed | Strangers invoke a third-party skill with verifiable publisher identity |
| 2 | Company mesh | Internal only | Two teams compose a plan across peers without a shared AI bus |
| 3 | Vendor ↔ partner | Vendor endpoint | Partner ships product using vendor caps + local caps without a proprietary agent cloud |

---

## What these are not

- A multi-user web chat over a single Ollama host (useful for demos; not the differentiator).  
- A central model marketplace baked into the protocol (explicitly out of scope; see [ROADMAP.md](ROADMAP.md)).  
- Guaranteed free unlimited LLM for the internet (abuse and cost are operator problems).

---

## Related docs

- [README.md](README.md) — project overview  
- [n3ur0n-architecture-v0.md](n3ur0n-architecture-v0.md) — protocol  
- [ROADMAP.md](ROADMAP.md) — milestones and registry-as-capability reflection  
