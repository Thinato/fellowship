//! Per-role agent PTY spawn planning.
//!
//! Resolves the program (or shell-banner) and environment that go into a
//! member-surface PTY. The role markdown prompts are baked into the binary
//! via `include_str!` so deployments don't need to ship the `agents/`
//! directory alongside the `fellowship` binary.
//!
//! When the `claude` CLI is available on the agent's PATH (resolved at boot
//! by `claude_available_at_boot`), `plan_for` returns a [`SpawnAction::Program`]
//! that exec's `claude` directly with the prompt as a real argv element — no
//! shell wrapper, no quoting hazards. When claude isn't available it returns
//! a [`SpawnAction::Banner`] that drops the user into a shell with a hint.
//!
//! See `docs/plans/agentic-ui-v1.md` §6 row 10 (Phase 10) and §3.7
//! (claude is invoked with `--dangerously-skip-permissions`; the safe-git
//! shim on PATH is the load-bearing guardrail against forbidden git ops).

use std::path::Path;

use anyhow::Result;
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

/// What `App` should do to spawn this member's PTY.
#[derive(Debug, Clone)]
pub enum SpawnAction {
    /// Launch a binary directly in the PTY (no shell wrapper).
    Program {
        program: String,
        args: Vec<String>,
        env: Vec<(String, String)>,
    },
    /// Launch the user's $SHELL with a one-shot startup command. Used for
    /// the missing-claude path and any future fallbacks that genuinely need
    /// a shell.
    Banner {
        startup_cmd: String,
        env: Vec<(String, String)>,
    },
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

/// Build the spawn action for `role` with member label `agent_id`. The
/// `claude_available` flag (computed once at boot via
/// [`claude_available_at_boot`]) selects the real-vs-banner shape.
pub fn plan_for(role: Role, agent_id: &str, claude_available: bool) -> SpawnAction {
    let prompt = prompt_for(role);
    let model = default_model_for(role);

    // AGENT_PROMPT and AGENT_ID are exposed via env for diagnostics
    // (`echo $AGENT_PROMPT` from another shell connected to the PTY) and
    // for any agent tooling that wants to re-read the role prompt without
    // command-line parsing.
    let mut env = vec![
        ("AGENT_PROMPT".to_string(), prompt.to_string()),
        ("AGENT_ID".to_string(), agent_id.to_string()),
    ];
    if let Some(m) = model {
        env.push(("AGENT_MODEL".to_string(), m.to_string()));
    }

    if claude_available {
        let mut args: Vec<String> = vec![
            "--dangerously-skip-permissions".to_string(),
            "--append-system-prompt".to_string(),
            prompt.to_string(),
        ];
        if let Some(m) = model {
            args.push("--model".to_string());
            args.push(m.to_string());
        }
        SpawnAction::Program {
            program: "claude".to_string(),
            args,
            env,
        }
    } else {
        let startup_cmd = format!(
            "echo '[{agent_id}] claude not on PATH — install it and restart fellowship'; \
             echo '(role prompt is in $AGENT_PROMPT)'; exec bash"
        );
        SpawnAction::Banner { startup_cmd, env }
    }
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

/// Spawn a member-surface PTY by dispatching the [`SpawnAction`].
///
/// `extra_env` is merged on top of the action's own env (the action's keys
/// take precedence on collision; the caller's `extra_env` is the base).
pub fn execute(
    action: &SpawnAction,
    rows: u16,
    cols: u16,
    cwd: &Path,
    tx: UnboundedSender<Event>,
    base_env: &[(String, String)],
) -> Result<TerminalPane> {
    let merged_env = merge_env(base_env, action_env(action));
    let env_refs: Vec<(&str, &str)> = merged_env
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    match action {
        SpawnAction::Program { program, args, .. } => {
            let arg_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
            TerminalPane::spawn_program_with_env(rows, cols, cwd, tx, program, &arg_refs, &env_refs)
        }
        SpawnAction::Banner { startup_cmd, .. } => {
            TerminalPane::spawn_with_env(rows, cols, cwd, tx, Some(startup_cmd), &env_refs)
        }
    }
}

fn action_env(action: &SpawnAction) -> &[(String, String)] {
    match action {
        SpawnAction::Program { env, .. } => env,
        SpawnAction::Banner { env, .. } => env,
    }
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

#[cfg(test)]
mod tests {
    use super::*;

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
    fn plan_with_claude_available_emits_program_action() {
        let action = plan_for(Role::Pm, "pm", true);
        match action {
            SpawnAction::Program { program, args, env } => {
                assert_eq!(program, "claude");
                assert!(args.iter().any(|a| a == "--dangerously-skip-permissions"));
                assert!(args.iter().any(|a| a == "--append-system-prompt"));
                assert!(args.iter().any(|a| a.contains("Product Manager")));
                assert!(!args.iter().any(|a| a == "--model"));
                let env_keys: Vec<&str> = env.iter().map(|(k, _)| k.as_str()).collect();
                assert!(env_keys.contains(&"AGENT_PROMPT"));
                assert!(env_keys.contains(&"AGENT_ID"));
                assert!(!env_keys.contains(&"AGENT_MODEL"));
            }
            other => panic!("expected Program action, got {other:?}"),
        }
    }

    #[test]
    fn plan_for_recon_passes_haiku_model_in_argv_and_env() {
        let action = plan_for(Role::Recon, "recon", true);
        match action {
            SpawnAction::Program { args, env, .. } => {
                let model_idx = args
                    .iter()
                    .position(|a| a == "--model")
                    .expect("--model arg missing");
                assert_eq!(args[model_idx + 1], "claude-haiku-4-5");
                let env_map: std::collections::HashMap<_, _> =
                    env.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
                assert_eq!(env_map.get("AGENT_MODEL"), Some(&"claude-haiku-4-5"));
            }
            other => panic!("expected Program action, got {other:?}"),
        }
    }

    #[test]
    fn plan_with_claude_missing_uses_banner_action() {
        let action = plan_for(Role::Engineer, "engineer-1", false);
        match action {
            SpawnAction::Banner { startup_cmd, env } => {
                assert!(startup_cmd.contains("claude not on PATH"));
                assert!(startup_cmd.contains("exec bash"));
                let env_keys: Vec<&str> = env.iter().map(|(k, _)| k.as_str()).collect();
                assert!(env_keys.contains(&"AGENT_PROMPT"));
            }
            other => panic!("expected Banner action, got {other:?}"),
        }
    }

    #[test]
    fn plan_for_engineer_includes_agent_id_in_env() {
        let action = plan_for(Role::Engineer, "engineer-7", true);
        let env: std::collections::HashMap<_, _> = action_env(&action)
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
}
