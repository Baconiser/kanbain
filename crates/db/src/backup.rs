//! SQLite backups (plan D12): a copy before every migration and a daily
//! copy, each pruned to the newest N. Copies are made with `VACUUM INTO`,
//! which is consistent while the database is in use.

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use chrono::Utc;
use sqlx::{Pool, Sqlite, migrate::Migrator};
use utils::assets::asset_dir;

/// Pre-migration backups kept regardless of the user setting.
pub const PRE_MIGRATION_KEEP: usize = 5;
/// Default number of daily backups to keep.
pub const DEFAULT_DAILY_KEEP: u32 = 7;
/// Age after which a new daily backup is taken.
pub const DAILY_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackupKind {
    PreMigration,
    Daily,
}

impl BackupKind {
    fn tag(self) -> &'static str {
        match self {
            Self::PreMigration => "premigration",
            Self::Daily => "daily",
        }
    }
}

pub fn backups_dir() -> PathBuf {
    asset_dir().join("backups")
}

fn file_prefix(kind: BackupKind) -> String {
    format!("db-{}-", kind.tag())
}

/// Backups of one kind in `dir`, oldest first. Names sort by timestamp.
pub fn list_backups(dir: &Path, kind: BackupKind) -> Vec<PathBuf> {
    let prefix = file_prefix(kind);
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with(&prefix) && n.ends_with(".sqlite"))
        })
        .collect();
    files.sort();
    files
}

/// Deletes all but the newest `keep` backups of `kind`.
pub fn prune(dir: &Path, kind: BackupKind, keep: usize) -> std::io::Result<()> {
    let files = list_backups(dir, kind);
    let excess = files.len().saturating_sub(keep);
    for path in &files[..excess] {
        std::fs::remove_file(path)?;
    }
    Ok(())
}

/// Writes a consistent copy of the database into `dir`.
pub async fn backup_into(
    pool: &Pool<Sqlite>,
    dir: &Path,
    kind: BackupKind,
) -> Result<PathBuf, sqlx::Error> {
    std::fs::create_dir_all(dir)?;
    let mut path = dir.join(format!(
        "{}{}.sqlite",
        file_prefix(kind),
        Utc::now().format("%Y%m%d-%H%M%S")
    ));
    // Two backups within the same second: keep both.
    let mut n = 1;
    while path.exists() {
        path = dir.join(format!(
            "{}{}-{n}.sqlite",
            file_prefix(kind),
            Utc::now().format("%Y%m%d-%H%M%S")
        ));
        n += 1;
    }
    sqlx::query("VACUUM INTO ?")
        .bind(path.to_string_lossy().to_string())
        .execute(pool)
        .await?;
    Ok(path)
}

/// Whether `migrator` has migrations the database has not applied yet. A
/// database without a migrations table is new and needs no backup.
pub async fn has_pending_migrations(
    pool: &Pool<Sqlite>,
    migrator: &Migrator,
) -> Result<bool, sqlx::Error> {
    let table_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = '_sqlx_migrations')",
    )
    .fetch_one(pool)
    .await?;
    if !table_exists {
        return Ok(false);
    }
    let applied: HashSet<i64> =
        sqlx::query_scalar("SELECT version FROM _sqlx_migrations WHERE success = 1")
            .fetch_all(pool)
            .await?
            .into_iter()
            .collect();
    Ok(migrator
        .iter()
        .any(|m| !m.migration_type.is_down_migration() && !applied.contains(&m.version)))
}

/// Takes a daily backup when the newest one is older than a day, then prunes.
/// Returns the new backup's path, if one was taken.
pub async fn run_daily_backup(
    pool: &Pool<Sqlite>,
    dir: &Path,
    keep: u32,
) -> Result<Option<PathBuf>, sqlx::Error> {
    let newest_age = list_backups(dir, BackupKind::Daily)
        .last()
        .and_then(|p| std::fs::metadata(p).ok())
        .and_then(|m| m.modified().ok())
        .and_then(|t| SystemTime::now().duration_since(t).ok());
    if newest_age.is_some_and(|age| age < DAILY_INTERVAL) {
        return Ok(None);
    }
    let path = backup_into(pool, dir, BackupKind::Daily).await?;
    prune(dir, BackupKind::Daily, keep.max(1) as usize)?;
    Ok(Some(path))
}

#[cfg(test)]
mod tests {
    use sqlx::sqlite::SqlitePoolOptions;

    use super::*;

    fn temp_dir() -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("kanbain-backup-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[tokio::test]
    async fn daily_backup_is_taken_once_and_pruned() {
        let dir = temp_dir();
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(
                sqlx::sqlite::SqliteConnectOptions::new()
                    .filename(dir.join("source.db"))
                    .create_if_missing(true),
            )
            .await
            .unwrap();
        sqlx::query("CREATE TABLE t (x INTEGER)")
            .execute(&pool)
            .await
            .unwrap();

        let first = run_daily_backup(&pool, &dir, 2).await.unwrap();
        let first = first.expect("first daily backup is taken");
        assert!(first.exists(), "missing {}", first.display());
        // Within a day: no second backup.
        assert!(run_daily_backup(&pool, &dir, 2).await.unwrap().is_none());

        for _ in 0..3 {
            backup_into(&pool, &dir, BackupKind::Daily).await.unwrap();
        }
        prune(&dir, BackupKind::Daily, 2).unwrap();
        assert_eq!(list_backups(&dir, BackupKind::Daily).len(), 2);
        assert!(list_backups(&dir, BackupKind::PreMigration).is_empty());

        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn pending_migrations_are_detected() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        let migrator = sqlx::migrate!("./migrations");
        // Fresh database: nothing to protect.
        assert!(!has_pending_migrations(&pool, &migrator).await.unwrap());
        migrator.run(&pool).await.unwrap();
        assert!(!has_pending_migrations(&pool, &migrator).await.unwrap());
        sqlx::query("DELETE FROM _sqlx_migrations WHERE version = (SELECT MAX(version) FROM _sqlx_migrations)")
            .execute(&pool)
            .await
            .unwrap();
        assert!(has_pending_migrations(&pool, &migrator).await.unwrap());
    }
}
