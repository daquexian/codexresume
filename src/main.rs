mod config;
mod data;
mod session_index;
mod ui;

use std::env;
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Command;

use anyhow::Context;
use config::PathOverrides;
use config::resolve_paths;
use data::ProviderVisibility;
use data::open_session_db;

struct CliArgs {
    forwarded_args: Vec<OsString>,
    provider_visibility: ProviderVisibility,
    path_overrides: PathOverrides,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _ = color_eyre::install();

    let cli_args = parse_args(env::args_os().skip(1).collect())?;
    let paths = resolve_paths(&cli_args.path_overrides).context("failed to resolve Codex paths")?;
    let session_db = open_session_db(&paths, cli_args.provider_visibility).await?;
    let selected = ui::run_picker(session_db, cli_args.provider_visibility)
        .await
        .context("failed to run picker UI")?;

    let Some(selected) = selected else {
        return Ok(());
    };

    exec_codex_resume(selected.thread_id, cli_args.forwarded_args)
}

fn parse_args(args: Vec<OsString>) -> anyhow::Result<CliArgs> {
    if args.len() == 1 {
        match args[0].to_str() {
            Some("-h") | Some("--help") => {
                print_help();
                std::process::exit(0);
            }
            Some("-V") | Some("--version") => {
                println!("codexresume {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            _ => {}
        }
    }

    let mut forwarded_args = Vec::with_capacity(args.len());
    let mut provider_visibility = ProviderVisibility::All;
    let mut path_overrides = PathOverrides::default();
    let mut index = 0usize;

    while index < args.len() {
        let arg = args[index].clone();

        if matches!(
            arg.to_str(),
            Some("--last" | "--all" | "--include-non-interactive")
        ) {
            anyhow::bail!(
                "`{}` is managed by codexresume itself; remove it and pass only resume runtime args",
                arg.to_string_lossy()
            );
        }

        match arg.to_str() {
            Some("--only-openai") => {
                provider_visibility = ProviderVisibility::OnlyOpenAi;
            }
            Some("--codex-home") => {
                index += 1;
                path_overrides.codex_home =
                    Some(parse_path_value(args.get(index), "--codex-home")?);
            }
            Some("--sqlite-home") => {
                index += 1;
                path_overrides.sqlite_home =
                    Some(parse_path_value(args.get(index), "--sqlite-home")?);
            }
            Some(value) if value.starts_with("--codex-home=") => {
                let path = value.trim_start_matches("--codex-home=");
                path_overrides.codex_home = Some(PathBuf::from(path));
            }
            Some(value) if value.starts_with("--sqlite-home=") => {
                let path = value.trim_start_matches("--sqlite-home=");
                path_overrides.sqlite_home = Some(PathBuf::from(path));
            }
            _ => forwarded_args.push(arg),
        }

        index += 1;
    }

    Ok(CliArgs {
        forwarded_args,
        provider_visibility,
        path_overrides,
    })
}

fn parse_path_value(value: Option<&OsString>, flag: &str) -> anyhow::Result<PathBuf> {
    let value = value.with_context(|| format!("{flag} requires a path value"))?;
    Ok(PathBuf::from(value))
}

fn print_help() {
    println!(
        "codexresume\n\n\
Pick from local Codex sessions, ignoring source/cwd filters.\n\
Use `--only-openai` to only show sessions whose provider id is `openai`.\n\
The picker loads the first page quickly and loads older sessions as you scroll.\n\
\n\
Wrapper flags:\n\
  --only-openai           only show sessions with provider id `openai`\n\
  --codex-home PATH       override CODEX_HOME / ~/.codex discovery\n\
  --sqlite-home PATH      override CODEX_SQLITE_HOME / config discovery\n\
\n\
Then exec:\n\
  codex resume <SESSION_ID> <forwarded args>\n\n\
Examples:\n\
  codexresume --yolo\n\
  codexresume --only-openai --yolo\n\
  codexresume --codex-home ~/.codex --sqlite-home ~/.codex --yolo\n\
  codexresume -C /path/to/project --model gpt-5.4\n\
  codexresume --dangerously-bypass-approvals-and-sandbox \"continue from here\"\n"
    );
}

fn exec_codex_resume(session_id: String, forwarded_args: Vec<OsString>) -> anyhow::Result<()> {
    let mut command = Command::new("codex");
    command.arg("resume").arg(session_id).args(forwarded_args);

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let err = command.exec();
        return Err(anyhow::Error::new(err)).context("failed to exec `codex resume`");
    }

    #[cfg(not(unix))]
    {
        let status = command.status().context("failed to spawn `codex resume`")?;
        std::process::exit(status.code().unwrap_or(1));
    }
}
