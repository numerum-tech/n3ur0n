# Direct Chat Mode Implementation Plan

> **For agentic workers:** REQUIRED: Use `superpowers:subagent-driven-development` or `superpowers:executing-plans` to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement direct chat mode allowing single-LLM-call conversations without plan compilation, with model override, mode toggle, and full UI support.

**Architecture:** Add `DirectChatPlanner` trait impl alongside existing `PlanExecPlanner`. Extend `Planner` trait with `DispatchOptions` for model override. Toggle mode per-message at API layer. UI stores mode/model in localStorage. No protocol/SQL changes. Planner-backed only (503 without planner).

**Tech Stack:** Rust (crates/node, crates/adapters, crates/server), trait dispatch, SSE events, JavaScript (localStorage), i18n JSON catalogs.

---

## File Structure

**Create:**
- `crates/node/src/planner/direct.rs` — DirectChatPlanner impl (~140 lines)

**Modify:**
- `crates/node/src/planner/mod.rs` — Add DispatchOptions, DispatchMode enums; route dispatch calls; expose MAX_CONTEXT_TURNS constant (~50 lines)
- `crates/node/src/runtime.rs` — Store direct planner, add mode/opts params to handle_user_message{,_streaming} (~40 lines)
- `crates/node/src/conversation.rs` — Move MAX_CONTEXT_TURNS export (1 line)
- `crates/adapters/src/openai.rs` — Add allow_model_override flag to OpenAIConfig, use in build_request (~25 lines)
- `crates/server/src/bootstrap.rs` — Construct DirectChatPlanner, set allow_model_override=true for planner backend (~15 lines)
- `crates/server/src/http.rs` — Extend ConversationMessageRequest, parse mode/model, pass to dispatch (~35 lines)
- `crates/server/ui/index.html` — Add mode toggle, model input, update stepper (~20 lines)
- `crates/server/ui/app.js` — Handle toggle/input UI, localStorage persistence, send mode/model in request (~80 lines)
- `crates/server/ui/style.css` — Style toggle/input elements (~15 lines)
- `crates/server/ui/locales/en.json` — Add 6-8 new i18n keys (~40 lines total in both files)
- `crates/server/ui/locales/fr.json` — Translated strings (~40 lines)
- `docker/cluster-smoke.sh` — Add direct-mode test case (~10 lines)

**Test files (new):**
- `crates/node/src/planner/direct.rs` — unit tests (inline in file, ~80 lines)
- `crates/adapters/tests/openai_backend.rs` — allow_model_override tests (~30 lines)
- `crates/server/tests/` — integration test new file or extend existing (~50 lines)

---

## Chunk 1: Planner Trait & DirectChatPlanner Core

### Task 1: Extend Planner trait with DispatchOptions

**Files:**
- Modify: `crates/node/src/planner/mod.rs:1-50`
- Test: inline assertions

- [ ] Define DispatchOptions and DispatchMode enums
- [ ] Update Planner trait signature
- [ ] Move MAX_CONTEXT_TURNS constant
- [ ] Update PlanExecPlanner impl to accept new params
- [ ] Compile and check no errors
- [ ] Commit

---

### Task 2: Create DirectChatPlanner

**Files:**
- Create: `crates/node/src/planner/direct.rs` (~140 lines + 80 lines tests)
- Modify: `crates/node/src/planner/mod.rs` (add mod declaration + pub use)

- [ ] Write failing test for DirectChatPlanner dispatch
- [ ] Implement DirectChatPlanner struct
- [ ] Implement dispatch method
- [ ] Add module declaration
- [ ] Run tests (expected to fail — MockBackend not yet defined)
- [ ] Create minimal MockBackend for testing
- [ ] Run tests again
- [ ] Commit

---

## Chunk 2: OpenAI Backend Model Override & Runtime Integration

### Task 3: Add model override flag to OpenAIConfig

**Files:**
- Modify: `crates/adapters/src/openai.rs:1-100`

- [ ] Add allow_model_override field to OpenAIConfig
- [ ] Update build_request to respect flag
- [ ] Add tests for allow_model_override behavior
- [ ] Compile and test
- [ ] Commit

---

### Task 4: Update bootstrap to set allow_model_override for planner backend

**Files:**
- Modify: `crates/server/src/bootstrap.rs:200-250`

- [ ] Locate backend construction in build_runtime
- [ ] Set allow_model_override=true only for planner backend
- [ ] Verify network-facing backends keep allow_model_override=false
- [ ] Compile
- [ ] Commit

---

### Task 5: Integrate DirectChatPlanner in NodeRuntime

**Files:**
- Modify: `crates/node/src/runtime.rs:50-150`

- [ ] Add direct planner field to NodeRuntime
- [ ] Update runtime constructor to build DirectChatPlanner
- [ ] Update handle_user_message signature
- [ ] Route based on mode
- [ ] Do same for handle_user_message_streaming
- [ ] Compile
- [ ] Commit

---

## Chunk 3: HTTP API Changes

### Task 6: Extend ConversationMessageRequest and API handlers

**Files:**
- Modify: `crates/server/src/http.rs:500-600`

- [ ] Define request schema
- [ ] Update api_conversations_messages POST handler
- [ ] Update streaming handler similarly
- [ ] Add integration test for request parsing
- [ ] Compile and test
- [ ] Commit

---

## Chunk 4: Frontend UI and i18n

### Task 7: Update HTML and CSS for mode toggle and model input

**Files:**
- Modify: `crates/server/ui/index.html`
- Modify: `crates/server/ui/style.css`

- [ ] Add mode toggle and model input to composer
- [ ] Add stepper status for direct mode
- [ ] Style toggle and input
- [ ] Verify HTML/CSS compiles (no syntax errors)
- [ ] Commit

---

### Task 8: Implement toggle and model handling in app.js

**Files:**
- Modify: `crates/server/ui/app.js:1-100` (init), update send logic

- [ ] Initialize localStorage keys and read saved values
- [ ] Call init when conversation is opened
- [ ] Update send message handler to include mode and model
- [ ] Update stepper status for direct mode
- [ ] Test toggle visibility on mode change
- [ ] Commit

---

### Task 9: Add i18n strings for direct mode

**Files:**
- Modify: `crates/server/ui/locales/en.json`
- Modify: `crates/server/ui/locales/fr.json`

- [ ] Add English strings
- [ ] Add French strings
- [ ] Verify JSON syntax
- [ ] Commit

---

## Chunk 5: Integration and Testing

### Task 10: Add direct mode test to docker smoke script

**Files:**
- Modify: `docker/cluster-smoke.sh`

- [ ] Find the section after invoke chat tests
- [ ] Add direct mode conversation test
- [ ] Test invalid mode parameter
- [ ] Run docker smoke script
- [ ] Commit

---

### Task 11: Add unit tests for DirectChatPlanner with edge cases

**Files:**
- Modify: `crates/node/src/planner/direct.rs` (test section)

- [ ] Add test for empty model override (uses default)
- [ ] Add test for error propagation
- [ ] Add test for streaming event sequence
- [ ] Run tests
- [ ] Commit

---

### Task 12: Integration test for API mode parsing and routing

**Files:**
- Create or Modify: `crates/server/tests/direct_mode.rs`

- [ ] Write integration test file
- [ ] Implement build_test_app helper (or reuse existing)
- [ ] Run integration tests
- [ ] Commit

---

## Chunk 6: Final Integration and Verification

### Task 13: Run full test suite and verify no regressions

- [ ] Run all unit tests
- [ ] Run integration tests
- [ ] Run docker smoke test
- [ ] Manual test: toggle mode in UI
- [ ] Verify localStorage persistence
- [ ] Commit test verification

---

### Task 14: Documentation update

**Files:**
- Modify: `CHANGELOG.md`

- [ ] Add entry to CHANGELOG
- [ ] Update ROADMAP if needed
- [ ] Commit docs

---

### Task 15: Create PR summary

- [ ] Review all commits
- [ ] Squash if desired or keep as-is
- [ ] Final compile and test
- [ ] Summary for PR
- [ ] Final commit
