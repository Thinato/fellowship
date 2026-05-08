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

## Phase 6 — Beads integration module

- **Started:** 2026-05-06
- **Branch:** feat/agentic-ui
- **Status:** done
- **Acceptance evidence:**
  - New `src/beads.rs` lib module: `BdRawIssue` (deserializer for `bd list --json`), canonical `Bead` struct + `Status` enum, `From<BdRawIssue> for Bead`, `parse_bd_list`, `list_beads_with(bd_path: &str)`, and `list_beads()` (defaults to `bd` on PATH).
  - **Schema insulation (Q1 from Phase 6 prompt):** `BdRawIssue` mirrors the upstream `bd` JSON shape with `#[serde(default)]` on every field; `Bead` is the fellowship-internal canonical shape. Drift tolerated; new upstream fields are silently ignored. `serde_json` lenient by default.
  - `Status::parse` recognizes `open`, `in_progress`/`in-progress`, `in_review`/`in-review`, `closed`/`done`; everything else → `Status::Other`.
  - 7 unit tests: `Status::parse` matrix, empty-array, blank-stdout, full mapping, unknown-field tolerance, `list_beads_with` happy path with a fake `bd` shell script (chmod 0o755, no PATH munging — invoked by absolute path), `list_beads_with` propagating non-zero exit + stderr.
  - **App wiring:** new field `App.beads: Vec<Bead>`. New event variant `Event::BeadsRefreshed(Vec<Bead>)` handled by `App::handle_event` → assigns into `self.beads`.
  - **Polling task:** `main::run` spawns a tokio interval task ticking every 3s; on each tick calls `beads::list_beads()` and forwards the result via `Event::BeadsRefreshed`. `bd` errors (binary missing, repo not initialized, etc.) are silently dropped — fellowship boots with no beads visible and the user can `bd init` later. Loop exits when the receiver drops.
  - Cargo gate green: 106 tests total — 99 lib (was 92; +7 beads) + 7 fellowship-ctl bin + 0 integration.
- **Notes:**
  - Tests construct fake `bd` binaries by writing a shell script into a `tempfile::TempDir`, chmod-ing 0o755, and passing the absolute path to `list_beads_with`. No PATH mutation, so tests stay parallel-safe under cargo's default test runner.
  - Once a real `bd` install is available in a session, the polling task picks up immediately. Real-bd compatibility check (does `bd list --json` actually emit the fields we read?) is deferred to Phase 12 end-to-end smoke; the schema insulation in `BdRawIssue` is the hedge against drift.
  - The `BdRawIssue::issue_type` field is renamed via `#[serde(rename = "type")]` to avoid clashing with Rust's `type` keyword. Plan §3.2 uses the term "kind" in label conventions but the upstream JSON field is "type"; downstream we expose it as `issue_type`.
  - Plan §3.2 also calls for `bd ready --label role:engineer --json` for engineer self-claim; that's a Phase 9 concern (engineer pool). Phase 6 ships only `bd list --json`. The wrapper is identical except for argv.

---

## Phase 7 — Status / Journal pane

- **Started:** 2026-05-06
- **Branch:** feat/agentic-ui
- **Status:** done
- **Acceptance evidence:**
  - **Container shape (Q1 from Phase 7 prompt = option B).** New `RightView` enum (`Git` | `Status`) on `App`. `ui::render_right_column` dispatches: `RightView::Git` → existing `GitStatusPane`; `RightView::Status` → new `StatusPane`. Same screen real estate; toggle via `Ctrl+a g` / `Ctrl+a s`. PaneId remains `GitStatus` (the right column is a single focusable pane regardless of sub-view).
  - **New keymap actions** `Action::FocusGitView` / `Action::FocusStatusView`. The old `Ctrl+a g → FocusPane(GitStatus)` mapping was replaced by `FocusGitView` (sets `right_view = Git` + focuses the right column). `Ctrl+a s → FocusStatusView` mirrors that for the Status view. `prefix_then_g_focuses_gitstatus` test renamed to `prefix_then_g_focuses_git_view`; new `prefix_then_s_focuses_status_view`.
  - **`StatusPane` (`src/panes/status.rs`)** — two sub-views: `Beads` (default) and `Journal`, toggled with `J`. Beads view = 4-column kanban (OPEN / IN-PROG / REVIEW / DONE) with up to 8 beads per column, titles truncated to 20 chars. Journal view = tail of last 200 entries colored per-agent (stable hash → 8-color palette). `f` toggles a single-agent filter (latches onto the most-recent entry's agent; press again to clear). 5 unit tests cover toggle, filter gating, ringbuf cap, truncate, color stability.
  - **Journal watch.** `spawn_state_watcher` renamed to `spawn_runtime_watcher`. Watches `<runtime>/state/` (heartbeats) AND `<runtime>/journal.ndjson` (journal). Journal modifications re-parse the whole file and emit `Event::JournalSnapshot(Vec<JournalEntry>)`. The watcher touches `journal.ndjson` if absent so notify has something to register from t=0.
  - **Runtime helpers.** `runtime::journal_path()` and `runtime::read_journal()` added — best-effort NDJSON parse; malformed lines skipped silently; missing file → empty vec.
  - **App wiring.** New fields: `App.status: StatusPane`, `App.right_view: RightView`. New event variant `Event::JournalSnapshot(Vec<JournalEntry>)` handled by `App::handle_event` → `self.status.replace_journal(entries)`. `dispatch_key_to_focused_pane` for `PaneId::GitStatus` now branches on `right_view` and forwards keys to `StatusPane::handle_key` when applicable (so `J`/`f` work without a separate keybind plumbing path).
  - **Status bar focus label** now shows "STATUS" or "GIT STATUS" depending on `right_view` when the right column is focused.
  - **Help overlay.** Added `Ctrl+a g  Focus Git view (right column)`, `Ctrl+a s  Focus Status view (right column)`, `J  Toggle Beads/Journal (Status view)`, `f  Toggle journal filter to recent agent`. Height bumped 19 → 21.
  - Cargo gate green: 112 tests total — 105 lib (was 99; +5 status + 1 keymap) + 7 fellowship-ctl bin + 0 integration.
- **Notes:**
  - Re-parsing the entire journal on every modification is intentional: bounded by `JOURNAL_TAIL_MAX = 200` entries displayed, the parse is cheap and avoids tracking file offsets across truncation/rotation. If the journal grows past ~10k lines and parse cost shows up in profiling, switch to incremental tail reads.
  - The journal-filter UX (`f`) is intentionally minimal for v1 — a real "filter by id" UI lands when there are >5 active agents and the simple latch becomes painful. Plan §4.2 calls for `f filters to a specific agent` so this matches scope.
  - The keymap ↔ ui contract: `RightView` lives in `app.rs` (it's app state); `keymap.rs` stays decoupled by exposing two distinct Actions instead of carrying a `RightView` value through the action enum. Trade-off: a new sub-view costs one new Action variant, but the alternative leaks app state into keymap's API.
  - `Ctrl+a Esc` cancels prefix mode but doesn't reset `right_view`; that's by design — last-set view persists across focus changes.

---

## Phase 7.5 — Smoke fixes (CURRENT_SESSION discovery, debug log, empty placeholders)

- **Started:** 2026-05-07
- **Branch:** feat/agentic-ui
- **Status:** done
- **Acceptance evidence:**
  - User smoke against Phases 1–7 surfaced three pain points; this followup addresses them in one commit on top of `11c4431` (Phase 7).

  **1. Heartbeats from a non-PTY shell wrote to the wrong dir.**
  - Root cause: `fellowship-ctl` resolved `runtime_dir` from `FELLOWSHIP_SESSION` env var, which is only set inside fellowship-spawned PTYs. Any shell started outside fellowship landed at `~/.fellowship/runtime/default/`. The running fellowship's watcher never saw those events.
  - Fix: fellowship now writes its session uuid to `~/.fellowship/runtime/CURRENT_SESSION` on boot and removes it on quit (only if the file still points at the same uuid; multi-instance safe). `runtime::runtime_dir` resolution adds a third tier between env-var and "default": read the marker file. New helpers `current_session_marker_path`, `read_current_session`, `write_current_session`, `clear_current_session` in `src/runtime.rs`.

  **2. No way to see what fellowship was doing under the hood.**
  - Fix: new module `src/debug_log.rs` initializes a global `tracing` subscriber writing to `<runtime_root>/fellowship.log`. `main` calls it at boot. Replaced existing `eprintln!` sites in `src/app.rs` (worktree add/remove failures) and `src/config.rs` (config parse failures) with `tracing::error!`. Added `debug!` on every agent heartbeat received and `info!` on member-surface switches and beads-poll recovery. Beads polling errors are now logged once-per-failure-type instead of silently dropped, and recovery is announced.
  - Tail with `tail -F ~/.fellowship/runtime/<session>/fellowship.log` while fellowship is running.

  **3. Empty kanban / journal had no hint.**
  - Fix: `panes/status.rs` `render_kanban` and `render_journal` now show a dimmed multi-line hint when their respective data is empty, with concrete commands the user can run (`bd init` / `bd create` for kanban, `fellowship-ctl log` for journal).

  **4. Session uuid was invisible to the user.**
  - Fix: status bar now appends `  session=<first-8-chars>` so the user can see at a glance which runtime dir to point ctl at. The CURRENT_SESSION marker auto-resolves it for most cases, but the visible id helps when running ctl with an explicit `FELLOWSHIP_SESSION=...` override.

  - **App.session_id field** added; threaded through `App::new(... session_id: String, ...)` and `main::run`. UI status bar reads from `app.session_id`.
  - New deps: `tracing = "0.1"`, `tracing-subscriber = "0.3"` (with the `fmt` feature).
  - Cargo gate green: 112 tests still pass (no new tests this commit; behavior is covered by manual smoke + the existing runtime/registry/watcher tests).
- **Notes:**
  - The "fallback" tier in `runtime_dir` (CURRENT_SESSION marker → default) was flagged by the slop hook; documented in code as the architectural fix to the user's smoke-test failure (heartbeats writing to `default/` instead of the live session). Single resolution chain, deterministic, and the marker is removed on clean exit.
  - **Filter UX clarification:** the user reported "`f` does nothing" in journal view. Behavior is correct given an empty journal — `f` latches onto the most-recent entry's agent, and there are no entries when no agent has called `fellowship-ctl log`. The new empty-journal hint surfaces this. After the CURRENT_SESSION fix, `fellowship-ctl log <id> "msg"` from any shell will now populate the journal and `f` will start latching.
  - tracing-subscriber's `try_init` is intentionally non-fatal: subsequent calls in the same process return `Err` but we `let _ =` it so duplicate inits don't crash the TUI.
  - The session-id status-bar substring uses `.get(..8)` (byte indexing) rather than `.chars().take(8)`. UUIDs are pure ASCII so byte slicing is safe; if we ever swap uuid for a non-ASCII session id this needs revisiting.

---

## Phase 8 — `safe-git` shim + PATH injection

- **Started:** 2026-05-07
- **Branch:** feat/agentic-ui
- **Status:** done
- **Acceptance evidence:**
  - **Policy module (`src/guard.rs`)** — pure data-in / decision-out logic implementing layer 2 of the no-merge guardrail (plan §3.7). `Tool` enum (Git / Gh / Other), `Decision` enum (Allow / Block(reason)). `decide(tool, args)` walks past leading global options and dispatches to `decide_git` / `decide_gh`. Blocked patterns:
    - `git merge ...`
    - `git push --force` / `-f` / `--force-with-lease`
    - `git push origin main|master` (also `refs/heads/main`, `HEAD:main`, etc.)
    - `gh pr merge ...`
  - **Shim binary (`src/bin/safe-git.rs`, new `[[bin]]`)** — argv[0]-driven dispatcher. Detects whether it was invoked as `git` or `gh` via the symlink name, calls `guard::decide`, and either:
    - exits non-zero with a stderr message naming the violation, or
    - strips the shim's directory from PATH and `exec`s the real binary.
  - **Install helpers (`guard::install_shims`, `guard::shim_dir`, `guard::locate_safe_git`, `guard::path_with_shim_prepended`)** — fellowship boot:
    1. Locates `safe-git` next to its own binary (same cargo build → same dir).
    2. Creates symlinks `~/.fellowship/bin/{git,gh}` → that path. Idempotent (existing symlink with the right target is left alone; stale target is replaced; non-symlink at the path is refused with a clear error).
    3. Computes a PATH override `${HOME}/.fellowship/bin:${original-PATH}` and threads it into `App::new`.
  - **Per-surface PATH injection.** `TerminalPane::spawn_with_env(rows, cols, cwd, tx, startup_cmd, extra_env)` is the new full-form constructor; the existing `spawn` delegates with empty env. Member surfaces (PM / Orchestrator / Architect / Recon PTYs in `App::new`) are spawned via `spawn_with_env` with `[("PATH", agent_path)]`. Workspace surfaces continue calling `spawn` and inherit fellowship's unmodified PATH — the user's own terminal is not affected.
  - **Boot fails loudly** if the shim cannot be installed. `App::new` does not silently degrade because the failure mode (agents pushing to master with `--dangerously-skip-permissions`) is exactly what this phase guards against.
  - **`tracing::info!` on shim install** records the resolved shim_dir and safe-git path in `<runtime>/fellowship.log` for visibility.
  - **Tests:** 16 unit tests in `guard.rs` cover every blocked / allowed pattern (`git merge`, force-push variants, push to main/master via plain ref / `refs/heads/` / `HEAD:` refspec / variations on prefix-only flags, feature-branch happy path, unrelated subcommands pass through, `gh pr merge` blocked, `gh pr {create,view,list,checkout,diff,comment}` allowed, `gh repo view` allowed, empty argv allowed, install-shims happy path / idempotency / stale-symlink replacement / refusal to clobber non-symlinks, PATH prepend ordering).
  - Cargo gate green: 128 tests total — 121 lib (was 105; +16 guard) + 7 fellowship-ctl bin + 0 safe-git bin (binary entry only; logic is in `guard`).
- **Notes:**
  - Plan §3.7 specifies three guardrail layers; Phase 8 ships layer 2 (PATH shim). Layer 1 (system-prompt prohibitions) lands with the role markdown in Phase 10. Layer 3 (GitHub branch protection) is documented in Phase 9 / 12 and not enforced by fellowship.
  - The shim is bypassable by an agent that calls `/usr/bin/git` directly. This is acknowledged in plan §9 as a known limitation; layer 3 (branch protection) is the load-bearing defense against that. Not a fix-for-this-phase concern.
  - `Tool::Other` short-circuits to `Decision::Allow` so the binary is safe to invoke under unexpected names without breaking the host. The binary entry refuses to exec in that case (it does not know which real tool to forward to) and exits with code 2.
  - `--force-with-lease` is treated identically to `--force` in this phase. Even though `--force-with-lease` is safer than `--force`, agents pushing without human review can still rewrite history visible to humans; only humans run force-pushes in this design.
  - `git rebase` is **not** blocked. Agents need rebase to integrate upstream changes onto feature branches before pushing. Rebasing onto a protected branch is implicitly blocked at push time (agents can't push the rewritten branch back to main/master). Plan §9 risk row "shim bypass via direct binary path" stays the relevant limitation.

---

## Phase 9 — Engineer spawn protocol

- **Started:** 2026-05-07
- **Branch:** feat/agentic-ui
- **Status:** done
- **Acceptance evidence:**
  - **Watcher consumes intent files at-most-once.** `spawn_runtime_watcher` now also watches `<runtime>/spawn-requests/` and `<runtime>/release-requests/`. On any new `*.json` write under those dirs the watcher reads the file, parses it, **deletes it**, then emits `Event::SpawnRequestReceived(SpawnRequest)` or `Event::ReleaseRequestReceived(ReleaseRequest)`. The delete-after-parse pattern guarantees a request is processed once even when notify fires duplicate events for the same write (common on macOS).
  - **New event variants** `Event::SpawnRequestReceived(SpawnRequest)` and `Event::ReleaseRequestReceived(ReleaseRequest)` (plumbed through `App::handle_event`).
  - **App engineer pool plumbing.** New fields on `App`:
    - `agent_path: String` — PATH override (with safe-git shim) injected into every member-surface PTY.
    - `max_engineers: usize` — Phase 9 hardcodes 4. Phase 10 wires this from `Config::agents.max_engineers`.
    - `spawn_queue: VecDeque<SpawnRequest>` — overflow queue when at capacity.
  - **`App::handle_spawn_request`** checks `live_engineer_count() >= max_engineers`. At/over cap → push request onto `spawn_queue` and log `info!`. Under cap → call `spawn_engineer`.
  - **`App::spawn_engineer`** flow:
    1. Require `req.branch` (Phase 9 enforces; Phase 10 may auto-derive from a claimed bead).
    2. Allocate the smallest unused `engineer-K` instance via `next_engineer_instance` (1, 2, 3, …).
    3. Run `git worktree add` against `last_workspace_path` for the requested branch — reuses Phase 0–7 worktree path scheme `$HOME/.fellowship/worktrees/<owner>/<repo>/<branch>`.
    4. Spawn a `TerminalPane` rooted at the new worktree with env `[("PATH", agent_path), ("AGENT_ID", "engineer-K"), ("FELLOWSHIP_SPAWN_REQUEST_ID", req.request_id)]`. Banner is the Phase 3 placeholder shape; Phase 10 swaps for real `claude`.
    5. Insert under `Surface::Member(MemberId::engineer(K))` in `App.terminals` and add to `MembersPane.members` so the Members pane renders it.
    6. Refresh worktrees list so the Workspaces pane sees the new entry too.
    7. `info!` the result.
  - **`App::handle_release_request`** parses the agent_id with `parse_engineer_id` (rejects singletons), removes the PTY (calls `shutdown`) and member entry, and **drains one queued spawn request** if any so the queue makes progress without external nudging.
  - **`MembersPane` API additions** — `add_member`, `remove_member` (clears active marker if removed; clamps selection), and `engineer_instances` for the allocator.
  - **5 new unit tests** — `parse_engineer_id` accepts `engineer-<n>` and rejects singletons / garbage / empty (2 tests); `add_member` appends + dedupes; `remove_member` clears active and clamps selection; `engineer_instances` returns only Engineer-role ids.
  - Cargo gate green: 133 tests total — 126 lib (was 121; +5) + 7 fellowship-ctl bin + 0 integration.
- **Notes:**
  - **`max_engineers = 4` hardcoded** for Phase 9. Plan §3.5 / §5 wire this from config in Phase 10. Beyond cap, requests queue (FIFO) and drain on release.
  - **Branch is required** for Phase 9. The plan envisions Orchestrator deriving `feat/bead-<id>` after engineer self-claims a bead, but the spawn protocol itself stays simple by requiring the branch upfront. Phase 10 can add a `--auto-branch` flag that derives a placeholder name like `feat/engineer-<n>-scratch` if needed.
  - **Watcher delete-after-parse.** macOS fseventsd often fires multiple Modify events for a single write (especially from editors that write+rename). Without consume-and-delete, fellowship would spawn the same engineer twice. The deleted file is the canonical "this request was handled" signal.
  - **`fellowship-ctl spawn-engineer` from Phase 4** is unchanged. It still drops `<runtime>/spawn-requests/<uuid>.json`; the watcher now actually consumes those.
  - **Status pane / journal updates** are not retroactive: a spawn that happens before the watcher fires its first event won't be replayed. Acceptable since fellowship is the only writer of spawn-request files in normal operation.

---

## Phase 10 — Role prompts (real claude invocations) + PM default focus

- **Started:** 2026-05-08
- **Branch:** feat/agentic-ui
- **Status:** done
- **Acceptance evidence:**
  - **Five real role prompt files** at `agents/{pm,orchestrator,architect,recon,engineer}.md`, replacing the Phase 0 placeholders. Each prompt covers: identity & tone; primary responsibilities; allowed tools (with `fellowship-ctl` bead/heartbeat/log/spawn-engineer/release-engineer/pr-comments commands explicit); forbidden commands (`git merge`, `git push --force`, `git push origin (main|master)`, `gh pr merge`, plus role-specific prohibitions); heartbeat protocol; coordination bus (beads only); and concrete success criteria. Engineer prompt encodes the Q2 self-claim model (`bd ready --label role:engineer` + `bd update --claim`) and the Q4 v1 path for review feedback (`fellowship-ctl pr-comments`).
  - **`src/agents/spawn.rs`** new module owns spawn planning. `prompt_for(Role)` returns the role's markdown via `include_str!` (prompts are baked into the binary; no runtime file IO). `default_model_for(Role)` returns `Some("claude-haiku-4-5")` for Recon and `None` for the rest, per Q3 resolution. `plan_for(role, agent_id, claude_available) -> SpawnPlan` returns the bash command line and the `AGENT_PROMPT` / `AGENT_ID` / optional `AGENT_MODEL` env vars. `claude_available_at_boot()` probes once via `claude --version`.
  - **Real claude invocation shape** when claude is on PATH:
    - `exec claude --dangerously-skip-permissions --append-system-prompt "$AGENT_PROMPT"` (PM, Orchestrator, Architect, Engineer)
    - `exec claude --dangerously-skip-permissions --model "$AGENT_MODEL" --append-system-prompt "$AGENT_PROMPT"` (Recon, with `AGENT_MODEL=claude-haiku-4-5`)
    - The prompt is delivered via env var (not on the command line) to avoid shell quoting issues with multi-line markdown. `exec` replaces bash so the PTY is owned by claude directly; when claude exits, the PTY closes (Phase 11 watchdog will pick that up later).
  - **Claude-missing path (option 2A from Phase 10 prompt)** falls back to a banner that drops the user into bash with `AGENT_PROMPT` still set, plus a `warn!` log on boot. Fellowship still functions as a worktree TUI on machines without claude installed.
  - **PM default focus (Q5 resolution)** wired in `App::new`. When `claude_available`, the boot state has `active_surface = Surface::Member(MemberId::singleton(Role::Pm))` and `members.active = Some(pm)`, so the user lands on PM's PTY immediately. When `claude_available = false`, falls back to the workspace surface as before.
  - **Engineer spawn now uses the helper** in `App::spawn_engineer` so engineers get the same real-claude shape and inherit the Engineer role prompt.
  - **`App.claude_available: bool` field** carries the boot probe through the session so spawn decisions stay consistent across the lifetime.
  - **`#[allow(clippy::too_many_arguments)]`** on `App::new` (8 params now). Bundling into a config struct is a future cleanup.
  - 6 new unit tests in `agents::spawn::tests`: every role's prompt is non-empty and contains a `Role:` header; `default_model_for` matrix; real-invocation shape (real claude line + env keys including PM-specific prompt content); Recon model wiring; missing-claude banner shape with `AGENT_PROMPT` still present; engineer agent_id propagation.
  - Cargo gate green: 139 tests total — 132 lib (was 126; +6 spawn) + 7 fellowship-ctl bin + 0 integration.
- **Notes:**
  - **No new config keys** — Phase 10 ships the "1A" path (light config). The role prompts are baked in via `include_str!` and Recon's haiku model is hardcoded. Phase 10.5 (or later) wires `[agents]` and `[agents.<role>]` blocks per plan §5 if/when users want per-deployment overrides.
  - **`include_str!` cost:** each agent prompt is ~80–120 lines of markdown, so the binary is ~5KB heavier. Negligible. The benefit is that `cargo install --path .` produces a self-contained binary that doesn't need `agents/` shipped alongside.
  - **`exec`-into-claude design** means the PTY's bash exits as soon as claude starts. If the user wants to drop back to bash without exiting fellowship, they'd need to kill the PTY (which Phase 11 watchdog would auto-restart). Acceptable — agent surfaces are not meant for interactive shell work.
  - The `Phase 3 placeholder` comment block was kept in `App::new` because the loop structure is unchanged; only the spawn shape moved into the helper. Phase 3 is still the right reference for "why each role gets a singleton PTY at boot."
  - `claude --version` is the cheapest probe; takes ~50ms. Once-at-boot is the right cadence.

---

## Phase 11 — Watchdog (STALE / DEAD / FAILED + auto-restart)

- **Started:** 2026-05-08
- **Branch:** feat/agentic-ui
- **Status:** done
- **Acceptance evidence:**
  - **Liveness state machine.** `src/agents/registry.rs` adds `Liveness { Live, Stale, Dead, Unknown }` plus the plan §3.6 thresholds (`HEARTBEAT_WARN_SEC = 60`, `HEARTBEAT_DEAD_SEC = 180`). `AgentRegistry::liveness_for(agent_id, now_ms)` derives the state from the most-recent heartbeat record. `Unknown` is the no-record case (placeholder PTYs, fresh boot before first heartbeat) — explicitly distinct from `Stale` so the watchdog leaves no-telemetry members alone instead of restart-looping them.
  - **Watchdog tick.** `main::run` now spawns a 5s tokio interval that emits `Event::WatchdogTick`. `App::run_watchdog` walks `members.members`, computes liveness per id, and acts:
    - `Live` / `Unknown` / `Stale` — no-op (Stale logs a `debug!`).
    - `Dead` with `restart_counts < max_restarts` — increments the counter, calls `restart_agent`, logs `info!`.
    - `Dead` with `restart_counts >= max_restarts` — adds to `failed_agents`, logs `error!`. No further restarts.
  - **Restart logic.** `App::restart_agent(id)` shuts down the old PTY, picks the right cwd (engineer's recorded worktree for `Role::Engineer`, `last_workspace_path` for singletons), re-runs the same `agents::spawn::plan_for + execute` path so the role prompt and shim PATH are preserved across restarts.
  - **Heartbeat recovery.** `Event::AgentHeartbeat` now also clears `restart_counts.remove(&id)` so a healthy agent that catches up isn't punished for a transient earlier silence. `singleton_id_for_label` resolves `pm` / `orchestrator` / `architect` / `recon` strings to their `MemberId`; `parse_engineer_id` handles `engineer-<n>` (already from Phase 9). Both are tried.
  - **Engineer worktree memory.** New `App.engineer_worktrees: HashMap<MemberId, PathBuf>` populated in `spawn_engineer` and cleared on `release-engineer`. `restart_agent` reads it so engineers come back in their original worktree (worktrees are not recreated on restart).
  - **Status-bar escalation banner.** When `failed_agents` is non-empty, the status bar appends a red `[!] watchdog failed: <ids>` span. Persistent until the user releases the failed agent.
  - **Members pane liveness badges.** `MembersPane::render` takes `now_ms` and `&HashSet<MemberId>` (the failed set); each row appends `[WORK]` (green) / `[STALE]` (yellow) / `[DEAD]` (red) / nothing (Unknown) — and forces `[DEAD]` red if the watchdog has given up regardless of latest heartbeat age.
  - **Cargo gate green: 148 tests total** — 141 lib (was 135; +6 = 4 liveness threshold tests + 2 singleton_id_for_label) + 7 fellowship-ctl bin + 0 integration.
- **Notes:**
  - **Hardcoded constants** (`HEARTBEAT_WARN_SEC=60`, `HEARTBEAT_DEAD_SEC=180`, `max_restarts=3`, watchdog tick=5s) match plan §3.6 defaults. Phase 10.5+ wires them from `Config::agents`.
  - **Restart preserves the role prompt** because `restart_agent` calls `agents::spawn::plan_for(...)` again — the role markdown is `include_str!`'d so any in-flight changes to `agents/<role>.md` only take effect on full fellowship restart, not per-PTY restart.
  - **`Unknown` liveness is intentional** — fellowship boots and immediately the watchdog sees members that haven't heartbeat yet. Treating Unknown as a no-op means we don't restart-storm at boot. Once a member produces its first heartbeat, the clock starts.
  - **`Stale` does not yet emit a warning bead.** Plan §3.6 envisions a watchdog-authored bead `[fellowship-watchdog] agent <id> stale` consumed by the Orchestrator. That arrives in a Phase 12 follow-up — Phase 11 acceptance only requires the badge + restart loop, which is in place.
  - **The user's PTY can fall through to plain shell** after claude exits (Phase 10's `; exec ${SHELL:-/bin/bash} -li`). When that happens, the heartbeat clock keeps ticking against the agent_id (no heartbeats from a plain shell). Watchdog will mark Dead at 180s and restart the PTY, replacing the user's shell with claude. This is intentional for autonomous operation; a future config flag could opt-out for interactive debugging.

---

<!-- New phase entries appended below. Do not delete past entries; append per attempt. -->

## Phase 12 — End-to-end smoke + asciicast

- **Started:** 2026-05-08
- **Branch:** feat/agentic-ui
- **Status:** in-progress
- **Acceptance evidence:**
  - **Watchdog stale-warn bead (Phase 11 follow-up).** `decide_watchdog_actions` now accepts `stale_warned: &HashSet<MemberId>` and emits two new actions: `WarnStale(id)` (first tick a member enters Stale; side-effect spawns `bd create --silent --title='[fellowship-watchdog] agent <id> stale' --description='Agent has not produced a heartbeat in over HEARTBEAT_WARN_SEC. Watchdog will auto-restart at HEARTBEAT_DEAD_SEC. Orchestrator may intervene…' --labels=fellowship-watchdog,agent-stale --type=task` via `tokio::spawn` so the watchdog tick doesn't block on bd I/O) and `ClearStale(id)` (member transitioned out of Stale — recovered to Live, escalated to Dead, or no telemetry — reset so next stale-window re-warns). New field `App.stale_warned: HashSet<MemberId>` mirrors the action stream. `AgentHeartbeat` handler also clears `stale_warned` defensively. Plan §3.6 watchdog-bead requirement is now satisfied.
  - **`beads::create_bead_with` + `create_bead`.** New async helper in `src/beads.rs` shells out to `bd create --silent` and parses the new bead id from stdout. Two new tests: argv-capturing (verifies `--silent`, title, description, comma-joined labels are passed) and failure-propagation (non-zero exit surfaces stderr). The `--silent` flag (verified via `bd create --help`) makes the id parse trivial.
  - **Test deltas.** `decide_watchdog_actions` test signature gained the `stale_warned` parameter at slot 5 (between `failed_agents` and `max_restarts`) — eight existing call sites updated. Two pre-existing tests (`watchdog_emits_note_stale_for_stale_members`, `watchdog_handles_mixed_member_set_in_one_pass`) updated to expect `WarnStale` alongside `NoteStale`. Four new tests cover the Phase 12 surface: warn-once-per-window, clear-on-recovery (Stale→Live), clear-on-Dead-progression (so the Restart action ships with a paired ClearStale), clear-on-Unknown (post-restart registry is empty so liveness goes Unknown — must reset the warned-set so the next stale window re-warns).
  - **Cargo gate green.** 163 tests total — 156 lib (was 150; +6 = 4 watchdog Stale-warn tests + 2 beads create-bead tests) + 7 fellowship-ctl bin + 0 integration. `cargo check && cargo fmt --all -- --check && cargo clippy --all-targets -- -D warnings && cargo test --all-targets` all clean.
  - **Smoke playbook.** `docs/agentic-ui-demo.md` is the deterministic recipe for the asciicast: pre-flight checks (bd / claude / gh / asciinema versions, gh auth, fellowship binary fresh, demo branches purged), four recording segments (PM creates bead → Orchestrator routes → Engineer self-claims, implements, opens PR → shim refusal of `gh pr merge`, `git push --force-with-lease`, `git push origin master`), exact expected shim stderr lifted from `src/guard.rs`, and post-record cleanup (close PR unmerged, delete branches, close demo bead). Demo target is **this fellowship repo on a disposable branch** per user decision; the demo PR is closed without merging.
  - **Asciicast** — pending. `docs/agentic-ui-demo.cast` is captured during the live recording session driven by the playbook; this journal entry will be flipped to `done` once the cast is committed alongside.
  - **First-attempt smoke + architectural pivot.** First demo attempt failed at segment 2 — PM created the bead correctly but neither the Orchestrator nor an Engineer picked it up. Root cause: claude is request-response; the role prompt says "run this loop every ~30s" but claude has no internal scheduler — after answering the system prompt it sits idle until a user types. Fix: replaced the LLM Orchestrator with a native fellowship-side tokio loop. Plan §11 changelog entry on 2026-05-08 captures the architectural change. New module `src/agents/orchestrator.rs` polls `bd list --json` every `DEFAULT_POLL_SECS=5`, runs a pure `decide_spawns(beads, already_spawned, current_pool, max_engineers) -> Vec<bead_id>` and (for each routed bead) calls `runtime::write_spawn_request(runtime_root, Some(format!("feat/bead-{id}")), false)` — fellowship's existing watcher then consumes the request and spawns an engineer PTY into a fresh worktree. In-memory `HashSet<String>` of routed bead ids prevents duplicate spawns within one session. 8 new pure-decision tests (route open beads, skip claimed/in-progress/closed, skip non-engineer roles, skip already-routed, respect pool cap, label-required helper). `Role::Orchestrator` removed from `SINGLETON_ROLES` (no LLM PTY); enum variant + `singleton_id_for_label("orchestrator")` parser kept for heartbeat / journal id continuity. `write_spawn_request` promoted from `src/bin/fellowship-ctl.rs` private fn to `runtime::write_spawn_request` public so both the native orchestrator and the `fellowship-ctl spawn-engineer` ctl path agree on the on-disk shape. `agents/orchestrator.md` keeps the original prompt under a deprecation banner. Members pane test (`new_lists_three_singletons_no_active`) updated to reflect the 3-singleton list (PM, Architect, Recon); `enter_emits_switch_surface_event` updated since Architect is now index 1 instead of 2. Cargo gate green: 171 tests total — 164 lib (was 156; +8 = 8 orchestrator decide tests) + 7 fellowship-ctl bin.
- **Notes:**
  - **Why fire-and-forget for the stale-warn bead.** The watchdog tick fires every 5s; a slow `bd create` (Dolt commit, network) shouldn't pin the tick or starve the next member's check. Failure is logged via `error!` but does not block the watchdog. The trade-off: a transient `bd` outage produces no warn-bead, only a log line. Acceptable — the user still sees the `[STALE]` badge in the Members pane and the underlying restart-on-Dead behavior is independent of the bead authoring.
  - **Why clear-on-Unknown.** After `restart_agent` clears the registry record, liveness becomes `Unknown` until the new PTY's first heartbeat. Without an explicit `ClearStale` for the Unknown branch, a member that died while warned would keep its warned-flag set across the restart, suppressing a fresh warning the next time it goes Stale. The new test `watchdog_clears_stale_warned_on_unknown_after_restart` pins this behavior.
  - **The bead authoring depends on `bd` being installed and pointed at a workspace.** When `bd create` fails (no workspace, network down), the watchdog logs the error and continues — `stale_warned.insert(id)` still runs, so the bead is **not** retried in the next tick (intentional: better one missed bead than a retry storm filling the bd board with duplicate stale alerts). A user remediation step is to clear the in-memory warned-set by triggering a recovery (member heartbeats again) or restart (member transitions to Dead → ClearStale).
  - **Demo recording is interactive and depends on real claude latency.** Plan estimate: ≈ 5 minutes wall-time end-to-end, compressed via `--idle-time-limit 3` in asciinema. If the recording goes off-script (claude prompt drift, `bd update --label` flag mismatch — bd's flag is `--labels`, the PM prompt at `agents/pm.md:29` uses `--label`), the run is aborted, the underlying issue gets a follow-up bead, and the recording is restarted from the top. The playbook acceptance checklist explicitly includes "demo PR is closed, not merged".
  - **Agent prompt fix.** `agents/pm.md` line 29 incorrectly named `bd update --label <label>`. `bd update` actually exposes `--add-label` / `--remove-label` / `--set-labels` (verified via `bd update --help`); a single `--label` flag is unrecognized and would have failed the demo's first step. Replaced with the explicit add/remove/priority forms. `bd ready --label` (singular) and `bd create --labels` (plural) are both valid and unchanged.

---

<!-- New phase entries appended below. Do not delete past entries; append per attempt. -->
