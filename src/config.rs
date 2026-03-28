use std::env;
use std::path::Path;
use std::path::PathBuf;

use anyhow::Context;
use serde::Deserialize;

const CODEX_HOME_ENV: &str = "CODEX_HOME";
const SQLITE_HOME_ENV: &str = "CODEX_SQLITE_HOME";
const CONFIG_TOML_FILE: &str = "config.toml";
const DEFAULT_STATE_DB_FILE: &str = "state_5.sqlite";
const LEGACY_STATE_DB_FILE: &str = "state.sqlite";
const SESSION_INDEX_FILE: &str = "session_index.jsonl";

#[derive(Clone, Debug, Default)]
pub struct PathOverrides {
    pub codex_home: Option<PathBuf>,
    pub sqlite_home: Option<PathBuf>,
}

#[derive(Clone, Debug)]
pub struct ResolvedPaths {
    pub state_db_path: PathBuf,
    pub session_index_path: PathBuf,
}

#[derive(Clone, Debug, Default)]
struct EnvPaths {
    codex_home: Option<PathBuf>,
    sqlite_home: Option<PathBuf>,
}

#[derive(Debug, Deserialize)]
struct ConfigToml {
    sqlite_home: Option<PathBuf>,
}

pub fn resolve_paths(overrides: &PathOverrides) -> anyhow::Result<ResolvedPaths> {
    let home_dir = dirs::home_dir().context("failed to determine home directory")?;
    let env_paths = EnvPaths::from_process_env(home_dir.as_path());
    resolve_paths_from_sources(overrides, &env_paths, home_dir.as_path())
}

fn resolve_paths_from_sources(
    overrides: &PathOverrides,
    env_paths: &EnvPaths,
    home_dir: &Path,
) -> anyhow::Result<ResolvedPaths> {
    let codex_home = overrides
        .codex_home
        .clone()
        .or_else(|| env_paths.codex_home.clone())
        .unwrap_or_else(|| home_dir.join(".codex"));
    let codex_home = expand_home(codex_home, home_dir);

    let sqlite_home = match overrides
        .sqlite_home
        .clone()
        .or_else(|| env_paths.sqlite_home.clone())
    {
        Some(path) => expand_home(path, home_dir),
        None => read_sqlite_home_from_config(codex_home.as_path(), home_dir)?
            .unwrap_or_else(|| codex_home.clone()),
    };
    let state_db_path = discover_state_db_path(sqlite_home.as_path())?;

    Ok(ResolvedPaths {
        session_index_path: codex_home.join(SESSION_INDEX_FILE),
        state_db_path,
    })
}

impl EnvPaths {
    fn from_process_env(home_dir: &Path) -> Self {
        Self {
            codex_home: env::var_os(CODEX_HOME_ENV)
                .map(PathBuf::from)
                .map(|path| expand_home(path, home_dir)),
            sqlite_home: env::var_os(SQLITE_HOME_ENV)
                .map(PathBuf::from)
                .map(|path| expand_home(path, home_dir)),
        }
    }
}

fn read_sqlite_home_from_config(
    codex_home: &Path,
    home_dir: &Path,
) -> anyhow::Result<Option<PathBuf>> {
    let config_path = codex_home.join(CONFIG_TOML_FILE);
    if !config_path.is_file() {
        return Ok(None);
    }

    let contents = std::fs::read_to_string(&config_path)
        .with_context(|| format!("failed to read {}", config_path.display()))?;
    let config: ConfigToml = toml::from_str(&contents)
        .with_context(|| format!("failed to parse {}", config_path.display()))?;
    Ok(config.sqlite_home.map(|path| expand_home(path, home_dir)))
}

pub fn discover_state_db_path(sqlite_home: &Path) -> anyhow::Result<PathBuf> {
    let metadata = std::fs::metadata(sqlite_home)
        .with_context(|| format!("failed to access sqlite home {}", sqlite_home.display()))?;
    if !metadata.is_dir() {
        anyhow::bail!("sqlite home {} is not a directory", sqlite_home.display());
    }

    let mut latest_version: Option<(u32, PathBuf)> = None;
    for entry in std::fs::read_dir(sqlite_home)
        .with_context(|| format!("failed to read sqlite home {}", sqlite_home.display()))?
    {
        let entry = entry?;
        let file_name = entry.file_name();
        let file_name = file_name.to_string_lossy();
        let Some(version) = parse_state_db_version(file_name.as_ref()) else {
            continue;
        };
        let path = entry.path();
        match latest_version {
            Some((best_version, _)) if best_version >= version => {}
            _ => latest_version = Some((version, path)),
        }
    }

    if let Some((_, path)) = latest_version {
        return Ok(path);
    }

    for file_name in [DEFAULT_STATE_DB_FILE, LEGACY_STATE_DB_FILE] {
        let candidate = sqlite_home.join(file_name);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }

    anyhow::bail!("no state db found under {}", sqlite_home.display());
}

fn parse_state_db_version(file_name: &str) -> Option<u32> {
    let version = file_name.strip_prefix("state_")?.strip_suffix(".sqlite")?;
    version.parse().ok()
}

fn expand_home(path: PathBuf, home_dir: &Path) -> PathBuf {
    let Some(path_str) = path.to_str() else {
        return path;
    };
    if path_str == "~" {
        return home_dir.to_path_buf();
    }
    if let Some(stripped) = path_str
        .strip_prefix("~/")
        .or_else(|| path_str.strip_prefix("~\\"))
    {
        return home_dir.join(stripped);
    }
    path
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use tempfile::tempdir;

    #[test]
    fn resolve_paths_uses_config_sqlite_home_when_present() {
        let home = tempdir().expect("tempdir");
        let codex_home = home.path().join(".codex");
        let sqlite_home = home.path().join("sqlite");
        std::fs::create_dir_all(&codex_home).expect("create codex home");
        std::fs::create_dir_all(&sqlite_home).expect("create sqlite home");
        std::fs::write(
            codex_home.join(CONFIG_TOML_FILE),
            format!("sqlite_home = {:?}\n", sqlite_home.display().to_string()),
        )
        .expect("write config");
        std::fs::write(sqlite_home.join(DEFAULT_STATE_DB_FILE), []).expect("write db");

        let resolved = resolve_paths_from_sources(
            &PathOverrides::default(),
            &EnvPaths::default(),
            home.path(),
        )
        .expect("resolve paths");

        assert_eq!(
            resolved
                .session_index_path
                .parent()
                .expect("codex home parent"),
            codex_home
        );
        assert_eq!(
            resolved.state_db_path.parent().expect("sqlite home parent"),
            sqlite_home
        );
        assert_eq!(
            resolved.state_db_path,
            sqlite_home.join(DEFAULT_STATE_DB_FILE)
        );
    }

    #[test]
    fn resolve_paths_prefers_overrides_over_env_and_config() {
        let home = tempdir().expect("tempdir");
        let config_codex_home = home.path().join(".codex");
        let config_sqlite_home = home.path().join("config-sqlite");
        let override_codex_home = home.path().join("override-codex");
        let override_sqlite_home = home.path().join("override-sqlite");
        std::fs::create_dir_all(&config_codex_home).expect("create config codex home");
        std::fs::create_dir_all(&config_sqlite_home).expect("create config sqlite home");
        std::fs::create_dir_all(&override_codex_home).expect("create override codex home");
        std::fs::create_dir_all(&override_sqlite_home).expect("create override sqlite home");
        std::fs::write(
            config_codex_home.join(CONFIG_TOML_FILE),
            format!(
                "sqlite_home = {:?}\n",
                config_sqlite_home.display().to_string()
            ),
        )
        .expect("write config");
        std::fs::write(override_sqlite_home.join(DEFAULT_STATE_DB_FILE), []).expect("write db");

        let resolved = resolve_paths_from_sources(
            &PathOverrides {
                codex_home: Some(override_codex_home.clone()),
                sqlite_home: Some(override_sqlite_home.clone()),
            },
            &EnvPaths {
                codex_home: Some(config_codex_home),
                sqlite_home: Some(config_sqlite_home),
            },
            home.path(),
        )
        .expect("resolve paths");

        assert_eq!(
            resolved
                .session_index_path
                .parent()
                .expect("codex home parent"),
            override_codex_home
        );
        assert_eq!(
            resolved.state_db_path.parent().expect("sqlite home parent"),
            override_sqlite_home
        );
    }

    #[test]
    fn discover_state_db_path_prefers_highest_version() {
        let dir = tempdir().expect("tempdir");
        std::fs::write(dir.path().join("state_4.sqlite"), []).expect("write db");
        std::fs::write(dir.path().join("state_7.sqlite"), []).expect("write db");
        std::fs::write(dir.path().join("state_5.sqlite"), []).expect("write db");

        let path = discover_state_db_path(dir.path()).expect("discover path");

        assert_eq!(path, dir.path().join("state_7.sqlite"));
    }
}
