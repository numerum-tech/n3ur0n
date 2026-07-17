# Design: n3uron.com one-pager (`www/`)

**Date:** 2026-07-15  
**Status:** approved for implementation  
**Role:** Canonical informative site for **n3ur0n.net** (project page). Seed peer (e.g. `seed.n3ur0n.net`) is separate.

## Decision

- Approach **A**: `www/` is the public narrative site. `docs/` remains GH Pages / downloads plumbing for now.
- Static only: `index.html` + `styles.css` + brand assets. No framework / build.
- EN first. Honest about no installers yet.

## Page sections

1. Hero — brand + one headline + one line + CTAs (Get started, GitHub)
2. What it is / isn’t
3. How it works (3 steps)
4. Two profiles (desktop / publisher)
5. Get started (commands from README)
6. Network — soft seed placeholder
7. Footer — license, links

## Visual

Product-adjacent: ink/charcoal, teal + amber brand mark, transparent hex logo, atmospheric background, expressive type + mono for commands. No purple-on-white, no cream-serif brochure look.

## Out of scope

i18n switcher, live seed status, Auth, Rust embed, retiring `docs/`.
