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

#[derive(Clone, Debug)]
pub struct SessionDbOptions {
    pub provider_visibility: ProviderVisibility,
    pub include_non_interactive: bool,
    pub include_archived: bool,
    pub filter_cwd: Option<PathBuf>,
}

#[derive(Clone)]
pub struct SessionDb {
    pool: SqlitePool,
    schema: ThreadSchema,
    session_index_path: PathBuf,
    options: SessionDbOptions,
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
    db_options: SessionDbOptions,
) -> anyhow::Result<SessionDb> {
    let connect_options = SqliteConnectOptions::new()
        .filename(&paths.state_db_path)
        .create_if_missing(false)
        .read_only(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(connect_options)
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
        options: db_options,
    })
}

impl SessionDb {
    pub async fn load_page(&self, sort_key: SortKey, offset: usize) -> anyhow::Result<SessionPage> {
        let query = page_query_sql(&self.schema, &self.options, sort_key);
        let mut db_query = sqlx::query(&query.sql);
        if let Some(cwd) = query.cwd_bind.as_deref() {
            db_query = db_query.bind(cwd);
        }
        let db_rows = db_query
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

    pub async fn select_last_thread_id(&self) -> anyhow::Result<Option<String>> {
        let query = last_thread_id_query_sql(&self.schema, &self.options);
        let mut db_query = sqlx::query(&query.sql);
        if let Some(cwd) = query.cwd_bind.as_deref() {
            db_query = db_query.bind(cwd);
        }
        let row = db_query
            .fetch_optional(&self.pool)
            .await
            .context("failed to query last thread id from state db")?;
        row.map(|row| row.try_get::<String, _>("id"))
            .transpose()
            .context("failed to decode last thread id")
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

struct ThreadsQuery {
    sql: String,
    cwd_bind: Option<String>,
}

fn page_query_sql(
    schema: &ThreadSchema,
    options: &SessionDbOptions,
    sort_key: SortKey,
) -> ThreadsQuery {
    let where_clause = threads_where_clause(schema, options);
    let clause = where_clause.clause.clone();
    let cwd_bind = where_clause.cwd_bind.clone();
    let sort_column = match sort_key {
        SortKey::UpdatedAt if schema.has("updated_at") => "updated_at",
        SortKey::CreatedAt if schema.has("created_at") => "created_at",
        SortKey::UpdatedAt | SortKey::CreatedAt => "id",
    };

    ThreadsQuery {
        cwd_bind,
        sql: format!(
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
{clause}
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
        ),
    }
}

fn last_thread_id_query_sql(schema: &ThreadSchema, options: &SessionDbOptions) -> ThreadsQuery {
    let where_clause = threads_where_clause(schema, options);
    let clause = where_clause.clause.clone();
    let cwd_bind = where_clause.cwd_bind.clone();
    let sort_column = if schema.has("updated_at") {
        "updated_at"
    } else if schema.has("created_at") {
        "created_at"
    } else {
        "id"
    };
    ThreadsQuery {
        cwd_bind,
        sql: format!(
            r#"
SELECT id
FROM threads
{clause}
ORDER BY {sort_column} DESC, id DESC
LIMIT 1
            "#
        ),
    }
}

struct WhereClause {
    clause: String,
    cwd_bind: Option<String>,
}

impl std::fmt::Display for WhereClause {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.clause)
    }
}

fn threads_where_clause(schema: &ThreadSchema, options: &SessionDbOptions) -> WhereClause {
    let mut parts: Vec<String> = Vec::new();
    let mut cwd_bind = None;

    if matches!(options.provider_visibility, ProviderVisibility::OnlyOpenAi)
        && schema.has("model_provider")
    {
        parts.push(format!("model_provider = '{OPENAI_PROVIDER_ID}'"));
    }
    if !options.include_archived && schema.has("archived") {
        parts.push("archived = 0".to_string());
    }
    if !options.include_non_interactive && schema.has("source") {
        parts.push("source IN ('cli', 'vscode')".to_string());
    }
    if let Some(filter_cwd) = options.filter_cwd.as_ref().filter(|_| schema.has("cwd")) {
        parts.push("cwd = ?".to_string());
        cwd_bind = Some(filter_cwd.display().to_string());
    }

    let clause = if parts.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", parts.join(" AND "))
    };

    WhereClause { clause, cwd_bind }
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
        let options = SessionDbOptions {
            provider_visibility: ProviderVisibility::OnlyOpenAi,
            include_non_interactive: true,
            include_archived: true,
            filter_cwd: None,
        };
        let sql = page_query_sql(
            &schema(&["id", "created_at", "updated_at", "model_provider"]),
            &options,
            SortKey::UpdatedAt,
        )
        .sql;
        assert_eq!(
            sql.contains(&format!("WHERE model_provider = '{OPENAI_PROVIDER_ID}'")),
            true
        );

        let options = SessionDbOptions {
            provider_visibility: ProviderVisibility::All,
            include_non_interactive: true,
            include_archived: true,
            filter_cwd: None,
        };
        let sql = page_query_sql(&schema(&["id", "created_at"]), &options, SortKey::CreatedAt).sql;
        assert_eq!(sql.contains("WHERE model_provider ="), false);
        assert_eq!(sql.contains("ORDER BY created_at DESC, id DESC"), true);
    }

    #[test]
    fn page_query_sql_falls_back_when_optional_columns_are_missing() {
        let options = SessionDbOptions {
            provider_visibility: ProviderVisibility::OnlyOpenAi,
            include_non_interactive: true,
            include_archived: true,
            filter_cwd: None,
        };
        let sql = page_query_sql(&schema(&["id"]), &options, SortKey::UpdatedAt).sql;

        assert_eq!(sql.contains("NULL AS created_at"), true);
        assert_eq!(sql.contains("NULL AS updated_at"), true);
        assert_eq!(sql.contains("'' AS model_provider"), true);
        assert_eq!(sql.contains("ORDER BY id DESC, id DESC"), true);
        assert_eq!(sql.contains("WHERE model_provider ="), false);
    }
}
