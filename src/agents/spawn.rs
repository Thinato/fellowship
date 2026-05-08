//! Per-role agent PTY spawn planning.
//!
//! Resolves the command line and environment that go into a member-surface
//! PTY. The role markdown prompts are baked into the binary via `include_str!`
//! so deployments don't need to ship the `agents/` directory alongside the
//! `fellowship` binary.
//!
//! When the `claude` CLI is available on the agent's PATH (resolved at boot
//! by `claude_available_at_boot`), the spawn plan exec's a real claude
//! invocation. When it isn't, the plan falls back to a placeholder banner
//! (Phase 3 shape) so fellowship still boots in environments that don't
//! have claude installed yet — the user can install it later and restart.
//!
//! See `docs/plans/agentic-ui-v1.md` §6 row 10 (Phase 10) and §3.7
//! (claude is invoked with `--dangerously-skip-permissions`; the safe-git
//! shim on PATH is the load-bearing guardrail against forbidden git ops).

use crate::surface::Role;

/// Markdown prompts baked into the binary at compile time. Updates to the
/// `agents/<role>.md` files require a rebuild.
const PM_PROMPT: &str = include_str!("../../agents/pm.md");
const ORCHESTRATOR_PROMPT: &str = include_str!("../../agents/orchestrator.md");
const ARCHITECT_PROMPT: &str = include_str!("../../agents/architect.md");
const RECON_PROMPT: &str = include_str!("../../agents/recon.md");
const ENGINEER_PROMPT: &str = include_str!("../../agents/engineer.md");

/// What to feed `TerminalPane::spawn_with_env` for a given role. The caller
/// appends any extra env it wants (notably `PATH` with the safe-git shim).
#[derive(Debug, Clone)]
pub struct SpawnPlan {
    /// Shell command line that bash exec's. Either a real `claude …`
    /// invocation or a placeholder banner that drops the user into bash.
    pub startup_cmd: String,
    /// Owned env pairs the caller must apply. Always contains `AGENT_PROMPT`
    /// and `AGENT_ID`; may also contain `AGENT_MODEL`.
    pub extra_env: Vec<(String, String)>,
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

/// Build the spawn plan for `role` with member label `agent_id` (e.g.
/// `"pm"`, `"engineer-1"`). Reads the `claude_available` flag (computed at
/// boot) to decide between the real and placeholder spawn shapes.
pub fn plan_for(role: Role, agent_id: &str, claude_available: bool) -> SpawnPlan {
    let prompt = prompt_for(role);
    let model = default_model_for(role);

    let mut extra_env = vec![
        ("AGENT_PROMPT".to_string(), prompt.to_string()),
        ("AGENT_ID".to_string(), agent_id.to_string()),
    ];
    if let Some(m) = model {
        extra_env.push(("AGENT_MODEL".to_string(), m.to_string()));
    }

    let startup_cmd = if claude_available {
        // The prompt is delivered via the AGENT_PROMPT env var to avoid
        // shell-quoting hell on multi-line markdown. Same for the model.
        // exec replaces bash so the PTY is owned by claude directly; when
        // claude exits, the PTY closes (Phase 11 watchdog will restart).
        if model.is_some() {
            r#"exec claude --dangerously-skip-permissions --model "$AGENT_MODEL" --append-system-prompt "$AGENT_PROMPT""#.to_string()
        } else {
            r#"exec claude --dangerously-skip-permissions --append-system-prompt "$AGENT_PROMPT""#
                .to_string()
        }
    } else {
        format!(
            "echo '[{agent_id}] claude not on PATH — install it and restart fellowship'; \
             echo '(role prompt is in $AGENT_PROMPT)'; exec bash"
        )
    };

    SpawnPlan {
        startup_cmd,
        extra_env,
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
    fn plan_with_claude_available_emits_real_invocation() {
        let plan = plan_for(Role::Pm, "pm", true);
        assert!(plan.startup_cmd.starts_with("exec claude"));
        assert!(plan.startup_cmd.contains("--dangerously-skip-permissions"));
        assert!(plan.startup_cmd.contains("--append-system-prompt"));
        assert!(
            !plan.startup_cmd.contains("--model"),
            "PM should not pass --model by default"
        );

        let env_keys: Vec<&str> = plan.extra_env.iter().map(|(k, _)| k.as_str()).collect();
        assert!(env_keys.contains(&"AGENT_PROMPT"));
        assert!(env_keys.contains(&"AGENT_ID"));
        assert!(!env_keys.contains(&"AGENT_MODEL"));

        let prompt = plan
            .extra_env
            .iter()
            .find(|(k, _)| k == "AGENT_PROMPT")
            .unwrap();
        assert!(prompt.1.contains("Product Manager"));
    }

    #[test]
    fn plan_for_recon_passes_haiku_model() {
        let plan = plan_for(Role::Recon, "recon", true);
        assert!(plan.startup_cmd.contains("--model"));
        let env: std::collections::HashMap<_, _> = plan
            .extra_env
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        assert_eq!(env.get("AGENT_MODEL"), Some(&"claude-haiku-4-5"));
    }

    #[test]
    fn plan_with_claude_missing_uses_placeholder_banner() {
        let plan = plan_for(Role::Engineer, "engineer-1", false);
        assert!(plan.startup_cmd.contains("claude not on PATH"));
        assert!(plan.startup_cmd.contains("exec bash"));
        assert!(!plan.startup_cmd.contains("exec claude"));
        // AGENT_PROMPT is still set so the user can `echo "$AGENT_PROMPT"`
        // to read what the prompt would have been.
        let env_keys: Vec<&str> = plan.extra_env.iter().map(|(k, _)| k.as_str()).collect();
        assert!(env_keys.contains(&"AGENT_PROMPT"));
    }

    #[test]
    fn plan_for_engineer_includes_agent_id() {
        let plan = plan_for(Role::Engineer, "engineer-7", true);
        let env: std::collections::HashMap<_, _> = plan
            .extra_env
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        assert_eq!(env.get("AGENT_ID"), Some(&"engineer-7"));
    }
}
