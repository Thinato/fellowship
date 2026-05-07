//! `safe-git` — guardrail shim for `git` and `gh` invoked from agent PTYs.
//!
//! Fellowship symlinks `~/.fellowship/bin/{git,gh}` at this binary and
//! prepends that dir to PATH for member surfaces only. Workspace surfaces
//! (the user's interactive terminal) keep an unmodified PATH.
//!
//! When invoked as `git` or `gh`, the shim parses argv against the policy
//! in [`fellowship::guard`]. Forbidden invocations exit non-zero with a
//! readable stderr message. Allowed invocations strip the shim's directory
//! from PATH and `exec` the real binary.
//!
//! See `docs/plans/agentic-ui-v1.md` §3.7 for the policy.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use fellowship::guard::{Decision, Tool, decide, detect_tool};

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().collect();
    let argv0 = argv.first().cloned().unwrap_or_default();
    let args: Vec<String> = argv.iter().skip(1).cloned().collect();
    let tool = detect_tool(&argv0);

    if matches!(tool, Tool::Other) {
        eprintln!(
            "safe-git: not invoked as `git` or `gh` (argv[0]={argv0:?}). \
             Install via fellowship's symlinks at ~/.fellowship/bin/{{git,gh}}."
        );
        return ExitCode::from(2);
    }

    match decide(tool, &args) {
        Decision::Allow => exec_real(tool, &argv0, &args),
        Decision::Block(reason) => {
            eprintln!("safe-git: blocked — {reason}");
            ExitCode::from(1)
        }
    }
}

fn exec_real(tool: Tool, argv0: &str, args: &[String]) -> ExitCode {
    let basename = match tool {
        Tool::Git => "git",
        Tool::Gh => "gh",
        Tool::Other => unreachable!(),
    };
    let shim_dir: Option<PathBuf> = Path::new(argv0).parent().map(Path::to_path_buf);
    let new_path = sanitized_path(shim_dir.as_deref());

    use std::os::unix::process::CommandExt;
    let mut cmd = std::process::Command::new(basename);
    cmd.args(args);
    if let Some(p) = new_path {
        cmd.env("PATH", p);
    }
    // exec replaces this process; only returns on failure.
    let err = cmd.exec();
    eprintln!(
        "safe-git: failed to exec real `{basename}` from sanitized PATH: {err}. \
         Is `{basename}` installed elsewhere on PATH?"
    );
    ExitCode::from(127)
}

/// Return PATH minus `shim_dir` if it appears, else None to leave env alone.
fn sanitized_path(shim_dir: Option<&Path>) -> Option<String> {
    let shim_dir = shim_dir?;
    let path = std::env::var_os("PATH")?;
    let entries: Vec<PathBuf> = std::env::split_paths(&path).collect();
    let filtered: Vec<PathBuf> = entries.into_iter().filter(|p| p != shim_dir).collect();
    let joined = std::env::join_paths(filtered).ok()?;
    joined.into_string().ok()
}
