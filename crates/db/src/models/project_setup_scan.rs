use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, SqlitePool, Type, types::Json};
use strum_macros::{Display, EnumString};
use ts_rs::TS;
use uuid::Uuid;

#[derive(
    Debug, Clone, Copy, Type, Serialize, Deserialize, PartialEq, Eq, TS, EnumString, Display,
)]
#[sqlx(type_name = "text", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum SetupScanStatus {
    Scanning,
    /// A proposal waits for the user's review.
    Ready,
    Failed,
    Approved,
    Discarded,
}

/// What the setup agent proposes for a project's sandbox.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct SetupProposal {
    /// Extra setup commands, run after KanbAIn's lockfile installs.
    pub setup_script: String,
    pub test_commands: Vec<String>,
    pub run_commands: Vec<String>,
    pub environment_md: String,
    /// Skip KanbAIn's own lockfile installs; the script does it all.
    pub skip_auto_install: bool,
}

/// A project's latest setup scan (one row per project).
#[derive(Debug, Clone, FromRow, Serialize, Deserialize, TS)]
pub struct ProjectSetupScan {
    pub project_id: Uuid,
    pub status: SetupScanStatus,
    pub job_id: Option<Uuid>,
    pub setup_script: Option<String>,
    #[ts(type = "string[]")]
    pub test_commands: Json<Vec<String>>,
    #[ts(type = "string[]")]
    pub run_commands: Json<Vec<String>>,
    pub environment_md: Option<String>,
    pub skip_auto_install: bool,
    pub summary: Option<String>,
    pub stack_hash: String,
    pub error: Option<String>,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub decided_at: Option<DateTime<Utc>>,
}

impl ProjectSetupScan {
    pub fn proposal(&self) -> SetupProposal {
        SetupProposal {
            setup_script: self.setup_script.clone().unwrap_or_default(),
            test_commands: self.test_commands.0.clone(),
            run_commands: self.run_commands.0.clone(),
            environment_md: self.environment_md.clone().unwrap_or_default(),
            skip_auto_install: self.skip_auto_install,
        }
    }

    pub async fn find(pool: &SqlitePool, project_id: Uuid) -> Result<Option<Self>, sqlx::Error> {
        sqlx::query_as::<_, Self>("SELECT * FROM project_setup_scans WHERE project_id = $1")
            .bind(project_id)
            .fetch_optional(pool)
            .await
    }

    /// Claims a new scan, replacing the previous proposal. False while one
    /// is already scanning.
    pub async fn begin(
        pool: &SqlitePool,
        project_id: Uuid,
        stack_hash: &str,
    ) -> Result<bool, sqlx::Error> {
        let result = sqlx::query(
            r#"INSERT INTO project_setup_scans (project_id, stack_hash) VALUES ($1, $2)
               ON CONFLICT(project_id) DO UPDATE SET
                   status = 'scanning', job_id = NULL, setup_script = NULL,
                   test_commands = '[]', run_commands = '[]', environment_md = NULL,
                   skip_auto_install = 0, summary = NULL, stack_hash = excluded.stack_hash,
                   error = NULL, started_at = datetime('now', 'subsec'),
                   finished_at = NULL, decided_at = NULL
               WHERE project_setup_scans.status != 'scanning'"#,
        )
        .bind(project_id)
        .bind(stack_hash)
        .execute(pool)
        .await?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn set_job(
        pool: &SqlitePool,
        project_id: Uuid,
        job_id: Uuid,
    ) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE project_setup_scans SET job_id = $2 WHERE project_id = $1")
            .bind(project_id)
            .bind(job_id)
            .execute(pool)
            .await?;
        Ok(())
    }

    pub async fn set_ready(
        pool: &SqlitePool,
        project_id: Uuid,
        proposal: &SetupProposal,
        summary: Option<&str>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            r#"UPDATE project_setup_scans SET status = 'ready', setup_script = $2,
                   test_commands = $3, run_commands = $4, environment_md = $5,
                   skip_auto_install = $6, summary = $7, error = NULL,
                   finished_at = datetime('now', 'subsec')
               WHERE project_id = $1"#,
        )
        .bind(project_id)
        .bind(&proposal.setup_script)
        .bind(Json(&proposal.test_commands))
        .bind(Json(&proposal.run_commands))
        .bind(&proposal.environment_md)
        .bind(proposal.skip_auto_install)
        .bind(summary)
        .execute(pool)
        .await?;
        Ok(())
    }

    pub async fn set_failed(
        pool: &SqlitePool,
        project_id: Uuid,
        error: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            r#"UPDATE project_setup_scans SET status = 'failed', error = $2,
                   finished_at = datetime('now', 'subsec')
               WHERE project_id = $1"#,
        )
        .bind(project_id)
        .bind(error)
        .execute(pool)
        .await?;
        Ok(())
    }

    /// Approves or discards a ready proposal. False when there was none.
    pub async fn decide(
        pool: &SqlitePool,
        project_id: Uuid,
        status: SetupScanStatus,
        proposal: Option<&SetupProposal>,
    ) -> Result<bool, sqlx::Error> {
        let result = match proposal {
            // The user may have edited it on the way.
            Some(p) => sqlx::query(
                r#"UPDATE project_setup_scans SET status = $2, setup_script = $3,
                       test_commands = $4, run_commands = $5, environment_md = $6,
                       skip_auto_install = $7, decided_at = datetime('now', 'subsec')
                   WHERE project_id = $1 AND status = 'ready'"#,
            )
            .bind(project_id)
            .bind(status)
            .bind(&p.setup_script)
            .bind(Json(&p.test_commands))
            .bind(Json(&p.run_commands))
            .bind(&p.environment_md)
            .bind(p.skip_auto_install),
            None => sqlx::query(
                r#"UPDATE project_setup_scans SET status = $2,
                       decided_at = datetime('now', 'subsec')
                   WHERE project_id = $1 AND status = 'ready'"#,
            )
            .bind(project_id)
            .bind(status),
        }
        .execute(pool)
        .await?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn fail_interrupted(pool: &SqlitePool) -> Result<u64, sqlx::Error> {
        let result = sqlx::query(
            r#"UPDATE project_setup_scans SET status = 'failed',
                   error = 'Interrupted by an app restart',
                   finished_at = datetime('now', 'subsec')
               WHERE status = 'scanning'"#,
        )
        .execute(pool)
        .await?;
        Ok(result.rows_affected())
    }
}
