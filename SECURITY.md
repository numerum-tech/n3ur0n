# Security Policy

## Supported versions

N3UR0N is pre-1.0. Only the latest tagged release is supported with security fixes.

| Version | Supported |
|---------|-----------|
| latest  | ✅        |
| older   | ❌        |

## Reporting a vulnerability

**Do not file a public issue.** Use GitHub's private vulnerability reporting:

1. Go to https://github.com/n3ur0n/n3ur0n/security/advisories/new
2. Describe the issue with reproduction steps and impact.
3. Suggest a fix if you have one.

If GitHub advisories are unavailable, email the maintainers (see commit history `git log --format='%ae' | sort -u`).

We aim to acknowledge reports within 72 hours and ship a fix within 30 days for high-severity issues.

## Scope

In scope:

- Protocol bypass: forged signatures, replay attacks, identity collision, JCS canonicalization divergence.
- Memory safety in the Rust core (we forbid `unsafe`).
- Capability injection: a peer causing another peer to invoke an unintended capability.
- Sandbox escape from the desktop shell.
- Credential disclosure from `backends/*.toml` over the wire.

Out of scope:

- Denial of service via resource exhaustion (we don't rate-limit at the protocol layer in v0.x).
- Issues that require physical access to a victim's machine.
- Vulnerabilities in dependencies — please report those upstream first.

## Disclosure

We follow coordinated disclosure. Once a fix ships, we publish a GitHub Security Advisory with credit (unless you ask to remain anonymous).
