use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;

use anyhow::Context;
use chrono::DateTime;
use chrono::Utc;
use sqlx::Row;
use sqlx::SqlitePool;
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::sqlite::SqlitePoolOptions;

use crate::config::ResolvedPaths;
use crate::session_index::find_thread_names_by_ids;

pub const OPENAI_PROVIDER_ID: &str = "openai";
pub const PAGE_SIZE: usize = 25;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SortKey {
    UpdatedAt,
    CreatedAt,
}

impl SortKey {
    pub fn toggle(self) -> Self {
        match self {
            Self::UpdatedAt => Self::CreatedAt,
            Self::CreatedAt => Self::UpdatedAt,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::UpdatedAt => "Updated at",
            Self::CreatedAt => "Created at",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProviderVisibility {
    All,
    OnlyOpenAi,
}

impl ProviderVisibility {
    pub fn label(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::OnlyOpenAi => "only openai",
        }
    }
}

#[derive(Clone)]
pub struct SessionDb {
    pool: SqlitePool,
    schema: ThreadSchema,
    session_index_path: PathBuf,
    provider_visibility: ProviderVisibility,
}

#[derive(Debug)]
pub struct SessionPage {
    pub rows: Vec<SessionRow>,
    pub has_more: bool,
}

#[derive(Clone, Debug)]
pub struct SessionRow {
    pub thread_id: String,
    pub thread_name: Option<String>,
    pub preview: String,
    pub created_at: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,
    pub archived: bool,
    pub provider: String,
    pub source: String,
    pub cwd: PathBuf,
    pub rollout_path: PathBuf,
    pub git_branch: Option<String>,
}

impl SessionRow {
    pub fn display_preview(&self) -> &str {
        self.thread_name.as_deref().unwrap_or(&self.preview)
    }

    pub fn short_id(&self) -> String {
        self.thread_id.chars().take(12).collect()
    }

    pub fn source_label(&self) -> &str {
        self.source.as_str()
    }
}

#[derive(Clone, Debug)]
struct ThreadSchema {
    columns: HashSet<String>,
}

impl ThreadSchema {
    fn has(&self, column: &str) -> bool {
        self.columns.contains(column)
    }

    fn select_expr(&self, column: &str, fallback: &str) -> String {
        if self.has(column) {
            column.to_string()
        } else {
            format!("{fallback} AS {column}")
        }
    }
}

pub async fn open_session_db(
    paths: &ResolvedPaths,
    provider_visibility: ProviderVisibility,
) -> anyhow::Result<SessionDb> {
    let options = SqliteConnectOptions::new()
        .filename(&paths.state_db_path)
        .create_if_missing(false)
        .read_only(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .with_context(|| {
            format!(
                "failed to open state db at {}",
                paths.state_db_path.display()
            )
        })?;
    let schema = load_thread_schema(&pool).await?;

    Ok(SessionDb {
        pool,
        schema,
        session_index_path: paths.session_index_path.clone(),
        provider_visibility,
    })
}

impl SessionDb {
    pub async fn load_page(&self, sort_key: SortKey, offset: usize) -> anyhow::Result<SessionPage> {
        let db_rows = sqlx::query(&page_query_sql(
            &self.schema,
            self.provider_visibility,
            sort_key,
        ))
        .bind(PAGE_SIZE as i64)
        .bind(offset as i64)
        .fetch_all(&self.pool)
        .await
        .context("failed to query threads from state db")?;

        let mut rows = db_rows
            .into_iter()
            .map(row_to_session)
            .collect::<anyhow::Result<Vec<_>>>()?;
        apply_thread_names(self.session_index_path.as_path(), &mut rows)?;

        Ok(SessionPage {
            has_more: rows.len() == PAGE_SIZE,
            rows,
        })
    }
}

async fn load_thread_schema(pool: &SqlitePool) -> anyhow::Result<ThreadSchema> {
    let rows = sqlx::query("PRAGMA table_info(threads)")
        .fetch_all(pool)
        .await
        .context("failed to inspect threads table schema")?;
    if rows.is_empty() {
        anyhow::bail!("threads table is missing from state db");
    }

    let columns = rows
        .into_iter()
        .map(|row| row.try_get::<String, _>("name"))
        .collect::<Result<HashSet<_>, _>>()
        .context("failed to read thread table column names")?;
    if !columns.contains("id") {
        anyhow::bail!("threads table does not contain an id column");
    }

    Ok(ThreadSchema { columns })
}

fn page_query_sql(
    schema: &ThreadSchema,
    provider_visibility: ProviderVisibility,
    sort_key: SortKey,
) -> String {
    let provider_filter = match provider_visibility {
        ProviderVisibility::All => String::new(),
        ProviderVisibility::OnlyOpenAi if schema.has("model_provider") => {
            format!("WHERE model_provider = '{OPENAI_PROVIDER_ID}'")
        }
        ProviderVisibility::OnlyOpenAi => String::new(),
    };
    let sort_column = match sort_key {
        SortKey::UpdatedAt if schema.has("updated_at") => "updated_at",
        SortKey::CreatedAt if schema.has("created_at") => "created_at",
        SortKey::UpdatedAt | SortKey::CreatedAt => "id",
    };

    format!(
        r#"
SELECT
    id,
    {},
    {},
    {},
    {},
    {},
    {},
    {},
    {},
    {},
    {}
FROM threads
{provider_filter}
ORDER BY {sort_column} DESC, id DESC
LIMIT ? OFFSET ?
        "#,
        schema.select_expr("rollout_path", "''"),
        schema.select_expr("created_at", "NULL"),
        schema.select_expr("updated_at", "NULL"),
        schema.select_expr("archived", "0"),
        schema.select_expr("source", "''"),
        schema.select_expr("model_provider", "''"),
        schema.select_expr("cwd", "''"),
        schema.select_expr("title", "''"),
        schema.select_expr("first_user_message", "''"),
        schema.select_expr("git_branch", "NULL"),
    )
}

fn row_to_session(row: sqlx::sqlite::SqliteRow) -> anyhow::Result<SessionRow> {
    let thread_id: String = row.try_get("id")?;
    let title: String = row.try_get("title")?;
    let first_user_message: String = row.try_get("first_user_message")?;
    let preview = if !first_user_message.trim().is_empty() {
        first_user_message
    } else if !title.trim().is_empty() {
        title
    } else {
        "(no message yet)".to_string()
    };

    Ok(SessionRow {
        thread_id,
        thread_name: None,
        preview,
        created_at: epoch_opt(row.try_get("created_at")?),
        updated_at: epoch_opt(row.try_get("updated_at")?),
        archived: row.try_get::<i64, _>("archived")? != 0,
        provider: normalize_provider_label(&row.try_get::<String, _>("model_provider")?),
        source: normalize_source_label(&row.try_get::<String, _>("source")?),
        cwd: PathBuf::from(row.try_get::<String, _>("cwd")?),
        rollout_path: PathBuf::from(row.try_get::<String, _>("rollout_path")?),
        git_branch: row.try_get("git_branch")?,
    })
}

fn apply_thread_names(session_index_path: &Path, rows: &mut [SessionRow]) -> anyhow::Result<()> {
    let ids = rows
        .iter()
        .map(|row| row.thread_id.clone())
        .collect::<HashSet<_>>();
    let names = find_thread_names_by_ids(session_index_path, &ids)
        .with_context(|| format!("failed to read {}", session_index_path.display()))?;
    for row in rows {
        row.thread_name = names.get(&row.thread_id).cloned();
    }
    Ok(())
}

fn epoch_opt(epoch: Option<i64>) -> Option<DateTime<Utc>> {
    epoch
        .and_then(|seconds| DateTime::from_timestamp(seconds, 0))
        .map(|dt| dt.with_timezone(&Utc))
}

fn normalize_provider_label(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        "unknown".to_string()
    } else {
        trimmed.to_string()
    }
}

fn normalize_source_label(raw: &str) -> String {
    match raw {
        "cli" => "cli".to_string(),
        "exec" => "exec".to_string(),
        "vscode" => "vscode".to_string(),
        "mcp" => "app-server".to_string(),
        other if other.starts_with('{') && other.contains("\"subagent\"") => "subagent".to_string(),
        other if other.trim().is_empty() => "unknown".to_string(),
        other => other.to_string(),
    }
}

pub fn session_cmp(sort_key: SortKey) -> impl Fn(&SessionRow, &SessionRow) -> std::cmp::Ordering {
    move |a, b| {
        let a_key = match sort_key {
            SortKey::UpdatedAt => a.updated_at.or(a.created_at),
            SortKey::CreatedAt => a.created_at,
        };
        let b_key = match sort_key {
            SortKey::UpdatedAt => b.updated_at.or(b.created_at),
            SortKey::CreatedAt => b.created_at,
        };
        b_key
            .cmp(&a_key)
            .then_with(|| a.thread_id.cmp(&b.thread_id))
    }
}

pub fn filter_rows(rows: &[SessionRow], query: &str, sort_key: SortKey) -> Vec<SessionRow> {
    let query = query.trim().to_lowercase();
    let mut filtered = rows
        .iter()
        .filter(|row| {
            if query.is_empty() {
                return true;
            }
            row.display_preview().to_lowercase().contains(&query)
                || row
                    .thread_name
                    .as_ref()
                    .is_some_and(|name| name.to_lowercase().contains(&query))
                || row.provider.to_lowercase().contains(&query)
                || row.source.to_lowercase().contains(&query)
                || row
                    .git_branch
                    .as_ref()
                    .is_some_and(|branch| branch.to_lowercase().contains(&query))
                || (row.archived && "archived".contains(&query))
                || row
                    .cwd
                    .display()
                    .to_string()
                    .to_lowercase()
                    .contains(&query)
                || row.thread_id.to_lowercase().contains(&query)
        })
        .cloned()
        .collect::<Vec<_>>();
    filtered.sort_by(session_cmp(sort_key));
    filtered
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn session(id: &str, provider: &str, preview: &str) -> SessionRow {
        SessionRow {
            thread_id: id.to_string(),
            thread_name: None,
            preview: preview.to_string(),
            created_at: DateTime::from_timestamp(10, 0).map(|dt| dt.with_timezone(&Utc)),
            updated_at: DateTime::from_timestamp(20, 0).map(|dt| dt.with_timezone(&Utc)),
            archived: false,
            provider: provider.to_string(),
            source: "exec".to_string(),
            cwd: PathBuf::from("/tmp/example"),
            rollout_path: PathBuf::from("/tmp/example.jsonl"),
            git_branch: None,
        }
    }

    fn schema(columns: &[&str]) -> ThreadSchema {
        ThreadSchema {
            columns: columns.iter().map(|column| (*column).to_string()).collect(),
        }
    }

    #[test]
    fn filter_rows_matches_provider_and_preview() {
        let rows = vec![
            session("thread-1", "openai", "fix tests"),
            session("thread-2", "apirouter", "audit contract"),
        ];

        let filtered = filter_rows(&rows, "api", SortKey::UpdatedAt);
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].provider, "apirouter");

        let filtered = filter_rows(&rows, "fix", SortKey::UpdatedAt);
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].preview, "fix tests");
    }

    #[test]
    fn page_query_sql_filters_only_openai_provider_when_requested() {
        let sql = page_query_sql(
            &schema(&["id", "created_at", "updated_at", "model_provider"]),
            ProviderVisibility::OnlyOpenAi,
            SortKey::UpdatedAt,
        );
        assert_eq!(
            sql.contains(&format!("WHERE model_provider = '{OPENAI_PROVIDER_ID}'")),
            true
        );

        let sql = page_query_sql(
            &schema(&["id", "created_at"]),
            ProviderVisibility::All,
            SortKey::CreatedAt,
        );
        assert_eq!(sql.contains("WHERE model_provider ="), false);
        assert_eq!(sql.contains("ORDER BY created_at DESC, id DESC"), true);
    }

    #[test]
    fn page_query_sql_falls_back_when_optional_columns_are_missing() {
        let sql = page_query_sql(
            &schema(&["id"]),
            ProviderVisibility::OnlyOpenAi,
            SortKey::UpdatedAt,
        );

        assert_eq!(sql.contains("NULL AS created_at"), true);
        assert_eq!(sql.contains("NULL AS updated_at"), true);
        assert_eq!(sql.contains("'' AS model_provider"), true);
        assert_eq!(sql.contains("ORDER BY id DESC, id DESC"), true);
        assert_eq!(sql.contains("WHERE model_provider ="), false);
    }
}
