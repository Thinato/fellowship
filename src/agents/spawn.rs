//! Per-role agent PTY spawn planning.
//!
//! Resolves the shell command line and environment that go into a
//! member-surface PTY. Role markdown prompts are baked into the binary via
//! `include_str!` and written to `<runtime>/prompts/<role>.md` at fellowship
//! boot so claude can read them with `--append-system-prompt-file`. This
//! avoids ever embedding the multi-line markdown into argv or stdin (which
//! is brittle under shell quoting and PTY init-time write races).
//!
//! Every member surface boots through the user's `$SHELL` so that:
//! - the agent CLI runs first (claude or a "claude not installed" banner),
//! - and when it exits, the PTY drops back into an interactive shell session
//!   instead of closing — the user can still inspect the worktree, run
//!   commands, etc., until they explicitly type `exit`.
//!
//! See `docs/plans/agentic-ui-v1.md` §6 row 10 (Phase 10) and §3.7
//! (claude is invoked with `--dangerously-skip-permissions`; the safe-git
//! shim on PATH is the load-bearing guardrail against forbidden git ops).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tokio::sync::mpsc::UnboundedSender;

use crate::event::Event;
use crate::panes::terminal::TerminalPane;
use crate::surface::Role;

/// Markdown prompts baked into the binary at compile time. Updates to the
/// `agents/<role>.md` files require a rebuild.
const PM_PROMPT: &str = include_str!("../../agents/pm.md");
const ORCHESTRATOR_PROMPT: &str = include_str!("../../agents/orchestrator.md");
const ARCHITECT_PROMPT: &str = include_str!("../../agents/architect.md");
const RECON_PROMPT: &str = include_str!("../../agents/recon.md");
const ENGINEER_PROMPT: &str = include_str!("../../agents/engineer.md");

/// What `App` should hand to the PTY for this member. The contract is now
/// uniform: a single shell command line that the user's `$SHELL` runs via
/// `-c`. The command line itself is responsible for handing control back to
/// an interactive shell after the agent exits.
#[derive(Debug, Clone)]
pub struct SpawnAction {
    pub command_line: String,
    pub env: Vec<(String, String)>,
}

pub fn prompt_for(role: Role) -> &'static str {
    match role {
        Role::Pm => PM_PROMPT,
        Role::Orchestrator => ORCHESTRATOR_PROMPT,
        Role::Architect => ARCHITECT_PROMPT,
        Role::Recon => RECON_PROMPT,
        Role::Engineer => ENGINEER_PROMPT,
    }
}

/// Default model per role. Recon uses Haiku; everyone else uses claude's
/// CLI default. Per Q3 resolution in `docs/plans/agentic-ui-v1.md` §10.
pub fn default_model_for(role: Role) -> Option<&'static str> {
    match role {
        Role::Recon => Some("claude-haiku-4-5"),
        _ => None,
    }
}

/// Stable on-disk path for a role's prompt file under `prompts_root`.
pub fn prompt_path_for(prompts_root: &Path, role: Role) -> PathBuf {
    let name = match role {
        Role::Pm => "pm.md",
        Role::Orchestrator => "orchestrator.md",
        Role::Architect => "architect.md",
        Role::Recon => "recon.md",
        Role::Engineer => "engineer.md",
    };
    prompts_root.join(name)
}

/// Write all five role prompts to `<prompts_root>/<role>.md`. Idempotent —
/// safe to call on every boot.
pub fn write_prompt_files(prompts_root: &Path) -> Result<()> {
    std::fs::create_dir_all(prompts_root)
        .with_context(|| format!("mkdir -p {}", prompts_root.display()))?;
    for (role, body) in [
        (Role::Pm, PM_PROMPT),
        (Role::Orchestrator, ORCHESTRATOR_PROMPT),
        (Role::Architect, ARCHITECT_PROMPT),
        (Role::Recon, RECON_PROMPT),
        (Role::Engineer, ENGINEER_PROMPT),
    ] {
        let path = prompt_path_for(prompts_root, role);
        std::fs::write(&path, body).with_context(|| format!("write {}", path.display()))?;
    }
    Ok(())
}

/// Build the spawn action for `role` with member label `agent_id`. The
/// `claude_available` flag (computed once at boot via
/// [`claude_available_at_boot`]) selects the real-vs-banner shape. The
/// `prompt_path` is the on-disk role prompt previously written by
/// [`write_prompt_files`].
pub fn plan_for(
    role: Role,
    agent_id: &str,
    claude_available: bool,
    prompt_path: &Path,
) -> SpawnAction {
    let model = default_model_for(role);

    let mut env = vec![
        ("AGENT_ID".to_string(), agent_id.to_string()),
        (
            "AGENT_PROMPT_FILE".to_string(),
            prompt_path.display().to_string(),
        ),
    ];
    if let Some(m) = model {
        env.push(("AGENT_MODEL".to_string(), m.to_string()));
    }

    // Single-quote the path so it survives any whitespace or special chars
    // (paths under `~/.fellowship/` won't contain single quotes themselves).
    let prompt_path_quoted = shell_single_quote(&prompt_path.display().to_string());
    let leading = if claude_available {
        if let Some(m) = model {
            format!(
                "claude --dangerously-skip-permissions --model {m} --append-system-prompt-file {prompt_path_quoted}"
            )
        } else {
            format!(
                "claude --dangerously-skip-permissions --append-system-prompt-file {prompt_path_quoted}"
            )
        }
    } else {
        format!(
            "echo '[{agent_id}] claude not on PATH — install it and restart fellowship'; \
             echo '(role prompt file is at {})'",
            prompt_path.display()
        )
    };

    // After the agent (or banner) exits, replace this short-lived shell with
    // a fresh interactive `$SHELL` so the PTY stays usable for the human.
    // Using `${SHELL:-/bin/bash}` keeps it portable when `$SHELL` is unset.
    let command_line = format!("{leading}; exec ${{SHELL:-/bin/bash}} -i");

    SpawnAction { command_line, env }
}

/// Probe whether the `claude` CLI is invokable. Cheap (one fork+exec); call
/// once at fellowship boot, log the result, and pass the bool around.
pub fn claude_available_at_boot() -> bool {
    std::process::Command::new("claude")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Spawn a member-surface PTY by running the action's `command_line` under
/// the user's `$SHELL`.
///
/// `base_env` (typically `[(PATH, agent_path)]` etc.) is merged with the
/// action's env; action keys take precedence on collision.
pub fn execute(
    action: &SpawnAction,
    rows: u16,
    cols: u16,
    cwd: &Path,
    tx: UnboundedSender<Event>,
    base_env: &[(String, String)],
) -> Result<TerminalPane> {
    let merged_env = merge_env(base_env, &action.env);
    let env_refs: Vec<(&str, &str)> = merged_env
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string());
    TerminalPane::spawn_program_with_env(
        rows,
        cols,
        cwd,
        tx,
        &shell,
        &["-c", action.command_line.as_str()],
        &env_refs,
    )
}

fn merge_env(base: &[(String, String)], overrides: &[(String, String)]) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = base.to_vec();
    for (k, v) in overrides {
        if let Some(existing) = out.iter_mut().find(|(ek, _)| ek == k) {
            existing.1 = v.clone();
        } else {
            out.push((k.clone(), v.clone()));
        }
    }
    out
}

/// POSIX-shell-safe single-quote escape. Wrap `s` in single quotes; escape
/// any single quotes in `s` by closing the quote, inserting `\'`, and
/// reopening. Idiomatic for fully-literal argument passing.
fn shell_single_quote(s: &str) -> String {
    let escaped = s.replace('\'', r"'\''");
    format!("'{escaped}'")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn prompt_for_returns_non_empty_for_every_role() {
        for role in [
            Role::Pm,
            Role::Orchestrator,
            Role::Architect,
            Role::Recon,
            Role::Engineer,
        ] {
            let p = prompt_for(role);
            assert!(!p.is_empty(), "{role:?} prompt empty");
            assert!(p.contains("Role:"), "{role:?} prompt missing role header");
        }
    }

    #[test]
    fn default_model_only_set_for_recon() {
        assert_eq!(default_model_for(Role::Pm), None);
        assert_eq!(default_model_for(Role::Orchestrator), None);
        assert_eq!(default_model_for(Role::Architect), None);
        assert_eq!(default_model_for(Role::Engineer), None);
        assert_eq!(default_model_for(Role::Recon), Some("claude-haiku-4-5"));
    }

    #[test]
    fn write_prompt_files_creates_all_five_with_role_headers() {
        let tmp = TempDir::new().unwrap();
        write_prompt_files(tmp.path()).unwrap();
        for (role, expected) in [
            (Role::Pm, "Product Manager"),
            (Role::Orchestrator, "Orchestrator"),
            (Role::Architect, "Architect"),
            (Role::Recon, "Codebase Recon"),
            (Role::Engineer, "Software Engineer"),
        ] {
            let path = prompt_path_for(tmp.path(), role);
            assert!(path.exists(), "{path:?} missing");
            let contents = std::fs::read_to_string(&path).unwrap();
            assert!(contents.contains(expected), "{path:?} missing {expected:?}");
        }
    }

    #[test]
    fn plan_with_claude_available_runs_claude_then_drops_to_interactive_shell() {
        let action = plan_for(
            Role::Pm,
            "pm",
            true,
            Path::new("/tmp/fellowship-test/prompts/pm.md"),
        );
        assert!(
            action
                .command_line
                .starts_with("claude --dangerously-skip-permissions"),
            "got: {}",
            action.command_line
        );
        assert!(
            action
                .command_line
                .contains("--append-system-prompt-file '/tmp/fellowship-test/prompts/pm.md'"),
            "prompt path missing or wrong: {}",
            action.command_line
        );
        assert!(
            !action.command_line.contains("--model"),
            "PM should not pass --model"
        );
        assert!(
            action
                .command_line
                .ends_with("; exec ${SHELL:-/bin/bash} -i"),
            "missing interactive shell drop: {}",
            action.command_line
        );
        let env: std::collections::HashMap<_, _> = action
            .env
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        assert_eq!(env.get("AGENT_ID"), Some(&"pm"));
        assert!(env.contains_key("AGENT_PROMPT_FILE"));
        assert!(!env.contains_key("AGENT_MODEL"));
    }

    #[test]
    fn plan_for_recon_passes_haiku_model_in_command_and_env() {
        let action = plan_for(Role::Recon, "recon", true, Path::new("/tmp/x/recon.md"));
        assert!(action.command_line.contains("--model claude-haiku-4-5"));
        let env: std::collections::HashMap<_, _> = action
            .env
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        assert_eq!(env.get("AGENT_MODEL"), Some(&"claude-haiku-4-5"));
    }

    #[test]
    fn plan_with_claude_missing_emits_banner_and_still_drops_to_shell() {
        let action = plan_for(
            Role::Engineer,
            "engineer-1",
            false,
            Path::new("/tmp/x/engineer.md"),
        );
        assert!(action.command_line.contains("claude not on PATH"));
        assert!(action.command_line.contains("/tmp/x/engineer.md"));
        assert!(
            action
                .command_line
                .ends_with("; exec ${SHELL:-/bin/bash} -i"),
            "missing interactive shell drop: {}",
            action.command_line
        );
    }

    #[test]
    fn plan_for_engineer_includes_agent_id_in_env() {
        let action = plan_for(
            Role::Engineer,
            "engineer-7",
            true,
            Path::new("/tmp/x/engineer.md"),
        );
        let env: std::collections::HashMap<_, _> = action
            .env
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        assert_eq!(env.get("AGENT_ID"), Some(&"engineer-7"));
    }

    #[test]
    fn merge_env_overrides_take_precedence() {
        let base = vec![
            ("PATH".to_string(), "/usr/bin".to_string()),
            ("FOO".to_string(), "base".to_string()),
        ];
        let overrides = vec![
            ("FOO".to_string(), "override".to_string()),
            ("BAR".to_string(), "new".to_string()),
        ];
        let merged = merge_env(&base, &overrides);
        let map: std::collections::HashMap<_, _> = merged
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        assert_eq!(map.get("FOO"), Some(&"override"));
        assert_eq!(map.get("PATH"), Some(&"/usr/bin"));
        assert_eq!(map.get("BAR"), Some(&"new"));
    }

    #[test]
    fn shell_single_quote_handles_apostrophes() {
        assert_eq!(shell_single_quote("plain"), "'plain'");
        assert_eq!(shell_single_quote("don't"), r"'don'\''t'");
        assert_eq!(
            shell_single_quote("/path/with spaces.md"),
            "'/path/with spaces.md'"
        );
    }
}
