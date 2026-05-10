# Plan: Agentic UI for Fellowship

**Status:** Draft v1
**Branch:** `feat/agentic-ui`
**Created:** 2026-05-06
**Last updated:** 2026-05-06

---

## 1. Goal

Turn fellowship from a passive worktree TUI into a multi-agent dashboard. The user chats with the **Product Manager (PM)** in plain language; PM creates **beads** (https://github.com/gastownhall/beads) capturing tasks; the **Orchestrator** watches the bead board, spawns Engineers in fresh worktrees per task, and Engineers open PRs. The user sees everything in real time via a new **Members** pane and a **Status / Journal** view.

The **user keeps full code-review authority**. Agents may open PRs but never merge. Branch protection enforces this at the GitHub layer; a `safe-git` wrapper enforces it locally.

## 2. Roles

Five roles. Four singletons + an Engineer pool.

| Role | Singleton? | Worktree | Primary surface |
|---|---|---|---|
| Product Manager | Yes | Repo root | Chat with user, creates beads |
| Orchestrator | Yes | Repo root | Watches beads, spawns/assigns engineers, supervises watchdog signals |
| Architect | Yes | Repo root | Design docs, ADRs, called by Orchestrator on `type=design` beads |
| Codebase Recon | Yes | Repo root (read-only) | Surveys structure, deps, history; produces briefs on request |
| Engineer | Pool (N) | One per bead | Implements bead, opens PR, fetches & resolves PR comments |

Each role has a system prompt loaded from `~/.config/fellowship/agents/<role>.md` (global) merged with `<repo>/.fellowship/agents/<role>.md` (local override per repo).

## 3. Architecture

### 3.1 Process model — Path A (PTY per agent)

Reuse the existing per-worktree PTY infrastructure. Generalize the terminal-keying enum:

```rust
enum Surface {
    Workspace(PathBuf),
    Member(MemberId),
}
// MemberId = { role: Role, instance: u32 }
App.terminals: HashMap<Surface, TerminalPane>
```

Each agent is a long-running CLI invocation (default `claude --dangerously-skip-permissions`, configurable per role). Switching focus to a member swaps which PTY the central Terminal pane displays. The user "talks to" an agent by typing into that pane.

### 3.2 Coordination bus — beads only

Beads is the **single source of truth** for inter-agent work. No socket, no in-process bus.

**Engineer pull model** (race-safe, decentralized):
- Engineers poll `bd ready --json` filtered by `role:engineer` label every ~30s. `bd ready` returns only beads with no open blockers, so dependency ordering is automatic.
- Engineers atomically claim with `bd update <id> --claim` (sets assignee + status=in-progress in one shot — verified atomic by upstream).
- No race-recovery code needed. No Orchestrator round-trip per task.

**Orchestrator's residual routing duties:**
- Spawning + reaping engineer PTY capacity (engineer pool sizing).
- Routing non-engineer beads (`role:architect`, `role:recon`) — these are singletons, not pools, so direct `bd update <id> --assign <role>` is fine.
- Consuming watchdog warning beads + escalating to user.
- Optional priority shaping: re-labeling beads as `priority:p0`-`p3` based on user intent expressed via PM.

Bead label conventions:
- `role:pm` / `role:orchestrator` / `role:architect` / `role:recon` / `role:engineer`
- `kind:design` / `kind:implementation` / `kind:bugfix` / `kind:research` / `kind:review-feedback`
- `priority:p0` … `p3`

**Beyond labels — leverage upstream features:**
- `bd setup claude` is run once per repo to install agent-side hooks/AGENTS.md guidance (saves us writing those instructions ourselves).
- `bd dep add` + `bd graph` are surfaced in Architect's prompt for dependency planning.
- `bd remember` replaces ad-hoc agent scratch notes.
- `bd` messaging type + `--thread` is reserved for **v2** (Phase 13) — agent↔agent chatter through threads keeps fellowship's journal focused on fellowship↔agent traffic only.

### 3.3 Fellowship → Agents — `fellowship-ctl` helper binary

Ship a second binary in this crate. Agents invoke `fellowship-ctl <subcommand>` from their PTYs. The helper writes JSON files into `~/.fellowship/runtime/<session>/` which fellowship watches via `notify`.

Subcommands:

| Command | Effect |
|---|---|
| `fellowship-ctl heartbeat <agent-id> --status "<msg>"` | Write `state/<agent-id>.json` with `last_seen` ts + status text |
| `fellowship-ctl log <agent-id> "<msg>"` | Append to `journal.ndjson` |
| `fellowship-ctl spawn-engineer [--branch <name>] [--single-shot]` | Drop `spawn-requests/<uuid>.json`; fellowship spawns worktree + engineer PTY. Engineer self-claims via `bd ready` once running. `--single-shot` exits after one bead. |
| `fellowship-ctl release-engineer <agent-id>` | Drop `release-requests/<uuid>.json`; fellowship reaps PTY + (optionally) keeps worktree |
| `fellowship-ctl pr-comments <pr-number> [--repo <slug>]` | Fetch unresolved review comments + general PR comments via `gh api` (pulls + issues endpoints), dedup, emit JSONL of `{path,line,body,resolved,author,created_at,thread_id}`. Engineer uses this when working a `kind:review-feedback` bead. |
| `fellowship-ctl bead <args…>` | Thin passthrough to `bd` so agent system prompts can stay tool-agnostic |

The helper is the **only** way an agent can ask fellowship to do anything. Keeps the boundary explicit and auditable.

### 3.4 Agents → Fellowship — heartbeats

Every agent's system prompt mandates a heartbeat every ~30s and on every state transition:

> "Before each tool call, run `fellowship-ctl heartbeat $AGENT_ID --status \"<one-line what you're doing>\"`. After completing or failing a step, run `fellowship-ctl log $AGENT_ID \"<detail>\"`."

Fellowship's watchdog (see §3.6) reads these.

### 3.5 Worktree lifecycle

**Fellowship owns worktree creation/destruction.** Orchestrator never runs `git worktree add`.

Flow when capacity is needed:
1. Orchestrator (or fellowship's auto-scaler) calls `fellowship-ctl spawn-engineer --branch <auto-or-explicit>`. Bead id is **not** required upfront — engineer self-claims after spawn.
2. Fellowship reads request file, allocates `engineer-K` instance id (next free), runs `git worktree add` at `$HOME/.fellowship/worktrees/<owner>/<repo>/<branch>` (existing logic), spawns a `claude` PTY in that worktree with the engineer system prompt + env `AGENT_ID=engineer-K`.
3. Engineer's loop: poll `bd ready --label role:engineer --json` → pick top → `bd update <id> --claim` (atomic) → if claim succeeded, work; if it lost the race, pick next. Opens PR; comments PR url back on bead.
4. On bead close (or watchdog signal), engineer either keeps polling for more work or exits if `--single-shot` was passed at spawn. Default: long-running, picks up next bead. `release-engineer engineer-K` reaps the PTY (worktree stays until user deletes via existing `d` keybind).

Engineer pool size cap (config `max_engineers`, default 4) — spawn requests beyond cap are queued.

### 3.6 Watchdog

Background tokio task in fellowship:
- Tick every 5s. For each registered agent, check `state/<agent-id>.json`.
- `last_seen` older than `heartbeat_warn_sec` (default 60) → set state `Stale`, push warning bead `[fellowship-watchdog] agent <id> stale` assigned to Orchestrator.
- `last_seen` older than `heartbeat_dead_sec` (default 180) → set state `Dead`, attempt restart (re-spawn PTY in same worktree, increment restart counter). Notify Orchestrator via bead.
- Restart counter ≥ `max_restarts` (default 3) → leave dead, escalate to user via persistent banner in status bar.

### 3.7 Guardrails — no agent merges

Three independent layers. All three required.

1. **System-prompt prohibition** in every agent's role file:
   > "Forbidden commands: `git merge`, `git rebase` onto protected branches, `gh pr merge`, `git push --force`, `git push origin master`, `git push origin main`. You only push to feature branches you own. You never merge."
2. **`safe-git` wrapper.** Fellowship installs a shim at `~/.fellowship/bin/git` and prepends it to the agent PTY's `PATH`. The shim parses argv and `exit 1`s on forbidden subcommands with a loud message; otherwise execs the real `git`. Same shim for `gh` covering `pr merge`.
3. **GitHub branch protection** (documentation + an optional `fellowship setup-protection` subcommand that calls `gh api` to set protection on `main`/`master`: require PR, require approving review, restrict who can push). Documented in README; not enforced by fellowship itself.

The shim is the load-bearing one: works offline, works on private hosts, works pre-merge.

## 4. UI changes

### 4.1 Members pane

New top-half of the left column. Below it, the existing Workspaces pane fills the bottom half.

```
┌─ Members ────────────┐
│ * PM                 │  ← active member (focused PTY)
│   Orchestrator (3)   │  ← agent has 3 unread log lines
│   Architect          │
│   Recon              │
│   engineer-1 [WORK]  │  ← state badge: WORK/IDLE/STALE/DEAD
│   engineer-2 [STALE] │
└──────────────────────┘
┌─ Workspaces ─────────┐
│ * master             │
│   feat/bead-123      │
│   feat/bead-124      │
└──────────────────────┘
```

Keys (when Members focused):
- `j` / `k` — move
- `Enter` — select; Terminal pane swaps to that agent's PTY
- `r` — restart selected agent (manual; bypasses watchdog)
- `L` — open journal pane filtered to this agent

Existing prefix bindings (`Ctrl+a e/t/g/o/h/j/k/l/1..9/:/?`) keep working. Add `Ctrl+a m` → focus Members. Add `Ctrl+a s` → focus right column + flip to Status view.

**Decision (Q5):** When fellowship boots with `agents.enabled = true`, initial focus is the **PM member** (Members pane selection = PM, focused pane = Terminal showing PM's PTY). When `agents.enabled = false`, initial focus stays at the repo-root Terminal (current behavior).

### 4.2 Status / Journal pane

**Decision (Q1):** Tab toggle inside the existing right column. Same area as Git Status. Layout stays 3-column. Two views, same key area:

- `Ctrl+a g` → focus right column + show Git view (diff + PR)
- `Ctrl+a s` → focus right column + show Status view
- When right column already focused, `Ctrl+a g` / `Ctrl+a s` flips the view in place. Status view itself has a sub-toggle `J` for Journal (see below).

Two sub-views inside the Status view:

**Beads view (default):**
```
┌─ Beads ──────────────────────────────┐
│ OPEN          IN-PROG     IN-REVIEW  │
│ #128 bug…     #123 feat…  #119 ref…  │
│ #129 docs…    #124 perf…             │
│                                      │
│ DONE (last 5)                        │
│ #118 cleanup  → PR #45 merged        │
└──────────────────────────────────────┘
```

**Journal view (toggle with `J`):**
Tail of `journal.ndjson`, newest at bottom. Color-coded by agent. `f` filters to a specific agent.

Source data:
- Beads: `bd list --json` polled every 3s.
- PR state: existing `gh pr view --json` per branch.
- Journal: file watch on `journal.ndjson`.

## 5. Configuration

Add to existing config schema (`~/.config/fellowship/config.toml` + `<repo>/.fellowship/config.toml`):

```toml
[agents]
enabled = true
default_cli = "claude --dangerously-skip-permissions"
max_engineers = 4
heartbeat_warn_sec = 60
heartbeat_dead_sec = 180
max_restarts = 3

[agents.pm]
cli = "claude --dangerously-skip-permissions"
prompt_path = "agents/pm.md"   # resolved against config dir
# model = "claude-sonnet-4-6"  # optional; appended as --model <value> when set

[agents.orchestrator]
cli = "claude --dangerously-skip-permissions"
prompt_path = "agents/orchestrator.md"

[agents.architect]
cli = "claude --dangerously-skip-permissions"
prompt_path = "agents/architect.md"

[agents.recon]
cli = "claude --dangerously-skip-permissions"
prompt_path = "agents/recon.md"
model = "claude-haiku-4-5"     # default for recon — read-only sweeps don't need top-tier

[agents.engineer]
cli = "claude --dangerously-skip-permissions"
prompt_path = "agents/engineer.md"
```

**Decision (Q3):** Default CLI is `claude --dangerously-skip-permissions` for all five roles. Model is per-role configurable via `model` (passed as `--model <value>` when set; omitted otherwise so the CLI picks its own default). Shipped defaults: Recon = `claude-haiku-4-5`; all others unspecified (CLI default = Sonnet).

Per-repo overrides win per-field (existing merge logic). Agent prompt files live as plain Markdown so users can edit roles without touching code.

## 6. Step-by-step phases & journal

Implementation is split into **13 phases** (0 through 13; Phase 13 is the v2 PR-comment-bus and is **required**, not optional — see Q4 resolution). Each phase ends with a journal entry in `JOURNAL.md` at repo root and a green CI run (`cargo check && cargo fmt && cargo clippy -D warnings && cargo test`).

`JOURNAL.md` schema (one entry per phase per attempt):

```markdown
## Phase N — <title>
- **Started:** YYYY-MM-DD
- **Branch:** feat/agentic-ui
- **Status:** in-progress | done | blocked
- **Acceptance evidence:** <links to commits, screenshots, test output>
- **Notes:** <decisions, surprises, open follow-ups>
```

| # | Phase | Done when |
|---|---|---|
| 0 | **Branch + scaffolding.** Cut `feat/agentic-ui`. Create `.omc/plans/agentic-ui-v1.md` (this file). Add `JOURNAL.md`. Add empty `agents/` prompt dir + 5 placeholder role files. | Branch pushed, plan + journal merged into branch, CI green. |
| 1 | **Members pane skeleton (no agents yet).** Split left column 50/50. Render hardcoded role list. New keybind `Ctrl+a m`. No PTY behind any of them yet. | Pane visible, navigable, swap focus works, tests pass. |
| 2 | **Generalize Surface enum.** Refactor `App.terminals` keying from `PathBuf` → `Surface`. Migrate every call site. No behavior change. | All existing flows still work; tests cover both `Workspace` and a stub `Member` surface. |
| 3 | **Singleton agent PTYs.** Spawn PM/Orchestrator/Architect/Recon PTYs at startup with placeholder echo commands (e.g. `bash` + a banner). Selecting member shows its PTY. | 4 always-on PTYs, switching works, no panics on quit (each shutdown). |
| 4 | **`fellowship-ctl` helper binary.** New `[[bin]]` in `Cargo.toml`. Subcommands: `heartbeat`, `log`, `spawn-engineer`, `release-engineer`, `bead`. Writes JSON files under `~/.fellowship/runtime/<session-uuid>/`. | `fellowship-ctl heartbeat foo --status bar` writes a valid JSON file; `--help` prints all subcommands. Unit tests for arg parsing + file writes. |
| 5 | **Runtime watcher in fellowship.** Watch `runtime/<session>/state/` and `runtime/<session>/spawn-requests/` via `notify`. Update an `AgentRegistry` in `App`. Display state badges in Members pane. | Manually running `fellowship-ctl heartbeat …` from any shell updates the badge live. |
| 6 | **Beads integration module.** `src/beads.rs` — `list_beads`, `create_bead`, `update_bead`, parsed JSON. Polls `bd list --json` every 3s. | Module compiles + tested with a fake `bd` script in `tests/fixtures/`. |
| 7 | **Status / Journal pane.** Render Beads view + Journal view. Toggle key `J`. Filtering. | With seeded fake beads + fake journal lines, pane renders correctly. |
| 8 | **`safe-git` wrapper + PATH injection.** Ship the shim, drop it into `~/.fellowship/bin/`, prepend to env when spawning agent PTYs. Cover `git merge`, `git push --force`, `git push origin (main\|master)`, `gh pr merge`. | Integration test: a shell process with the injected PATH errors on each forbidden cmd; allowed pushes pass through. |
| 9 | **Engineer spawn protocol.** Implement `spawn-engineer` end-to-end: read intent file → allocate id → create worktree → spawn PTY with env + system prompt + injected PATH. Implement pool cap & queue. | `fellowship-ctl spawn-engineer --bead 1 --branch feat/x` from any shell results in a new Engineer member + a new worktree, both visible. |
| 10 | **Role prompts (v1).** Author the 5 markdown role files (`agents/pm.md` etc) with: role description, allowed tools, heartbeat protocol, beads conventions, forbidden commands, success criteria. Wire `cli` + `prompt_path` config and pass `--system-prompt $(cat …)` (or equivalent flag for chosen CLI). | Booting fellowship with a real `claude` CLI produces 4 agents that each respond in-character to a "what is your role?" message. |
| 11 | **Watchdog.** Background task with stale/dead state machine, restart logic, restart counter, escalation banner. Restart records its own bead for traceability. | Killing an agent's PTY externally produces a STALE badge within 60s, a DEAD badge + auto-restart within 180s, and an escalation banner after 3 failed restarts. |
| 12 | **End-to-end smoke.** With agents enabled and a real `bd` install: type a feature request to PM → bead appears → engineer self-claims via `bd update --claim` → engineer spawns (or picks up via existing pool) → PR opens (against fork or test repo) → PR url shows up on the bead in the Status pane. No merge happens (verified by attempting `gh pr merge` from inside the engineer PTY and seeing the shim refuse). | Demo recording committed to `docs/agentic-ui-demo.cast` (asciinema). Journal entry written. |
| 13 | **PR-comment bus (v2 commitment).** Replace fellowship-ctl `pr-comments` polling-by-engineer with a fellowship-side watcher: fellowship polls `gh api .../comments` per open PR linked to a bead; new comments are written as `bd` messages with `--thread` linked to the engineer's bead. Engineer's prompt switches from "run pr-comments when label appears" to "you have new messages on your bead — read and resolve". `pr-comments` subcommand stays as a manual fallback but is no longer the primary path. | Reviewer leaves a comment on an engineer's open PR → within one fellowship poll interval (default 30s), a `bd` message appears threaded on the bead; engineer's heartbeat shows it picking the message up; engineer pushes a fix and the message thread records resolution. Journal closed. |

Each phase = one or more commits + a `JOURNAL.md` update. **No phase is "complete" until its journal entry is written and CI is green.**

## 7. File map

```
src/
  app.rs                  (modified — Surface enum, AgentRegistry)
  ui.rs                   (modified — Members pane, Status/Journal pane)
  panes/
    workspaces.rs         (modified — now bottom half)
    members.rs            (NEW)
    status.rs             (NEW — beads + journal)
    terminal.rs           (modified — accept Surface key, env injection)
  agents/
    mod.rs                (NEW)
    registry.rs           (NEW — agent state machine)
    runtime.rs            (NEW — notify watcher on ~/.fellowship/runtime)
    spawn.rs              (NEW — worktree + PTY spawn)
    watchdog.rs           (NEW)
    prompts.rs            (NEW — load role markdown)
  beads.rs                (NEW)
  bin/
    fellowship-ctl.rs     (NEW)
  guard/
    safe_git.rs           (NEW — wrapper binary source; built as separate [[bin]])
agents/                   (NEW — default role prompts shipped with binary)
  pm.md
  orchestrator.md
  architect.md
  recon.md
  engineer.md
JOURNAL.md                (NEW)
```

## 8. Acceptance criteria (overall)

The plan is "done" when **all** of the following hold on `feat/agentic-ui`:

1. `cargo check && cargo fmt --all -- --check && cargo clippy --all-targets -- -D warnings && cargo test --all-targets` pass on CI for every phase commit.
2. Members pane renders 4 singleton roles + N engineers; navigation works under existing keymap conventions.
3. `fellowship-ctl` binary builds and ships in the same release artifact as `fellowship`.
4. Spawning an engineer from a non-fellowship shell (`fellowship-ctl spawn-engineer …`) produces a new worktree + a new visible engineer member within 5s.
5. Killing an agent PTY externally is detected within `heartbeat_dead_sec` and triggers exactly one restart attempt (verified via journal).
6. The `safe-git` shim refuses every forbidden command listed in §3.7 with a non-zero exit and an explanatory stderr message.
7. The end-to-end smoke (Phase 12) succeeds against a real test repo and is captured in `docs/agentic-ui-demo.cast`.
8. `JOURNAL.md` has 14 entries (phases 0–13), each with a non-empty `Acceptance evidence` field.
9. Reviewer-comment on an engineer PR appears as a threaded `bd` message on the engineer's bead within 30s, and the engineer's heartbeat reflects pickup (Phase 13 acceptance).

## 9. Risks & mitigations

| Risk | Mitigation |
|---|---|
| Long-running `claude` PTYs hit context limits and exit | Watchdog restarts; agent system prompt instructs use of beads as durable memory; worktrees survive restarts |
| Cost runaway from polling loops | Configurable poll intervals; default 30s for agents, 3s for fellowship's `bd list`; warn in status bar if engineer count × poll rate × est-tokens exceeds a threshold |
| `safe-git` bypass via direct binary path (`/usr/bin/git`) | Document limitation; recommend GitHub branch protection as the load-bearing layer; consider Linux seccomp only as future work, **not** in this plan |
| Concurrent worktree path collisions | Existing `worktree_path_for` already namespaces by `<owner>/<repo>/<branch>`; engineer branch names must include bead id (`feat/bead-<n>`) to guarantee uniqueness |
| Agents step on each other's beads (race when claiming) | Beads `bd update --if-status=open` semantics (verify upstream supports CAS; if not, Orchestrator is the only assigner — engineers never self-claim) |
| User confusion about who they're talking to | Status bar shows `[MEMBER: pm]` when a member surface is focused; distinct border color per role |
| `bd` not installed | Fellowship boots with agents disabled if `bd --version` fails; status bar shows `[agents off: bd missing]` |

## 10. Resolved decisions

All five open questions resolved on 2026-05-06.

| # | Question | Decision | Rationale |
|---|---|---|---|
| Q1 | Status pane placement | Tab toggle inside right column. `Ctrl+a g` Git, `Ctrl+a s` Status. | Preserves 3-column layout; agents view is bursty so swapping in place is fine. |
| Q2 | Beads claim race-safety | Engineers self-claim via `bd update <id> --claim` (verified atomic). Engineers poll `bd ready --label role:engineer`. | Decentralizes; removes Orchestrator round-trip per task; race-recovery code unneeded. |
| Q3 | Default CLI per role | `claude --dangerously-skip-permissions` for all five. Per-role `model` field, defaults Recon to `claude-haiku-4-5`. | Single install/auth/quirks profile; cost optimization opt-in via config. |
| Q4 | PR-comment ingestion | v1: `fellowship-ctl pr-comments` (Phase 9 area). v2: PR-comments → `bd` messages bus (**Phase 13, mandatory**). | v1 ships fast; v2 is the right end state and is committed in scope. |
| Q5 | Initial focus on boot | PM member when `agents.enabled = true`; repo-root terminal otherwise. | Opt-in is the agents flag itself; no extra session-state file needed. |

## 11. Out of scope (deferred)

- In-process LLM calls (Path B). Stays out of this plan.
- Multi-repo orchestration (one fellowship session = one repo).
- Agent-driven merging under any circumstance.
- A web dashboard.
- Authoring beads from the TUI directly (PM creates them via its CLI; fellowship only reads).
- Cost reporting / budget enforcement (tracked, not enforced).

## 12. Verification steps (per phase)

For every phase:

1. `cargo check`
2. `cargo fmt --all -- --check`
3. `cargo clippy --all-targets -- -D warnings`
4. `cargo test --all-targets`
5. Manual smoke step matching the phase's "Done when" column.
6. Journal entry written, committed, pushed.
7. CI green on the resulting commit.

For the final phase (12), additionally:
- Run the smoke scenario against a disposable test repo.
- Record asciicast.
- Tag `agentic-ui-v1.0.0` candidate (do not release; user reviews first).

---

## Changelog
- 2026-05-06 — Draft v1 written.
- 2026-05-06 — Q1–Q5 resolved (see §10). Folded results: tab-toggle status pane (Q1), `bd ready` + `bd update --claim` self-claim model (Q2), per-role `model` config + Recon=haiku (Q3), `fellowship-ctl pr-comments` for v1 + Phase 13 PR-comment bus added as mandatory v2 (Q4), PM default focus when `agents.enabled` (Q5). Phases bumped from 12 to 13. Acceptance criteria bumped to 14 journal entries + new Phase 13 reviewer-comment criterion.
- 2026-05-08 — **Phase 12 architectural change.** Orchestrator was originally specified as a fifth claude PTY (Q3). First demo attempt revealed claude is request-response and never auto-ticks its operational loop, so a freshly-created `role:engineer` bead was never routed. Phase 12 replaces the LLM Orchestrator with a native fellowship-side tokio loop in `src/agents/orchestrator.rs` that polls `bd list --json` every 5s and writes spawn-requests for unclaimed engineer beads. Engineer remains LLM-driven (its first turn does the full claim → implement → PR sequence, which fits within one claude turn). `Role::Orchestrator` is dropped from `SINGLETON_ROLES`; the enum variant + heartbeat-id parser stay for journal continuity. `agents/orchestrator.md` is preserved as deprecated historical doc. Plan §6 row 12 done-criterion is unchanged but the implementation surface widened.
