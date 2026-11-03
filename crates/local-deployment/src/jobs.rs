//! Headless agent jobs (D34): intake and history analysis run an agent
//! definition of the matching role in a throwaway sandbox container with no
//! repo and no tools. The setup scan gets the project's repos and its
//! agent's tools.

use std::{sync::Arc, time::Duration};

use db::{
    DBService,
    models::{agent_job::AgentJob, project::Project},
};
use executors::adapters::{AgentRole, RunSettings, adapter_for};
use services::services::agent_library::AgentLibrary;
use uuid::Uuid;

use crate::sandbox::{JobOutput, JobRun, SandboxRuntime};

/// Spend limit when the agent definition sets none.
const DEFAULT_JOB_BUDGET_USD: f64 = 0.5;
/// Time limit when the agent definition sets none.
const DEFAULT_JOB_MINUTES: u32 = 10;

/// One job to run.
pub struct JobRequest {
    pub kind: &'static str,
    pub role: AgentRole,
    pub project: Option<Project>,
    pub issue_id: Option<Uuid>,
    /// Files for `/kanbain/context`.
    pub context: Vec<(String, String)>,
    /// The first message, with everything the agent needs inline (it has no
    /// tools to read files).
    pub task: String,
    /// Repos to clone into the job container, `(name, host checkout,
    /// branch)`.
    pub repos: Vec<(String, std::path::PathBuf, String)>,
}

#[derive(Clone)]
pub struct AgentJobs {
    db: DBService,
    sandbox: SandboxRuntime,
    library: Arc<AgentLibrary>,
}

impl AgentJobs {
    pub fn new(db: DBService, sandbox: SandboxRuntime, library: Arc<AgentLibrary>) -> Self {
        Self {
            db,
            sandbox,
            library,
        }
    }

    /// The agent for a role: the one named after the role, else the first
    /// valid definition with that role. Shared with the `for-role` API via
    /// the library so they can't drift.
    pub async fn agent_name_for(&self, role: AgentRole) -> Result<String, String> {
        self.library
            .agent_name_for_role(role)
            .await
            .map_err(|e| e.to_string())
    }

    /// Creates the job row and runs it to completion. The job row records the
    /// outcome; the result is the agent's final message and out files.
    pub async fn run(&self, request: JobRequest) -> (Option<Uuid>, Result<JobOutput, String>) {
        let pool = &self.db.pool;
        let agent = match self.agent_name_for(request.role).await {
            Ok(name) => match self.library.resolve(&name).await {
                Ok(agent) => agent,
                Err(e) => return (None, Err(format!("Agent {name}: {e}"))),
            },
            Err(e) => return (None, Err(e)),
        };
        let def = &agent.definition;
        let budget = def.limits.budget_usd.unwrap_or(DEFAULT_JOB_BUDGET_USD);
        let job = match AgentJob::create(
            pool,
            request.kind,
            request.project.as_ref().map(|p| p.id),
            request.issue_id,
            &def.name,
            &def.model,
            Some(budget),
        )
        .await
        {
            Ok(job) => job,
            Err(e) => return (None, Err(e.to_string())),
        };

        let result = if !self.sandbox.enabled().await {
            Err("Agent jobs need the Docker sandbox, which is not available".to_string())
        } else {
            let launch = adapter_for(def.runtime).launch(
                &agent,
                &RunSettings {
                    budget_usd: Some(budget),
                },
            );
            let minutes = def.limits.wall_minutes.unwrap_or(DEFAULT_JOB_MINUTES);
            self.sandbox
                .run_job(JobRun {
                    job_id: job.id,
                    launch: &launch,
                    context: request.context,
                    task: request.task,
                    project: request.project.as_ref(),
                    budget_usd: Some(budget),
                    timeout: Duration::from_secs(u64::from(minutes) * 60),
                    repos: request.repos,
                })
                .await
                .map_err(|e| e.to_string())
        };

        let (output, error) = match &result {
            Ok(output) => (Some(output.message.as_str()), None),
            Err(e) => (None, Some(e.as_str())),
        };
        if let Err(e) = AgentJob::finish(pool, job.id, output, error).await {
            tracing::error!("Recording job {} failed: {e}", job.id);
        }
        (Some(job.id), result)
    }
}
