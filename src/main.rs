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
use data::SessionDbOptions;
use data::open_session_db;

enum RunMode {
    Passthrough {
        resume_args: Vec<OsString>,
    },
    Picker {
        resume_args: Vec<OsString>,
        options: SessionDbOptions,
    },
    Last {
        resume_args: Vec<OsString>,
        options: SessionDbOptions,
    },
}

struct CliArgs {
    mode: RunMode,
    path_overrides: PathOverrides,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _ = color_eyre::install();

    let cli_args = parse_args(env::args_os().skip(1).collect())?;
    let paths = resolve_paths(&cli_args.path_overrides).context("failed to resolve Codex paths")?;

    match cli_args.mode {
        RunMode::Passthrough { resume_args } => exec_codex_resume_passthrough(resume_args),
        RunMode::Last {
            resume_args,
            options,
        } => {
            let session_db = open_session_db(&paths, options).await?;
            let Some(session_id) = session_db
                .select_last_thread_id()
                .await
                .context("failed to pick --last session id")?
            else {
                anyhow::bail!("no sessions found (try --all or --include-non-interactive)");
            };
            exec_codex_resume(session_id, resume_args)
        }
        RunMode::Picker {
            resume_args,
            options,
        } => {
            let session_db = open_session_db(&paths, options.clone()).await?;
            let selected = ui::run_picker(session_db, options)
                .await
                .context("failed to run picker UI")?;

            let Some(selected) = selected else {
                return Ok(());
            };

            exec_codex_resume(selected.thread_id, resume_args)
        }
    }
}

fn parse_args(args: Vec<OsString>) -> anyhow::Result<CliArgs> {
    for arg in &args {
        match arg.to_str() {
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

    let mut resume_args = Vec::with_capacity(args.len());
    let mut positionals: Vec<OsString> = Vec::new();

    let mut provider_visibility = ProviderVisibility::All;
    let mut path_overrides = PathOverrides::default();
    let mut last = false;
    let mut show_all = false;
    let mut include_non_interactive = false;
    let mut include_archived = false;
    let mut remote = false;
    let mut requested_cwd: Option<PathBuf> = None;
    let mut after_double_dash = false;
    let mut index = 0usize;

    while index < args.len() {
        let arg = args[index].clone();

        if after_double_dash {
            resume_args.push(arg.clone());
            positionals.push(arg);
            index += 1;
            continue;
        }

        if matches!(arg.to_str(), Some("--")) {
            resume_args.push(arg);
            after_double_dash = true;
            index += 1;
            continue;
        }

        match arg.to_str() {
            Some("--only-openai") => {
                provider_visibility = ProviderVisibility::OnlyOpenAi;
            }
            Some("--include-archived") => {
                include_archived = true;
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
            Some("--last") => {
                last = true;
                resume_args.push(arg);
            }
            Some("--all") => {
                show_all = true;
                resume_args.push(arg);
            }
            Some("--include-non-interactive") => {
                include_non_interactive = true;
                resume_args.push(arg);
            }
            Some("--remote") => {
                remote = true;
                resume_args.push(arg);
                index += 1;
                let value = args
                    .get(index)
                    .with_context(|| "--remote requires a value")?
                    .clone();
                resume_args.push(value);
            }
            Some(value) if value.starts_with("--remote=") => {
                remote = true;
                resume_args.push(arg);
            }
            Some("--remote-auth-token-env") => {
                remote = true;
                resume_args.push(arg);
                index += 1;
                let value = args
                    .get(index)
                    .with_context(|| "--remote-auth-token-env requires a value")?
                    .clone();
                resume_args.push(value);
            }
            Some(value) if value.starts_with("--remote-auth-token-env=") => {
                remote = true;
                resume_args.push(arg);
            }
            Some("-C") | Some("--cd") => {
                resume_args.push(arg);
                index += 1;
                let value = args
                    .get(index)
                    .with_context(|| "-C/--cd requires a directory")?
                    .clone();
                if requested_cwd.is_none() {
                    requested_cwd = Some(PathBuf::from(&value));
                }
                resume_args.push(value);
            }
            Some(value) if value.starts_with("--cd=") => {
                if requested_cwd.is_none() {
                    requested_cwd = Some(PathBuf::from(value.trim_start_matches("--cd=")));
                }
                resume_args.push(arg);
            }
            Some(value) if value.starts_with("-C=") => {
                if requested_cwd.is_none() {
                    requested_cwd = Some(PathBuf::from(value.trim_start_matches("-C=")));
                }
                resume_args.push(arg);
            }
            Some(value) if takes_value(value) => {
                let flag = value.to_string();
                resume_args.push(arg);
                index += 1;
                let next = args
                    .get(index)
                    .with_context(|| format!("{flag} requires a value"))?
                    .clone();
                resume_args.push(next);
            }
            Some(value) if takes_value_with_equals(value) => {
                resume_args.push(arg);
            }
            Some(value) if value.starts_with('-') => {
                resume_args.push(arg);
            }
            _ => {
                resume_args.push(arg.clone());
                positionals.push(arg);
            }
        }

        index += 1;
    }

    let filter_cwd = if show_all {
        None
    } else {
        requested_cwd
            .map(ensure_absolute)
            .or_else(|| std::env::current_dir().ok())
    };
    let options = SessionDbOptions {
        provider_visibility,
        include_non_interactive,
        include_archived,
        filter_cwd,
    };

    let session_id_present = !positionals.is_empty();
    let resume_args_for_picker = strip_selection_flags(&resume_args);
    let mode = if remote || session_id_present {
        RunMode::Passthrough { resume_args }
    } else if last {
        RunMode::Last {
            resume_args: resume_args_for_picker,
            options,
        }
    } else {
        RunMode::Picker {
            resume_args: resume_args_for_picker,
            options,
        }
    };

    Ok(CliArgs {
        mode,
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
Drop-in-ish replacement for `codex resume` with a local picker that can list sessions\n\
across all providers.\n\
Use `--only-openai` to only show sessions whose provider id is `openai`.\n\
The picker loads the first page quickly and loads older sessions as you scroll.\n\
\n\
Wrapper flags:\n\
  --only-openai           only show sessions with provider id `openai`\n\
  --include-archived      include archived sessions in the picker and --last\n\
  --codex-home PATH       override CODEX_HOME / ~/.codex discovery\n\
  --sqlite-home PATH      override CODEX_SQLITE_HOME / config discovery\n\
\n\
Then exec:\n\
  codex resume <SESSION_ID> <forwarded args>\n\n\
Examples:\n\
  codexresume --last\n\
  codexresume --yolo\n\
  codexresume --only-openai --yolo\n\
  codexresume --codex-home ~/.codex --sqlite-home ~/.codex --yolo\n\
  codexresume -C /path/to/project --model gpt-5.4\n\
  codexresume --dangerously-bypass-approvals-and-sandbox \"continue from here\"\n"
    );
}

fn strip_selection_flags(args: &[OsString]) -> Vec<OsString> {
    let mut stripped = Vec::with_capacity(args.len());
    let mut index = 0usize;
    while index < args.len() {
        let arg = &args[index];
        if matches!(
            arg.to_str(),
            Some("--last" | "--all" | "--include-non-interactive")
        ) {
            index += 1;
            continue;
        }
        stripped.push(arg.clone());
        index += 1;
    }
    stripped
}

fn takes_value(flag: &str) -> bool {
    matches!(
        flag,
        "-c" | "--config"
            | "--enable"
            | "--disable"
            | "-i"
            | "--image"
            | "-m"
            | "--model"
            | "--local-provider"
            | "-p"
            | "--profile"
            | "-s"
            | "--sandbox"
            | "-a"
            | "--ask-for-approval"
            | "--add-dir"
    )
}

fn takes_value_with_equals(flag: &str) -> bool {
    [
        "--config=",
        "--enable=",
        "--disable=",
        "--image=",
        "--model=",
        "--local-provider=",
        "--profile=",
        "--sandbox=",
        "--ask-for-approval=",
        "--add-dir=",
        "-c=",
        "-i=",
        "-m=",
        "-p=",
        "-s=",
        "-a=",
    ]
    .into_iter()
    .any(|prefix| flag.starts_with(prefix))
}

fn ensure_absolute(path: PathBuf) -> PathBuf {
    if path.is_absolute() {
        return path;
    }
    std::env::current_dir()
        .map(|cwd| cwd.join(&path))
        .unwrap_or(path)
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

fn exec_codex_resume_passthrough(resume_args: Vec<OsString>) -> anyhow::Result<()> {
    let mut command = Command::new("codex");
    command.arg("resume").args(resume_args);

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
