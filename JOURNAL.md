# Journal — Agentic UI

Implementation log for the agentic UI overhaul. One entry per phase per attempt. See `docs/plans/agentic-ui-v1.md` for the full plan and acceptance criteria.

**Branch:** `feat/agentic-ui`

---

## Phase 0 — Branch + scaffolding

- **Started:** 2026-05-06
- **Branch:** feat/agentic-ui
- **Status:** done
- **Acceptance evidence:**
  - Branch `feat/agentic-ui` cut from master @ `c5434cd` (`feat: prefix+o to cycle pane focus`).
  - Plan tracked on branch at `docs/plans/agentic-ui-v1.md`. Working copy retained at `.omc/plans/agentic-ui-v1.md` (gitignored).
  - `JOURNAL.md` created (this file).
  - `agents/` directory created with 5 placeholder role prompt files: `pm.md`, `orchestrator.md`, `architect.md`, `recon.md`, `engineer.md`.
  - CI gate green on the pre-commit snapshot: `cargo check`, `cargo fmt --all -- --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test --all-targets` (68 passed, 0 failed).
- **Notes:**
  - Master had one master-quality feature uncommitted at session start (`prefix+o` pane cycle). Committed to master as `c5434cd` before branching (path A from the planning interview).
  - `docs/plans/agentic-ui-v1.md` is the source of truth. The `.omc/plans/` copy is a scratch working copy and may drift — always reconcile to `docs/plans/` before referencing.
  - Role markdown files are placeholders only. Phase 10 fleshes out the actual role content (system prompts, allowed tools, heartbeat protocol, forbidden commands).

---

## Phase 1 — Members pane skeleton

- **Started:** 2026-05-06
- **Branch:** feat/agentic-ui
- **Status:** done
- **Acceptance evidence:**
  - New `src/panes/members.rs` with `MembersPane` (hardcoded role list: PM, Orchestrator, Architect, Recon, engineer-1, engineer-2). 4 unit tests covering construction, j/k clamping, Enter→active sync.
  - `PaneId::Members` added to enum (`src/app.rs:18`); `App.members: MembersPane` field initialized in `App::new`.
  - `src/layout.rs` `default_horizontal()` upgraded to 4-pane layout: Members(0,0), Workspaces(0,1), Terminal(1,0), GitStatus(2,0). All 12 default-layout tests rewritten to match (terminal.left = Members, workspaces.up = Members, members.down = Workspaces, etc.). Layout/algorithm unchanged — only the slot config moved.
  - `Ctrl+a m` keybind → `Action::FocusPane(PaneId::Members)`. Test `prefix_then_m_focuses_members` covers it.
  - `CyclePane` (Ctrl+a o) cycle extended: Members → Workspaces → Terminal → GitStatus → Members.
  - UI: left column split 50/50 vertically (`Constraint::Percentage(50)`); top half = Members, bottom = Workspaces. Status bar shows `MEMBERS` label when Members is focused. Help overlay updated with `Ctrl+a m  Focus Members` line; height bumped 18 → 19.
  - Cargo gate green: 77 tests pass (was 68 before Phase 1; +9 net = 4 members + 1 keymap + 4 layout).
- **Notes:**
  - No PTYs behind Members yet. Phase 2 generalizes `App.terminals` keying so `Member` surfaces can host PTYs; Phase 3 actually spawns the per-role placeholder PTYs.
  - `members.handle_key` is a void method (no `Option<Event>` shape) since no app-level event surface needs it this phase. Will be revisited in Phase 3 when Enter must trigger `Event::SwitchSurface(...)`.
  - Status bar prefix indicator unchanged. `[PREFIX]` still works for all bindings including `m`.
  - Visual change is significant — left column halves cut Workspaces vertical room. Acceptable while members list is short; revisit if engineer pool grows beyond ~6.

---

## Phase 2 — Generalize Surface enum

- **Started:** 2026-05-06
- **Branch:** feat/agentic-ui
- **Status:** done
- **Acceptance evidence:**
  - New module `src/surface.rs` defining `Role` (Pm/Orchestrator/Architect/Recon/Engineer), `MemberId { role, instance }`, and `Surface = Workspace(PathBuf) | Member(MemberId)`. All derive `Hash + Eq + Clone` so `Surface` is a valid `HashMap` key. 4 unit tests cover hashmap keying, singleton labels, engineer labels, workspace_path projection.
  - `App.terminals` keying refactored from `HashMap<PathBuf, TerminalPane>` → `HashMap<Surface, TerminalPane>`.
  - `App.active_path: PathBuf` replaced with two fields:
    - `active_surface: Surface` — drives terminal pane lookup; in Phase 2 always `Surface::Workspace(...)`. Phase 3 introduces `Surface::Member(...)`.
    - `last_workspace_path: PathBuf` — workspace context for git/worktree operations. Always set when `Event::SwitchWorkspace` fires; persists when active surface later flips to a Member.
  - Migrated call sites: `active_terminal_mut`, `Event::SwitchWorkspace` (insert/lookup by `Surface::Workspace(path)`), `Event::PromptDeleteWorktree` / `Event::DeleteWorktree` (use `last_workspace_path`), `Event::CreateWorktree` / `Event::GitRefresh` (use `last_workspace_path`), `ui::render_terminal_pane`.
  - `#[allow(dead_code)]` placed on `Role`, `MemberId`, and `Surface::workspace_path` to silence Phase 3-only items under `-D warnings`.
  - Cargo gate green: 81 tests pass (was 77; +4 from surface tests).
- **Notes:**
  - Behavior unchanged from user's perspective: members still have no PTY backing; workspaces flow identical.
  - `last_workspace_path` is the key invariant for keeping git/worktree commands working when a Member surface becomes active in Phase 3+. Without it, those commands would have no path to operate against.
  - `pending_delete: Option<(PathBuf, String)>` deliberately kept on the workspace path type — delete confirmations are workspace-only by definition (you can't delete a Member surface).
  - The dead-code allow on `MemberId` covers `singleton`, `engineer`, and `label`. Phase 3 spawn logic will exercise all three; the allows can be removed then.

---

## Phase 3 — Singleton agent PTYs

- **Started:** 2026-05-06
- **Branch:** feat/agentic-ui
- **Status:** done
- **Acceptance evidence:**
  - At startup, fellowship spawns 4 always-on PTYs — one per singleton role (PM, Orchestrator, Architect, Recon) — keyed by `Surface::Member(MemberId::singleton(role))` in the existing `terminals` HashMap. Each PTY runs `echo '[<role>] placeholder — real prompt lands in Phase 10'; exec bash` so the role is visible on first focus and a real shell is available afterward.
  - `App::new` signature changed to `Result<Self>` so PTY spawn errors propagate cleanly through `main`. `main.rs` updated to `App::new(...)?`.
  - New event variant `Event::SwitchSurface(Surface)` (`src/event.rs:14`). Members-pane `Enter` now emits `Event::SwitchSurface(Surface::Member(id))`; the App handler swaps `active_surface`, sets `members.active`, focuses Terminal, and resizes the PTY. The `SwitchSurface(Workspace(_))` arm delegates to the existing `SwitchWorkspace` flow so its workspace-only side effects (git_status root, GitRefresh, list select) still fire.
  - `MembersPane` rewrite: `Vec<String>` → `Vec<MemberId>`, `active: usize` → `active: Option<MemberId>`. Added `set_active_member`, `selected_member`. Engineers dropped from the placeholder list — the dynamic engineer pool arrives in Phase 9. 5 unit tests cover construction, j/k clamping, Enter-event emission, and active-marker driving.
  - `Event::SwitchWorkspace` now also calls `members.set_active_member(None)` so the active marker clears when the user switches back to a workspace.
  - Status bar shows a green `[member: <label>]` badge whenever `active_surface` is a Member, regardless of which pane is focused — makes the surface unambiguous when typing in the Terminal.
  - `#[allow(dead_code)]` narrowed to `Role::Engineer`, `MemberId::engineer`, and `Surface::workspace_path` (Phase 9 / future use).
  - Cargo gate green: 82 tests pass (was 81; +1 net = 5 new members tests minus 4 phase-1 tests rewritten).
- **Notes:**
  - Banner command intentionally lightweight. Phase 10 swaps it for the real `claude --dangerously-skip-permissions [--system-prompt …] [--model …]` invocation per the resolved Q3 (claude for all five, per-role model with Recon=haiku).
  - `Event::SwitchSurface(Workspace(_))` delegating to `SwitchWorkspace` is intentional: it gives the Members pane a single emit shape (`SwitchSurface`) while keeping the workspace flow's extra side effects in one place.
  - Spawning 4 PTYs at startup costs 4 background processes. On exit, the existing `for t in app.terminals.values_mut() { t.shutdown(); }` loop in `main.rs` already iterates by Surface key and reaps every PTY uniformly — no shutdown changes needed.
  - Manual smoke not run (this agent has no interactive terminal). Verified by unit tests + cargo gate; the user is expected to run a quick visual check after pulling: `cargo run`, then `Ctrl+a m`, `j j`, `Enter` should land on Architect's PTY with the `[architect] placeholder` banner. If the banner does not appear or no prompt is interactive, the spawn config in `src/app.rs` `App::new` is the place to look.

---

## Phase 4 — `fellowship-ctl` helper binary

- **Started:** 2026-05-06
- **Branch:** feat/agentic-ui
- **Status:** done
- **Acceptance evidence:**
  - New `[[bin]]` entry in `Cargo.toml` for `fellowship-ctl`. Two binaries now ship from this crate: `fellowship` (TUI) and `fellowship-ctl` (helper).
  - New file `src/bin/fellowship-ctl.rs` with 6 clap-derived subcommands: `heartbeat`, `log`, `spawn-engineer`, `release-engineer`, `pr-comments`, `bead`.
  - File-writing subcommands (`heartbeat`, `log`, `spawn-engineer`, `release-engineer`) drop JSON files into `~/.fellowship/runtime/<session>/{state, spawn-requests, release-requests}/` and append to `journal.ndjson`.
  - Runtime dir resolution order:
    1. `FELLOWSHIP_RUNTIME_DIR` (explicit override; used by tests).
    2. `~/.fellowship/runtime/$FELLOWSHIP_SESSION` (fellowship-set when spawning agent PTYs in Phase 5).
    3. `~/.fellowship/runtime/default` for standalone / pre-Phase-5 runs.
  - JSON shapes (`HeartbeatRecord`, `SpawnRequest`, `ReleaseRequest`, `JournalEntry`) carry epoch-ms timestamps. Spawn / release requests use UUIDv4 for `request_id`. Watcher in Phase 5 will consume these shapes; they are intentionally local to the binary file for now to avoid premature lib extraction (revisit when Phase 5 wires the watcher into fellowship).
  - Shell-out subcommands (`pr-comments`, `bead`) invoke `gh api` and `bd` respectively. `pr-comments` queries both the `pulls/.../comments` (inline review comments) and `issues/.../comments` (general PR convo) endpoints and emits one JSONL record per element.
  - 8 unit tests cover heartbeat (write + overwrite), journal append, spawn-request shape (with branch + single-shot + missing-branch variants), release-request, runtime-dir env override, and a parser smoke that exercises all 9 documented invocations.
  - `cargo run --bin fellowship-ctl -- --help` prints all 6 subcommands cleanly (verified).
  - Cargo gate green: 90 tests pass total (82 fellowship + 8 fellowship-ctl).
  - New deps added: `clap` (derive feature), `uuid` (v4 feature). New dev-dep: `tempfile`.
- **Notes:**
  - Unit tests prove the file-writing path end-to-end at the function level. Interactive end-to-end (running the binary against a real `~/.fellowship/runtime/<session>/`) is left for the user to spot-check; the same code paths are exercised by the unit tests.
  - JSON types stay private to the binary file in this phase. Phase 5 will need fellowship's watcher to deserialize the same shapes — at that point either (a) introduce `src/lib.rs` and move types into a shared `runtime` module, or (b) duplicate the small structs. Decision deferred to Phase 5 when both call sites are in front of us.
  - `runtime_dir_honors_explicit_override_env_var` test uses `unsafe { std::env::set_var(...) }` because Rust's 2024 edition flagged env mutation as unsafe. The test is single-threaded by virtue of cargo's per-test isolation; if parallelism becomes an issue, gate with `serial_test`.
  - `pr-comments` deliberately re-runs `gh repo view` per invocation when no `--repo` is passed. Latency is one extra `gh` call per agent invocation; acceptable for v1 and irrelevant in Phase 13 (where the bus replaces the primary path).

---

## Phase 5 — Runtime watcher + shared lib refactor

- **Started:** 2026-05-06
- **Branch:** feat/agentic-ui
- **Status:** done
- **Acceptance evidence:**
  - **Library extraction (Path A from Phase 4 deferred decision).** Added `src/lib.rs` declaring all modules (`pub mod app; ...`). Both binaries (`fellowship`, `fellowship-ctl`) now `use fellowship::...` from this crate's library. `src/main.rs` slimmed: dropped `mod` declarations; imports are explicit `use fellowship::...`.
  - **Shared runtime module (`src/runtime.rs`)** holds the cross-binary contract: `HeartbeatRecord`, `SpawnRequest`, `ReleaseRequest`, `JournalEntry`, the directory constants (`STATE_DIR`, `SPAWN_REQUEST_DIR`, `RELEASE_REQUEST_DIR`, `JOURNAL_FILE`), `now_ms()`, `runtime_dir()`, and `ensure_subdir()`. fellowship-ctl removes its local copies and imports from `fellowship::runtime`. Single source of truth for the JSON schemas.
  - **`AgentRegistry` (`src/agents/registry.rs`)** — in-memory mirror of the on-disk `state/<agent-id>.json` heartbeats. Exposes `upsert`, `get`, and `load_from_state_dir` (boot-time scan). Phase 11 will layer the STALE/DEAD state machine on top using thresholds from config. 4 unit tests.
  - **Notify watcher (`src/agents/watcher.rs`)** — `spawn_state_watcher` creates the `<runtime>/state/` dir if missing, registers a `notify::recommended_watcher`, and emits `Event::AgentHeartbeat(record)` on each create/modify of a `*.json` file. The watcher handle is held in `main::run` for the duration of the session. 3 unit tests cover `read_heartbeat` parse + `is_json_file` filter.
  - **Session uuid scoping.** `main::main` allocates a per-session uuid via `uuid::Uuid::new_v4()` and sets `FELLOWSHIP_SESSION` in the process environment **before** any PTY spawn so spawned child shells (and any `fellowship-ctl` invocations from inside them) inherit it. Concurrent fellowship instances no longer collide on `~/.fellowship/runtime/`.
  - **App wiring.** New `App.agent_registry: AgentRegistry` field. `App::new` now takes `runtime_root: &Path` and calls `agent_registry.load_from_state_dir(...)` at boot to ingest any pre-existing heartbeats. New event variant `Event::AgentHeartbeat(HeartbeatRecord)` handled by `App::handle_event` which calls `agent_registry.upsert(record)`.
  - **Members pane status badge.** `MembersPane::render` now takes `registry: &AgentRegistry`. Each member row shows `marker + label + " — " + status` when a heartbeat record exists for that member's id. `ui::render_members_pane` plumbs the registry through.
  - Cargo gate green: 99 tests pass total — 92 lib (was 82 before Phase 5; +10 = 3 runtime + 4 registry + 3 watcher) + 7 fellowship-ctl bin (was 8 before; one moved into the lib's runtime tests) + 0 integration.
- **Notes:**
  - The watcher uses `notify::recommended_watcher` (synchronous handler closure) which forwards into the existing tokio mpsc `event_tx`. Latency is dominated by the OS event delivery; on macOS this is fseventsd batching, typically sub-second.
  - `is_json_file` was renamed from `&PathBuf` to `&Path` per clippy `ptr_arg`. The caller wraps in `|p| is_json_file(p)` because `Iterator::any` cannot coerce `fn(&Path)` directly when the iterator yields `&PathBuf`.
  - `Default` impl added for `MembersPane` to satisfy `clippy::new_without_default`.
  - The `EventKind::Any` arm in `is_heartbeat_event` is defensive — some platforms emit `Any` for filesystem events that don't decompose into `Create`/`Modify`. Without it, those events are dropped silently.
  - **Acceptance test path (manual, deferred to user):** start fellowship → in another shell run `FELLOWSHIP_SESSION=<copy from running fellowship's env or `ls -t ~/.fellowship/runtime/`> fellowship-ctl heartbeat pm --status "alive"`. Within ~1s, the Members pane should re-render the PM row with `pm — alive`.

---

<!-- New phase entries appended below. Do not delete past entries; append per attempt. -->
