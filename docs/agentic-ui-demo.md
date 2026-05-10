# Phase 12 — End-to-end smoke playbook

This is the deterministic recipe for the Phase 12 demo recording. Follow it
keystroke-by-keystroke while `asciinema rec` is running so the resulting
`docs/agentic-ui-demo.cast` exercises the full PM → bead → engineer → PR loop
plus the safe-git shim refusal.

The demo target is **this fellowship repo itself**, on a disposable branch.
The PR opened during the demo is **closed without merging** at the end.

---

## Pre-flight

Run these once before recording. They must all succeed.

```bash
# 1. Repo state — clean, on the agentic-ui branch.
cd ~/repos/thinato/fellowship
git status                                # working tree clean
git rev-parse --abbrev-ref HEAD           # feat/agentic-ui

# 2. Tooling.
which bd claude gh asciinema              # all 4 must resolve
bd --version
claude --version
gh --version
asciinema --version

# 3. GH auth (so the engineer can `gh pr create`).
gh auth status                            # logged in to github.com

# 4. Fellowship binary fresh.
cargo build --release                     # builds fellowship + fellowship-ctl + safe-git

# 5. Clean any leftover demo artifacts (idempotent).
git branch -D smoke/agentic-ui-demo 2>/dev/null || true
git push origin --delete smoke/agentic-ui-demo 2>/dev/null || true
rm -f docs/agentic-ui-demo-evidence.md
```

If any pre-flight step fails, fix it before starting the recording.

---

## Recording

Start asciinema recording the demo:

```bash
asciinema rec docs/agentic-ui-demo.cast \
  --title "fellowship agentic-ui v1 — Phase 12 e2e" \
  --idle-time-limit 3
```

`--idle-time-limit 3` collapses any idle gap longer than 3s to 3s during
playback so the watcher isn't waiting through real claude-latency dead time.

When the recording shell starts:

```bash
cd ~/repos/thinato/fellowship
./target/release/fellowship
```

Fellowship boots into the PM member surface (claude is available).

---

## Segment 1 — PM creates a bead (≈ 60s)

You are now talking to the PM agent in the middle pane.

Type **exactly** this user message at the PM prompt and press Enter:

> Track this as a bead for the engineer pool, please. Title: "Phase 12
> demo evidence file". Description: "Add a single new file
> `docs/agentic-ui-demo-evidence.md` whose only contents are the line
> `Phase 12 demo evidence — generated 2026-05-08`. No code changes, no
> dependencies. Use the `feat/bead-<id>` convention for the branch."
> Priority p2. Labels: role:engineer, kind:implementation, priority:p2.
> Reply with the bead id once created.

**Expected PM behavior:** within ~30s, the PM runs
`fellowship-ctl bead -- create …` and replies with the new bead id (e.g.
`fellowship-abc`).

**Verify in fellowship:** press `Ctrl+a s` to focus the Status pane → the new
bead is visible in the OPEN column.

---

## Segment 2 — Native Orchestrator routes (≈ 5–15s)

Phase 12 replaced the LLM Orchestrator with a deterministic native loop in
`src/agents/orchestrator.rs`. There is no Orchestrator member surface to
focus — the loop runs as a tokio task in fellowship's process.

The loop polls `bd list --json` every 5s. Within one tick of the PM's bead
landing on the board it should:
- Pick up the new `role:engineer` open bead.
- Write a `SpawnRequest` to `~/.fellowship/runtime/<session>/spawn-requests/`
  with `branch: feat/bead-<id>`.
- Fellowship's runtime watcher consumes the request and spawns
  `engineer-1` PTY into a fresh worktree.

**Verify in fellowship:**
- Members pane gains `engineer-1` row with `[WORK]` badge within ~10s.
- Workspaces pane gains a new `feat/bead-<id>` worktree.
- (Optional) `tail -F ~/.fellowship/runtime/<session>/fellowship.log` shows
  `orchestrator: spawn-request enqueued bead=<id>`.

If no engineer appears within 60s, the orchestrator likely couldn't read
`bd list --json` (e.g. bd not initialized in the repo). Check
`fellowship.log` for `orchestrator: bd list failed` and abort the recording.

---

## Segment 3 — Engineer implements + opens PR (≈ 3 min)

Switch to engineer-1: `Ctrl+a m`, navigate to `engineer-1`, `Enter`.

The Engineer's autonomous loop:
1. `bd ready --label role:engineer --json` → finds the bead.
2. `bd update <id> --claim` → atomic claim.
3. Reads the bead description.
4. `echo "Phase 12 demo evidence — generated 2026-05-08" > docs/agentic-ui-demo-evidence.md`
5. `git add docs/agentic-ui-demo-evidence.md && git commit -m "docs: phase 12 demo evidence"`
6. `git push -u origin feat/bead-<id>`.
7. `gh pr create --title "docs: phase 12 demo evidence" --body "Closes <bead>." --base master`
8. `bd note <id> "PR opened: <url>"` (or equivalent).

**Verify in Status pane:** the bead now shows the PR URL in its notes.
**Verify in GitHub:** `gh pr list --head feat/bead-<id>` shows the open PR.

---

## Segment 4 — Shim refusal (≈ 30s)

Stay focused on the engineer-1 terminal.

Type **exactly** this user message:

> Verify the safe-git shim refuses merge attempts. Run these three commands
> one at a time and paste each output:
>
> 1. `gh pr merge --squash <pr-number>`
> 2. `git push --force-with-lease`
> 3. `git push origin master`

**Expected:** all three exit non-zero with stderr matching the shim's exact
refuse messages from `src/guard.rs`:
- `gh pr merge` → ``agents must not run `gh pr merge` — humans review and merge PRs.``
- `git push --force-with-lease` → `agents must not force-push — pushes are append-only.`
- `git push origin master` → ``agents must not push directly to `master` — open a PR from a feature branch.``

The engineer should paste the three error blocks into the terminal so they
appear in the asciicast frame.

---

## Stop recording

In the recording shell:
1. `Ctrl+a q` to quit fellowship cleanly (each PTY shuts down; engineer
   worktree stays on disk).
2. `exit` to leave the asciinema-tracked shell — recording auto-saves to
   `docs/agentic-ui-demo.cast`.

---

## Post-record cleanup

```bash
# 1. Close the demo PR without merging.
PR=$(gh pr list --head feat/bead-<id> --json number -q '.[0].number')
gh pr close "$PR" --comment "Phase 12 demo PR — closed unmerged per agentic-ui-v1 acceptance."

# 2. Delete the demo branches (local + remote).
git branch -D feat/bead-<id>
git push origin --delete feat/bead-<id>

# 3. Delete the engineer worktree (frees the disk path).
git worktree remove $HOME/.fellowship/worktrees/Thinato/fellowship/feat/bead-<id> 2>/dev/null || true

# 4. Close the demo bead.
bd close <bead-id> --reason="Phase 12 demo run captured in docs/agentic-ui-demo.cast"

# 5. Verify the asciicast.
asciinema play docs/agentic-ui-demo.cast
```

Replace `<id>` and `<bead-id>` with the values you saw during the run.

---

## Acceptance checklist (Phase 12)

- [ ] Pre-flight all green.
- [ ] Bead appears in Status pane within 60s of PM message.
- [ ] Engineer member spawns automatically (not via manual ctl).
- [ ] PR URL appears on the bead within 5 minutes of bead creation.
- [ ] All three shim refusal commands exit non-zero with shim stderr.
- [ ] `docs/agentic-ui-demo.cast` exists and replays correctly.
- [ ] Demo PR is **closed**, not merged.
- [ ] Phase 12 entry written in `JOURNAL.md` with cast path + bead id.

If any segment goes off-script (claude latency, prompt drift, shim bug),
**stop the recording**, fix the underlying issue (file a follow-up bead if
it's a fellowship code change), and re-record from the top. The asciicast
must capture a clean run.
