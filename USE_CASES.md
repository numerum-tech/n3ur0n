# N3UR0N use cases (samples)

Illustrative scenarios — not promises. They show where a **peer-to-peer network for AI skills** does something a hosted chatbot or a single API gateway can't.

The common thread: the thing you share and call is a **skill running on someone's own machine**. Every call is signed, so you always know who published a skill and who's calling it. Callers keep their own identity; publishers keep their own models and data.

A few words used throughout:

- **Skill (capability)** — one unit of work a node offers, described in a small text file: a translation prompt, a document summarizer, a tool wrapped for the network.
- **Publisher** — a node that exposes skills to the network.
- **Consumer** — a node you talk to; it plans and calls skills on your behalf (a desktop app or your own small server).
- **Seed** — a public starting point a fresh node can connect to, to discover its first peers.

---

## 1. Public — a shared library of community skills, with provenance

### The situation

Communities (open source, civic, language) trade AI "skills" as prompts and scripts on Discord, Gists, and forums. Nobody can verify who wrote them, they go stale, and everyone re-wires the same model and API plumbing to use them.

### How N3UR0N helps

Libraries, non-profits, universities, and hobbyists run a node and publish skills — for example:

- a French→English translator over a local or sponsored model
- a "summarize this legislation" skill: document in, short brief out
- existing tools (file search, lookups) wrapped so the network can call them

Anyone can **download N3UR0N** (or build from source), connect to a seed they trust, see what skills the network offers, and call them — with signed messages. The skill stays on the publisher's machine; you never hand your keys to a central service, and every skill carries a verifiable "who published this."

### Why not just ChatGPT or a forum post?

| What you need | With N3UR0N |
|---|---|
| Know who actually offered a skill | A self-verifying ID + a signed call — no impersonation |
| Know where the code and data live | On the publisher's own machine; skills are just files |
| Swap the underlying model without breaking callers | The model sits behind the gateway |
| Get going without waiting for a marketplace | Peers introduce each other; a public seed is optional |

### First step

1. Run a public **seed** offering a free `echo` skill (proves the path works).
2. Tell builders which seed to connect to.
3. Community publishers add real skills over time.

---

## 2. Company — one fabric, many teams

### The situation

Companies tend to funnel all AI through one shared gateway or a single ops-owned "agent platform." Team tools, models, and compliance lines get blurred. Security wants an audit trail; teams want independence; legal cares which vendor saw which data.

### How N3UR0N helps

Each team (or environment) runs its own node and publishes its own skills:

| Team | Example skill | Who can use it |
|---|---|---|
| Legal | contract redaction | restricted / private |
| Data | safe warehouse queries | restricted |
| SRE | runbook diagnosis | restricted |
| ML | experimental chat skills | private (never advertised) |

Employees use a **consumer** (the desktop app or an internal server). It connects to an **internal seed**, not the open internet. When someone makes a request, the built-in planner breaks it into steps and calls the right team's skill for each — every call signed and attributable. Sensitive skills can be **private**: usable, but never listed to the rest of the network.

### Why not one corporate ChatGPT?

| What you need | With N3UR0N |
|---|---|
| Each team keeps its own model keys / GPUs | Each team is its own publisher |
| Audit who called what | Every call is signed and tied to an ID |
| One path for both models and tools | A skill can be a prompt, a tool, or an API |
| Sensitive skills invisible to everyone else | Mark them private |

### First step

1. Stand up an internal seed with a health/`echo` skill.
2. Add one real restricted skill from a willing team.
3. Point people at the desktop app or an internal server — no public interface on the publishers.

---

## 3. Hybrid — a vendor's edge to its partners

### The situation

A vendor wants partners to use its specialized AI (defect classification, config generation, industry-specific search) **without** shipping the model or building a full multi-tenant portal for every product line. Today that means API keys, rate portals, and brittle integrations.

### How N3UR0N helps

The vendor runs a reachable node offering:

- free demo skills for evaluation
- restricted production skills for contracted partners

Partners run their own node to consume those skills — and can optionally publish their **own** site-local skills (plant-floor adapters, local search over their data) that never leave their premises. The planner can combine **the vendor's intelligence** with **the partner's local actions**, and — if the data is shaped for it — the vendor never sees the partner's on-site data.

Contracts, rate limits, and billing stay off the wire and out of band; the network only carries signed calls (and an opaque access token if the operator requires one).

### Why not a partner API + SDK?

| What you need | With N3UR0N |
|---|---|
| Extend without shipping a new SDK | Skills are just files |
| Partner keeps regional / air-gapped models | On the partner's own nodes |
| One identity model for vendor and partner nodes | The same signed-ID scheme everywhere |
| Discovery later, without a hard-coded registry | Peers introduce each other |

### First step

1. Vendor runs a node with a free demo skill.
2. Contracted partner connects and gets access to a restricted skill (keyed through the vendor's normal onboarding).
3. Optionally, the partner publishes local-only skills on their own node.

---

## At a glance

| # | Context | Seed | Success looks like |
|---|---|---|---|
| 1 | Public commons | Optional public seed | Strangers call a third party's skill and can verify who published it |
| 2 | Company fabric | Internal only | Two teams' skills combine into one answer, no shared AI bus |
| 3 | Vendor ↔ partner | Vendor's node | A partner ships a product using the vendor's skills plus their own, no proprietary agent cloud |

---

## What N3UR0N is not

- A multi-user web chat in front of one local model — handy for a demo, but not the point.
- A model marketplace baked into the network — deliberately out of scope (see [ROADMAP.md](ROADMAP.md)).
- A promise of free, unlimited AI for the internet — cost and abuse are the operator's to manage.

---

## Related docs

- [README.md](README.md) — project overview
- [n3ur0n-architecture-v0.md](n3ur0n-architecture-v0.md) — how it works under the hood
- [ROADMAP.md](ROADMAP.md) — what's planned
