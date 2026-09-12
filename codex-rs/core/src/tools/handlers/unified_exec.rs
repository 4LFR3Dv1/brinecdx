use crate::sandboxing::SandboxPermissions;
use crate::shell::Shell;
use crate::shell::ShellType;
use crate::shell::get_shell_by_model_provided_path;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::context::ToolPayload;
use crate::tools::hook_names::HookToolName;
use crate::tools::registry::PostToolUsePayload;
use codex_exec_server::Environment;
use codex_protocol::models::AdditionalPermissionProfile;
use codex_tools::UnifiedExecShellMode;
use serde::Deserialize;
use shlex::split as shlex_split;
use std::path::PathBuf;
use std::sync::Arc;

#[cfg(test)]
use crate::tools::handlers::parse_arguments;

mod exec_command;
mod write_stdin;

pub use exec_command::ExecCommandHandler;
pub(crate) use exec_command::ExecCommandHandlerOptions;
pub use write_stdin::WriteStdinHandler;

const BRINE_EXEC_SERVER_URL_ENV_VAR: &str = "BRINE_EXEC_SERVER_URL";

#[derive(Debug, Deserialize)]
pub(crate) struct ExecCommandArgs {
    pub(crate) cmd: String,
    #[serde(default)]
    shell: Option<String>,
    #[serde(default)]
    login: Option<bool>,
    #[serde(default = "default_tty")]
    tty: bool,
    #[serde(default = "default_exec_yield_time_ms")]
    yield_time_ms: u64,
    #[serde(default)]
    timeout_ms: Option<u64>,
    #[serde(default)]
    max_output_tokens: Option<usize>,
    #[serde(default)]
    sandbox_permissions: Option<SandboxPermissions>,
    #[serde(default)]
    additional_permissions: Option<AdditionalPermissionProfile>,
    #[serde(default)]
    justification: Option<String>,
    #[serde(default)]
    prefix_rule: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct ExecCommandEnvironmentArgs {
    #[serde(default)]
    environment_id: Option<String>,
    // Keep this raw until after environment selection; relative paths must be
    // resolved against the selected environment cwd, not the process cwd.
    #[serde(default)]
    workdir: Option<String>,
}

fn default_exec_yield_time_ms() -> u64 {
    10_000
}

fn default_write_stdin_yield_time_ms() -> u64 {
    250
}

fn default_tty() -> bool {
    false
}

#[derive(Debug)]
pub(crate) struct ResolvedCommand {
    pub(crate) command: Vec<String>,
    pub(crate) shell_type: ShellType,
}

fn post_unified_exec_tool_use_payload(
    invocation: &ToolInvocation,
    result: &dyn ToolOutput,
) -> Option<PostToolUsePayload> {
    let ToolPayload::Function { .. } = &invocation.payload else {
        return None;
    };

    let tool_input = result.post_tool_use_input(&invocation.payload)?;
    let tool_use_id = result.post_tool_use_id(&invocation.call_id);
    let tool_response = result.post_tool_use_response(&tool_use_id, &invocation.payload)?;
    Some(PostToolUsePayload {
        tool_name: HookToolName::bash(),
        tool_use_id,
        tool_input,
        tool_response,
    })
}

fn brine_exec_server_configured() -> bool {
    std::env::var(BRINE_EXEC_SERVER_URL_ENV_VAR)
        .ok()
        .is_some_and(|value| !value.trim().is_empty())
}

fn direct_mode_command(
    args: &ExecCommandArgs,
    session_shell: Arc<Shell>,
    use_login_shell: bool,
    brine_direct_argv: bool,
) -> Result<ResolvedCommand, String> {
    if brine_direct_argv {
        // BRINE_EXEC_SERVER_URL is an explicit BrineCDX opt-in. Preserve the
        // command as structured argv instead of re-wrapping it in the remote
        // shell. Brine remains the authoritative admission boundary and can
        // therefore prove/deny the executable and subcommand without parsing a
        // shell program. Shell metacharacters remain ordinary argv bytes here;
        // they are never evaluated by a shell on the executor.
        let command = shlex_split(&args.cmd).ok_or_else(|| {
            "Brine direct execution requires a command that can be represented as argv".to_string()
        })?;
        if command.is_empty() {
            return Err("Brine direct execution requires a non-empty command".to_string());
        }
        return Ok(ResolvedCommand {
            command,
            shell_type: session_shell.shell_type,
        });
    }

    let model_shell = args
        .shell
        .as_ref()
        .map(|shell_str| get_shell_by_model_provided_path(&PathBuf::from(shell_str)));
    let shell = model_shell.as_ref().unwrap_or(session_shell.as_ref());
    Ok(ResolvedCommand {
        command: shell.derive_exec_args(&args.cmd, use_login_shell),
        shell_type: shell.shell_type,
    })
}

pub(crate) fn get_command(
    args: &ExecCommandArgs,
    session_shell: Arc<Shell>,
    shell_mode: &UnifiedExecShellMode,
    allow_login_shell: bool,
) -> Result<ResolvedCommand, String> {
    let use_login_shell = match args.login {
        Some(true) if !allow_login_shell => {
            return Err(
                "login shell is disabled by config; omit `login` or set it to false.".to_string(),
            );
        }
        Some(use_login_shell) => use_login_shell,
        None => allow_login_shell,
    };

    match shell_mode {
        UnifiedExecShellMode::Direct => direct_mode_command(
            args,
            session_shell,
            use_login_shell,
            brine_exec_server_configured(),
        ),
        UnifiedExecShellMode::ZshFork(zsh_fork_config) => {
            if args.shell.is_some() {
                return Err(
                    "`shell` is not supported for local zsh-fork exec; omit `shell` to use zsh-fork, or target a remote environment where `shell` is supported.".to_string(),
                );
            }

            Ok(ResolvedCommand {
                command: vec![
                    zsh_fork_config.shell_zsh_path.to_string_lossy().to_string(),
                    if use_login_shell { "-lc" } else { "-c" }.to_string(),
                    args.cmd.clone(),
                ],
                shell_type: ShellType::Zsh,
            })
        }
    }
}

pub(crate) fn shell_mode_for_environment(
    turn_shell_mode: &UnifiedExecShellMode,
    environment: &Environment,
) -> UnifiedExecShellMode {
    if environment.is_remote() {
        UnifiedExecShellMode::Direct
    } else {
        turn_shell_mode.clone()
    }
}

#[cfg(test)]
mod brine_direct_argv_tests {
    use super::*;
    use crate::shell::default_user_shell;
    use pretty_assertions::assert_eq;

    #[test]
    fn brine_direct_mode_preserves_git_as_structured_argv() -> anyhow::Result<()> {
        let args: ExecCommandArgs = parse_arguments(r#"{"cmd":"git status --short"}"#)?;
        let resolved = direct_mode_command(
            &args,
            Arc::new(default_user_shell()),
            /*use_login_shell*/ false,
            /*brine_direct_argv*/ true,
        )
        .map_err(anyhow::Error::msg)?;

        assert_eq!(resolved.command, vec!["git", "status", "--short"]);
        Ok(())
    }

    #[test]
    fn brine_direct_mode_never_interprets_shell_operators() -> anyhow::Result<()> {
        let args: ExecCommandArgs =
            parse_arguments(r#"{"cmd":"git status --short && touch should-not-run"}"#)?;
        let resolved = direct_mode_command(
            &args,
            Arc::new(default_user_shell()),
            /*use_login_shell*/ false,
            /*brine_direct_argv*/ true,
        )
        .map_err(anyhow::Error::msg)?;

        assert_eq!(
            resolved.command,
            vec!["git", "status", "--short", "&&", "touch", "should-not-run"]
        );
        Ok(())
    }

    #[test]
    fn brine_direct_mode_fails_closed_on_unbalanced_quotes() -> anyhow::Result<()> {
        let args: ExecCommandArgs = parse_arguments(r#"{"cmd":"git grep 'unterminated"}"#)?;
        let error = direct_mode_command(
            &args,
            Arc::new(default_user_shell()),
            /*use_login_shell*/ false,
            /*brine_direct_argv*/ true,
        )
        .expect_err("unbalanced quotes must not fall back to a shell");

        assert!(error.contains("represented as argv"));
        Ok(())
    }
}

#[cfg(test)]
#[path = "unified_exec_tests.rs"]
mod tests;
