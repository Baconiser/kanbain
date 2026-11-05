//! The setup scan: the `setup` agent reads a project's repos (and the web,
//! when the README is lying) and proposes how the sandbox should prepare
//! them: a `setup.sh` on top of KanbAIn's lockfile installs, test and run
//! commands, and `environment.md` notes for every later agent.
//!
//! It runs as a headless job with the repos cloned in, read + web tools and
//! writes fenced to `/kanbain/out`. Nothing it says takes effect until the
//! user approves it in the project settings; approval copies the proposal
//! into [`ProjectSettings`], which [`crate::project_setup`] and
//! [`crate::environment`] read on every run.
//!
//! The approved setup remembers the repos' stack fingerprint
//! ([`stack_fingerprint`]). When an agent run sees a different one (new
//! manifest, switched package manager, bumped toolchain pin), the scan runs
//! again by itself and the fresh proposal waits for review. One rescan per
//! fingerprint, so a stack the agent can't figure out doesn't loop forever.
//!
//! Agents that find something missing mid-run ask for it with
//! `request_setup` ([`crate::setup_request`]). The next scan gets the
//! pending requests in its task and answers them with role-only commands
//! (`roles/<role>.sh`); approving that proposal resolves them.

use std::{collections::BTreeMap, path::PathBuf};

use db::{
    DBService,
    models::{
        agent_job::JOB_KIND_SETUP,
        project::{Project, ProjectRepo, ProjectSettings, UpdateProject},
        project_setup_scan::{ProjectSetupScan, SetupProposal, SetupScanStatus},
        setup_request::{SetupRequest, SetupRequestStatus},
    },
};
use executors::adapters::{AgentRole, headless::extract_json_object};
use git::GitService;
use serde::Serialize;
use services::services::container::ContainerService;
use sha2::{Digest, Sha256};
use ts_rs::TS;
use uuid::Uuid;

use crate::{
    container::LocalContainerService,
    jobs::{AgentJobs, JobRequest},
    project_setup,
};

/// Files whose presence (or, for version pins, contents) define a repo's
/// stack. Lockfile *contents* are deliberately out: a dependency bump is
/// project setup's job (it reinstalls on its own), not a reason to rescan.
const STACK_FILES: &[&str] = &[
    "package.json",
    "pnpm-lock.yaml",
    "pnpm-workspace.yaml",
    "yarn.lock",
    "package-lock.json",
    "bun.lock",
    "bun.lockb",
    "Cargo.toml",
    "pyproject.toml",
    "requirements.txt",
    "setup.py",
    "Pipfile",
    "poetry.lock",
    "uv.lock",
    "go.mod",
    "Gemfile",
    "composer.json",
    "pom.xml",
    "build.gradle",
    "build.gradle.kts",
    "mix.exs",
    "deno.json",
    "Dockerfile",
    "docker-compose.yml",
    "compose.yaml",
    "Makefile",
    "justfile",
];

/// Version pins: their contents count, because "Node 18 → 22" is a stack
/// change even though no file appeared.
const PIN_FILES: &[&str] = &[
    "rust-toolchain.toml",
    "rust-toolchain",
    ".nvmrc",
    ".node-version",
    ".python-version",
    ".tool-versions",
    ".ruby-version",
];

/// Largest `setup.sh` / `environment.md` kept from the agent.
const MAX_SCRIPT_CHARS: usize = 16 * 1024;
const MAX_NOTES_CHARS: usize = 16 * 1024;
const MAX_COMMANDS: usize = 20;

#[derive(Debug, thiserror::Error)]
pub enum SetupScanError {
    #[error(transparent)]
    Database(#[from] sqlx::Error),
    #[error("Project not found")]
    NotFound,
    #[error("A setup scan for this project is already running")]
    AlreadyScanning,
    #[error("{0}")]
    Invalid(String),
}

/// What the project settings show about the setup scan.
#[derive(Debug, Clone, Serialize, TS)]
pub struct SetupScanView {
    pub scan: Option<ProjectSetupScan>,
    /// The repos' stack differs from the one the approved setup was made
    /// for. `false` when nothing was approved yet.
    pub stack_changed: bool,
    /// What KanbAIn installs by itself, as `[repo] command`.
    pub auto_steps: Vec<String>,
    /// Setup requests from agents that wait for the user.
    pub requests: Vec<SetupRequest>,
}

/// Fingerprint of the repos' stacks: which stack files exist at each root,
/// plus the contents of the version pins. `repos` are `(name, root)` pairs.
pub fn stack_fingerprint(repos: &[(String, PathBuf)]) -> String {
    let mut repos: Vec<&(String, PathBuf)> = repos.iter().collect();
    repos.sort_by(|a, b| a.0.cmp(&b.0));
    let mut hasher = Sha256::new();
    for (name, root) in repos {
        hasher.update(format!("repo {name}\n"));
        for file in STACK_FILES {
            if root.join(file).exists() {
                hasher.update(format!("has {file}\n"));
            }
        }
        for file in PIN_FILES {
            if let Ok(contents) = std::fs::read_to_string(root.join(file)) {
                hasher.update(format!("pin {file}\n{}\n", contents.trim()));
            }
        }
    }
    hasher
        .finalize()
        .iter()
        .take(8)
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The setup agent's task. `auto_steps` is what KanbAIn already installs,
/// so the script doesn't do it twice.
pub fn scan_task(
    project_name: &str,
    repos: &[String],
    auto_steps: &[String],
    settings: &ProjectSettings,
    requests: &[SetupRequest],
) -> String {
    let mut task = format!(
        "Work out how to set up the project \"{project_name}\" in KanbAIn's sandbox, so \
         coding and testing agents can build, test and run it without guessing.\n\n\
         The repositories are checked out read-only at:\n"
    );
    for repo in repos {
        task.push_str(&format!("- /workspace/{repo}\n"));
    }
    task.push_str(&format!(
        "\nThe sandbox is a Debian container (user `node`, no root, no sudo) with:\n{}\
         Network access goes only to package registries and the project's allowed domains. \
         Every run starts from a fresh container; package caches persist.\n\n",
        crate::environment::BASE_IMAGE_TOOLS
    ));
    if auto_steps.is_empty() {
        task.push_str("KanbAIn found no lockfiles to install from on its own.\n\n");
    } else {
        task.push_str(
            "Before every run KanbAIn already runs these in each repo (whenever a lockfile \
             changes):\n",
        );
        for step in auto_steps {
            task.push_str(&format!("- {step}\n"));
        }
        task.push('\n');
    }
    if let Some(current) = settings
        .sandbox_setup_commands
        .as_deref()
        .filter(|c| !c.trim().is_empty())
    {
        task.push_str(&format!(
            "The project's current setup commands (improve on them; keep what still \
             works):\n```sh\n{}\n```\n\n",
            current.trim()
        ));
    }
    let current_roles: Vec<_> = settings
        .sandbox_role_setup
        .iter()
        .filter(|(_, commands)| !commands.trim().is_empty())
        .collect();
    if !current_roles.is_empty() {
        task.push_str("The project's current role setups (kept unless you replace them):\n");
        for (role, commands) in current_roles {
            task.push_str(&format!("\n{role}:\n```sh\n{}\n```\n", commands.trim()));
        }
        task.push('\n');
    }
    if !requests.is_empty() {
        task.push_str(
            "Agents working on this project found these missing in their sandbox and asked \
             for them. Answer each one: add what installs it to the role's setup file (or to \
             setup.sh if every role needs it), or explain in your summary why it can't be \
             done without the user (system packages need a project Dockerfile, hosts need \
             allowing).\n",
        );
        for request in requests {
            let role = request
                .role
                .0
                .map(|r| r.to_string())
                .unwrap_or_else(|| "unknown role".into());
            task.push_str(&format!(
                "\n- **{}** (asked by a {role} agent): {}\n",
                request.need,
                request.reason.replace('\n', " ")
            ));
            if let Some(suggestion) = &request.suggestion {
                task.push_str(&format!(
                    "  Its suggestion: `{}`\n",
                    suggestion.replace('\n', "; ")
                ));
            }
        }
        task.push('\n');
    }
    task.push_str(
        "Read the manifests, lockfiles, toolchain pins, CI workflows (.github/workflows, \
         .gitlab-ci.yml), Makefiles, Dockerfiles and READMEs. Use the web only for what the \
         repos don't answer. You cannot run commands, so be conservative: only write \
         commands you are confident work.\n\n\
         Write exactly these files:\n\n\
         1. /kanbain/out/setup.sh: bash, run from /workspace as `node` after KanbAIn's \
         installs, with `bash -e`. Only what those installs miss: code generation, building \
         internal packages other packages need, `cargo install`/`pip install` of tools the \
         tests need, copying `.env.example` to `.env`, and so on. Address repos as \
         /workspace/<repo>. Make it idempotent. Leave it empty if nothing is needed. No \
         `sudo`, no `apt-get`, no services that must keep running.\n\n\
         2. /kanbain/out/commands.json, exactly this shape:\n\
         {\"test\": [\"cd /workspace/<repo> && <command>\"], \
         \"run\": [\"cd /workspace/<repo> && <command>\"], \
         \"skip_auto_install\": false}\n\
         `test`: the commands that check a change (unit tests, type checks, lint), fastest \
         first. `run`: how to build or start the project (dev server, CLI). Set \
         `skip_auto_install` to true only when KanbAIn's installs above are wrong for this \
         project and setup.sh installs the dependencies itself.\n\n\
         3. /kanbain/out/roles/<role>.sh, only for roles that need something the others \
         don't (a headless browser or e2e runner for `tester`, say). Same rules as \
         setup.sh; it runs after setup.sh in that role's runs only. Roles: ",
    );
    task.push_str(
        &project_setup::SETUP_ROLES
            .iter()
            .map(|r| format!("`{r}`"))
            .collect::<Vec<_>>()
            .join(", "),
    );
    task.push_str(
        ". Browsers for Playwright install without root with `npx playwright install \
         chromium` (its system libraries may still be missing, and downloads from outside \
         the package registries need their host allowed; say so). Write an empty file \
         to drop a current role setup; leave a role out to keep it as it is.\n\n\
         4. /kanbain/out/environment.md: short notes for the agents that work on this \
         project: how the repos fit together, how to build and test, required environment \
         variables, services that are not available in the sandbox (databases, browsers) and \
         how tests cope without them, and known traps. Facts from the repos only; no \
         generic advice.\n\n\
         Then answer with a short summary for the user: what you found, and anything they \
         must do themselves (domains to allow, a project Dockerfile for system packages, \
         secrets).\n",
    );
    task
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    text.chars().take(max).collect::<String>() + "\n# … truncated by KanbAIn\n"
}

/// `test`/`run` as a list of non-empty commands. A bare string counts as one.
fn command_list(value: Option<&serde_json::Value>) -> Vec<String> {
    let items: Vec<&serde_json::Value> = match value {
        Some(serde_json::Value::Array(items)) => items.iter().collect(),
        Some(v @ serde_json::Value::String(_)) => vec![v],
        _ => Vec::new(),
    };
    let mut out: Vec<String> = Vec::new();
    for item in items {
        let Some(cmd) = item.as_str().map(str::trim).filter(|c| !c.is_empty()) else {
            continue;
        };
        if !out.iter().any(|c| c == cmd) && out.len() < MAX_COMMANDS {
            out.push(cmd.to_string());
        }
    }
    out
}

/// The proposal from the job's out files. Errors when the agent handed in
/// none of them; a missing piece is just empty.
pub fn read_proposal(
    files: &serde_json::Map<String, serde_json::Value>,
) -> Result<SetupProposal, String> {
    let file = |name: &str| files.get(name).and_then(|v| v.as_str()).map(str::trim);
    let script = file("setup.sh");
    let commands = file("commands.json");
    let notes = file("environment.md");
    let role_setup = role_scripts(files);
    if script.is_none() && commands.is_none() && notes.is_none() && role_setup.is_empty() {
        return Err(
            "The setup agent wrote none of setup.sh, commands.json, roles/<role>.sh and \
             environment.md"
                .to_string(),
        );
    }
    let commands = match commands.filter(|c| !c.is_empty()) {
        Some(text) => extract_json_object(text)
            .ok_or_else(|| "commands.json is not a JSON object".to_string())?,
        None => serde_json::Value::Null,
    };
    Ok(SetupProposal {
        setup_script: script_body(script.unwrap_or_default()),
        test_commands: command_list(commands.get("test")),
        run_commands: command_list(commands.get("run")),
        environment_md: truncate(notes.unwrap_or_default(), MAX_NOTES_CHARS),
        skip_auto_install: commands
            .get("skip_auto_install")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        role_setup,
    })
}

/// A script without its shebang, trimmed and capped. A shebang-only script
/// is an empty one; don't make the user review it.
fn script_body(script: &str) -> String {
    let body = script
        .lines()
        .filter(|l| !l.starts_with("#!"))
        .collect::<Vec<_>>()
        .join("\n");
    truncate(body.trim(), MAX_SCRIPT_CHARS)
}

/// `roles/<role>.sh` files for roles that have workspace runs. An empty one
/// stays in the map: it drops that role's current setup.
fn role_scripts(files: &serde_json::Map<String, serde_json::Value>) -> BTreeMap<AgentRole, String> {
    files
        .iter()
        .filter_map(|(path, contents)| {
            let name = path.strip_prefix("roles/")?.strip_suffix(".sh")?;
            let role: AgentRole = name.parse().ok()?;
            project_setup::SETUP_ROLES
                .contains(&role)
                .then(|| (role, script_body(contents.as_str().unwrap_or_default())))
        })
        .collect()
}

/// The proposal with the current role setups it doesn't mention kept, so a
/// rescan for an unrelated reason doesn't silently drop the tester's
/// browser.
pub fn keep_unmentioned_roles(
    mut proposal: SetupProposal,
    settings: &ProjectSettings,
) -> SetupProposal {
    for (role, commands) in &settings.sandbox_role_setup {
        if !commands.trim().is_empty() {
            proposal
                .role_setup
                .entry(*role)
                .or_insert_with(|| commands.trim().to_string());
        }
    }
    proposal
}

/// The settings with an approved proposal applied.
pub fn apply_proposal(
    settings: &ProjectSettings,
    proposal: &SetupProposal,
    stack_hash: &str,
) -> ProjectSettings {
    let non_empty = |s: &str| Some(s.trim().to_string()).filter(|s| !s.is_empty());
    let commands = |list: &[String]| -> Vec<String> {
        list.iter()
            .map(|c| c.trim().to_string())
            .filter(|c| !c.is_empty())
            .collect()
    };
    ProjectSettings {
        sandbox_setup_commands: non_empty(&proposal.setup_script),
        sandbox_setup_skip_install: proposal.skip_auto_install,
        sandbox_test_commands: commands(&proposal.test_commands),
        sandbox_run_commands: commands(&proposal.run_commands),
        sandbox_environment_notes: non_empty(&proposal.environment_md),
        sandbox_setup_stack: Some(stack_hash.to_string()),
        sandbox_role_setup: proposal
            .role_setup
            .iter()
            .filter_map(|(role, commands)| Some((*role, non_empty(commands)?)))
            .collect(),
        ..settings.clone()
    }
}

#[derive(Clone)]
pub struct SetupScanService {
    db: DBService,
    container: LocalContainerService,
    git: GitService,
    jobs: AgentJobs,
}

impl SetupScanService {
    pub fn new(
        db: DBService,
        container: LocalContainerService,
        git: GitService,
        jobs: AgentJobs,
    ) -> Self {
        Self {
            db,
            container,
            git,
            jobs,
        }
    }

    async fn project(&self, project_id: Uuid) -> Result<Project, SetupScanError> {
        Project::find_by_id(&self.db.pool, project_id)
            .await?
            .ok_or(SetupScanError::NotFound)
    }

    /// The project's repos as `(name, main checkout)`, plus the branch each
    /// is scanned on.
    async fn repos(
        &self,
        project_id: Uuid,
    ) -> Result<Vec<(String, PathBuf, String)>, SetupScanError> {
        let repos = ProjectRepo::find_repos_for_project(&self.db.pool, project_id).await?;
        let mut out = Vec::new();
        for repo in repos {
            let branch = match &repo.default_target_branch {
                Some(branch) => branch.clone(),
                None => self
                    .git
                    .get_current_branch(&repo.path)
                    .map_err(|e| SetupScanError::Invalid(format!("{}: {e}", repo.name)))?,
            };
            out.push((repo.name.clone(), repo.path.clone(), branch));
        }
        Ok(out)
    }

    fn roots(repos: &[(String, PathBuf, String)]) -> Vec<(String, PathBuf)> {
        repos
            .iter()
            .map(|(name, path, _)| (name.clone(), path.clone()))
            .collect()
    }

    pub async fn view(&self, project_id: Uuid) -> Result<SetupScanView, SetupScanError> {
        let project = self.project(project_id).await?;
        let roots = Self::roots(&self.repos(project_id).await.unwrap_or_default());
        let stack_changed = project
            .settings
            .sandbox_setup_stack
            .as_deref()
            .is_some_and(|approved| approved != stack_fingerprint(&roots));
        Ok(SetupScanView {
            scan: ProjectSetupScan::find(&self.db.pool, project_id).await?,
            stack_changed,
            auto_steps: auto_steps(&roots, &project.settings),
            requests: SetupRequest::find_pending(&self.db.pool, project_id).await?,
        })
    }

    /// Starts a scan in the background.
    pub async fn start(&self, project_id: Uuid) -> Result<ProjectSetupScan, SetupScanError> {
        let pool = &self.db.pool;
        let project = self.project(project_id).await?;
        let repos = self.repos(project_id).await?;
        if repos.is_empty() {
            return Err(SetupScanError::Invalid(
                "The project has no repositories to scan; link one first".to_string(),
            ));
        }
        if !self.container.sandbox().enabled().await {
            return Err(SetupScanError::Invalid(
                "The setup agent runs in the sandbox; turn it on in Settings → Sandbox".to_string(),
            ));
        }
        let stack = stack_fingerprint(&Self::roots(&repos));
        // The scan answers what is pending now; requests that arrive while
        // it runs wait for the next one.
        let requests = SetupRequest::find_pending(pool, project_id).await?;
        let request_ids: Vec<Uuid> = requests.iter().map(|r| r.id).collect();
        if !ProjectSetupScan::begin(pool, project_id, &stack, &request_ids).await? {
            return Err(SetupScanError::AlreadyScanning);
        }
        let service = self.clone();
        tokio::spawn(async move {
            if let Err(e) = service.scan(project, repos, requests).await {
                tracing::warn!("Setup scan of project {project_id} failed: {e}");
                let _ = ProjectSetupScan::set_failed(&service.db.pool, project_id, &e.to_string())
                    .await;
            }
        });
        ProjectSetupScan::find(pool, project_id)
            .await?
            .ok_or(SetupScanError::NotFound)
    }

    async fn scan(
        &self,
        project: Project,
        repos: Vec<(String, PathBuf, String)>,
        requests: Vec<SetupRequest>,
    ) -> Result<(), SetupScanError> {
        let pool = &self.db.pool;
        let project_id = project.id;
        let names: Vec<String> = repos.iter().map(|(name, _, _)| name.clone()).collect();
        let steps = auto_steps(&Self::roots(&repos), &project.settings);
        let task = scan_task(&project.name, &names, &steps, &project.settings, &requests);
        let name = project.name.clone();
        let settings = project.settings.clone();
        let (job_id, result) = self
            .jobs
            .run(JobRequest {
                kind: JOB_KIND_SETUP,
                role: AgentRole::Setup,
                project: Some(project),
                issue_id: None,
                context: Vec::new(),
                task,
                repos,
            })
            .await;
        if let Some(job_id) = job_id {
            ProjectSetupScan::set_job(pool, project_id, job_id).await?;
        }
        let output = result.map_err(SetupScanError::Invalid)?;
        let proposal = keep_unmentioned_roles(
            read_proposal(&output.files).map_err(SetupScanError::Invalid)?,
            &settings,
        );
        let summary = Some(output.message.trim()).filter(|s| !s.is_empty());
        ProjectSetupScan::set_ready(pool, project_id, &proposal, summary).await?;
        self.container
            .notification_service()
            .notify(
                "Sandbox setup ready for review",
                &format!(
                    "The setup agent proposed a sandbox setup for \"{name}\". Review it in \
                     the project settings."
                ),
                None,
            )
            .await;
        Ok(())
    }

    /// Approves the ready proposal, with the user's edits, and applies it to
    /// the project settings.
    pub async fn approve(
        &self,
        project_id: Uuid,
        proposal: &SetupProposal,
    ) -> Result<Project, SetupScanError> {
        let pool = &self.db.pool;
        let project = self.project(project_id).await?;
        let scan = ProjectSetupScan::find(pool, project_id)
            .await?
            .filter(|s| s.status == SetupScanStatus::Ready)
            .ok_or_else(|| SetupScanError::Invalid("No setup proposal to approve".into()))?;
        if !ProjectSetupScan::decide(pool, project_id, SetupScanStatus::Approved, Some(proposal))
            .await?
        {
            return Err(SetupScanError::Invalid(
                "The proposal changed meanwhile; reload it".to_string(),
            ));
        }
        let settings = apply_proposal(&project.settings, proposal, &scan.stack_hash);
        let project = Project::update(
            pool,
            project_id,
            &UpdateProject {
                settings: Some(settings),
                ..Default::default()
            },
        )
        .await?;
        SetupRequest::decide(
            pool,
            project_id,
            &scan.request_ids.0,
            SetupRequestStatus::Resolved,
        )
        .await?;
        Ok(project)
    }

    /// Drops a pending setup request without setting anything up.
    pub async fn dismiss_request(
        &self,
        project_id: Uuid,
        request_id: Uuid,
    ) -> Result<(), SetupScanError> {
        let changed = SetupRequest::decide(
            &self.db.pool,
            project_id,
            &[request_id],
            SetupRequestStatus::Dismissed,
        )
        .await?;
        if changed == 0 {
            return Err(SetupScanError::Invalid(
                "That setup request is not pending in this project".into(),
            ));
        }
        Ok(())
    }

    pub async fn discard(&self, project_id: Uuid) -> Result<(), SetupScanError> {
        if !ProjectSetupScan::decide(&self.db.pool, project_id, SetupScanStatus::Discarded, None)
            .await?
        {
            return Err(SetupScanError::Invalid(
                "No setup proposal to discard".into(),
            ));
        }
        Ok(())
    }

    /// Starts a scan if the project's stack moved since its setup was
    /// approved and no scan has looked at this stack yet. Fire and forget;
    /// called when an agent run starts.
    pub fn spawn_refresh_if_stale(&self, project_id: Uuid) {
        let service = self.clone();
        tokio::spawn(async move {
            if let Err(e) = service.refresh_if_stale(project_id).await {
                tracing::debug!("Setup rescan check for {project_id} skipped: {e}");
            }
        });
    }

    async fn refresh_if_stale(&self, project_id: Uuid) -> Result<(), SetupScanError> {
        let project = self.project(project_id).await?;
        let Some(approved) = project.settings.sandbox_setup_stack.as_deref() else {
            return Ok(());
        };
        let current = stack_fingerprint(&Self::roots(&self.repos(project_id).await?));
        let latest = ProjectSetupScan::find(&self.db.pool, project_id).await?;
        if !needs_rescan(approved, &current, latest.as_ref()) {
            return Ok(());
        }
        tracing::info!(
            "The stack of project {} changed; rescanning its setup",
            project.name
        );
        self.start(project_id).await?;
        Ok(())
    }
}

/// Whether a stack move warrants a new scan: the fingerprint left the
/// approved one and the latest scan didn't already look at this exact stack.
fn needs_rescan(approved: &str, current: &str, latest: Option<&ProjectSetupScan>) -> bool {
    approved != current
        && !latest.is_some_and(|scan| {
            scan.status == SetupScanStatus::Scanning || scan.stack_hash == current
        })
}

/// KanbAIn's own installs for these repos, as `[repo] command`.
fn auto_steps(roots: &[(String, PathBuf)], settings: &ProjectSettings) -> Vec<String> {
    // The scan judges the lockfile installs themselves, so list them even
    // when the approved setup switched them off.
    let settings = ProjectSettings {
        sandbox_setup_skip_install: false,
        ..settings.clone()
    };
    project_setup::plan(roots, Some(&settings), None)
        .steps
        .into_iter()
        .map(|step| format!("[{}] {}", step.repo, step.command))
        .collect()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn files(pairs: &[(&str, &str)]) -> serde_json::Map<String, serde_json::Value> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), json!(v)))
            .collect()
    }

    #[test]
    fn reads_a_full_proposal() {
        let proposal = read_proposal(&files(&[
            (
                "setup.sh",
                "#!/usr/bin/env bash\ncd /workspace/web && pnpm run codegen\n",
            ),
            (
                "commands.json",
                r#"{"test": ["cd /workspace/web && pnpm test", "  ", "cd /workspace/web && pnpm test"],
                    "run": "cd /workspace/web && pnpm dev", "skip_auto_install": true}"#,
            ),
            ("environment.md", "  # Notes\nNo database here.\n"),
        ]))
        .unwrap();
        assert_eq!(
            proposal,
            SetupProposal {
                setup_script: "cd /workspace/web && pnpm run codegen".into(),
                test_commands: vec!["cd /workspace/web && pnpm test".into()],
                run_commands: vec!["cd /workspace/web && pnpm dev".into()],
                environment_md: "# Notes\nNo database here.".into(),
                skip_auto_install: true,
                role_setup: BTreeMap::new(),
            }
        );
    }

    #[test]
    fn reads_role_scripts_for_workspace_roles_only() {
        let proposal = read_proposal(&files(&[
            (
                "roles/tester.sh",
                "#!/bin/bash\ncd /workspace/web && npx playwright install chromium\n",
            ),
            ("roles/reviewer.sh", "\n"),
            // Headless jobs never run role setup; made-up roles neither.
            ("roles/triage.sh", "echo no"),
            ("roles/wizard.sh", "echo no"),
            ("tester.sh", "echo no"),
        ]))
        .unwrap();
        assert_eq!(
            proposal.role_setup,
            BTreeMap::from([
                (
                    AgentRole::Tester,
                    "cd /workspace/web && npx playwright install chromium".to_string()
                ),
                (AgentRole::Reviewer, String::new()),
            ])
        );
        assert!(proposal.setup_script.is_empty());
    }

    #[test]
    fn rescans_keep_role_setups_they_dont_mention() {
        let settings = ProjectSettings {
            sandbox_role_setup: BTreeMap::from([
                (AgentRole::Tester, "npx playwright install chromium".into()),
                (AgentRole::Coder, "cargo install just".into()),
            ]),
            ..Default::default()
        };
        let proposal = keep_unmentioned_roles(
            SetupProposal {
                role_setup: BTreeMap::from([(AgentRole::Coder, String::new())]),
                ..Default::default()
            },
            &settings,
        );
        // Kept: the agent said nothing about testers. Dropped: it emptied
        // the coder setup on purpose.
        assert_eq!(
            proposal
                .role_setup
                .get(&AgentRole::Tester)
                .map(String::as_str),
            Some("npx playwright install chromium")
        );
        assert_eq!(
            proposal
                .role_setup
                .get(&AgentRole::Coder)
                .map(String::as_str),
            Some("")
        );
        let applied = apply_proposal(&settings, &proposal, "s");
        assert_eq!(
            applied.sandbox_role_setup,
            BTreeMap::from([(
                AgentRole::Tester,
                "npx playwright install chromium".to_string()
            )])
        );
    }

    #[test]
    fn partial_and_missing_proposals() {
        let notes_only = read_proposal(&files(&[("environment.md", "hi")])).unwrap();
        assert_eq!(notes_only.environment_md, "hi");
        assert!(notes_only.setup_script.is_empty());
        assert!(notes_only.test_commands.is_empty());

        assert!(read_proposal(&files(&[])).is_err());
        assert!(read_proposal(&files(&[("plan.json", "{}")])).is_err());
        assert!(read_proposal(&files(&[("commands.json", "not json")])).is_err());
    }

    #[test]
    fn fingerprint_tracks_stack_not_dependencies() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let repos = vec![("web".to_string(), root.to_path_buf())];
        std::fs::write(root.join("package.json"), "{}").unwrap();
        std::fs::write(root.join("pnpm-lock.yaml"), "a: 1").unwrap();
        let base = stack_fingerprint(&repos);

        // A dependency bump is not a stack change.
        std::fs::write(root.join("pnpm-lock.yaml"), "a: 2").unwrap();
        std::fs::write(root.join("package.json"), r#"{"x": 1}"#).unwrap();
        assert_eq!(stack_fingerprint(&repos), base);

        // A new manifest is.
        std::fs::write(root.join("Cargo.toml"), "").unwrap();
        let with_rust = stack_fingerprint(&repos);
        assert_ne!(with_rust, base);

        // So is a moved version pin.
        std::fs::write(root.join(".nvmrc"), "18").unwrap();
        let node18 = stack_fingerprint(&repos);
        std::fs::write(root.join(".nvmrc"), "22\n").unwrap();
        assert_ne!(stack_fingerprint(&repos), node18);

        // Renaming the repo changes it; repo order doesn't.
        let other = tempfile::tempdir().unwrap();
        let a = vec![
            ("a".to_string(), root.to_path_buf()),
            ("b".to_string(), other.path().to_path_buf()),
        ];
        let b = vec![a[1].clone(), a[0].clone()];
        assert_eq!(stack_fingerprint(&a), stack_fingerprint(&b));
        assert_eq!(stack_fingerprint(&repos).len(), 16);
    }

    fn scan(status: SetupScanStatus, stack: &str) -> ProjectSetupScan {
        ProjectSetupScan {
            project_id: Uuid::nil(),
            status,
            job_id: None,
            setup_script: None,
            test_commands: sqlx::types::Json(vec![]),
            run_commands: sqlx::types::Json(vec![]),
            environment_md: None,
            skip_auto_install: false,
            role_setup: sqlx::types::Json(BTreeMap::new()),
            request_ids: sqlx::types::Json(vec![]),
            summary: None,
            stack_hash: stack.into(),
            error: None,
            started_at: chrono::Utc::now(),
            finished_at: None,
            decided_at: None,
        }
    }

    #[test]
    fn rescans_once_per_new_stack() {
        assert!(!needs_rescan("a", "a", None));
        assert!(needs_rescan("a", "b", None));
        assert!(needs_rescan(
            "a",
            "b",
            Some(&scan(SetupScanStatus::Approved, "a"))
        ));
        // Already looked at (or looking at) this stack.
        for status in [
            SetupScanStatus::Ready,
            SetupScanStatus::Failed,
            SetupScanStatus::Discarded,
        ] {
            assert!(!needs_rescan("a", "b", Some(&scan(status, "b"))));
        }
        assert!(!needs_rescan(
            "a",
            "b",
            Some(&scan(SetupScanStatus::Scanning, "a"))
        ));
    }

    #[test]
    fn approval_applies_to_settings() {
        let before = ProjectSettings {
            sandbox_setup_commands: Some("old".into()),
            sandbox_allowed_domains: vec!["example.com".into()],
            ..Default::default()
        };
        let after = apply_proposal(
            &before,
            &SetupProposal {
                setup_script: "  ".into(),
                test_commands: vec![" cargo test ".into(), "".into()],
                run_commands: vec![],
                environment_md: "notes".into(),
                skip_auto_install: true,
                role_setup: BTreeMap::new(),
            },
            "abc",
        );
        assert_eq!(after.sandbox_setup_commands, None);
        assert!(after.sandbox_setup_skip_install);
        assert_eq!(after.sandbox_test_commands, vec!["cargo test"]);
        assert_eq!(after.sandbox_environment_notes.as_deref(), Some("notes"));
        assert_eq!(after.sandbox_setup_stack.as_deref(), Some("abc"));
        assert_eq!(after.sandbox_allowed_domains, vec!["example.com"]);
    }

    #[test]
    fn task_lists_repos_installs_and_outputs() {
        let task = scan_task(
            "Shop",
            &["web".into(), "api".into()],
            &["[web] pnpm install --frozen-lockfile".into()],
            &ProjectSettings {
                sandbox_setup_commands: Some("cargo install sqlx-cli".into()),
                sandbox_role_setup: BTreeMap::from([(
                    AgentRole::Tester,
                    "npx playwright install chromium".into(),
                )]),
                ..Default::default()
            },
            &[request()],
        );
        assert!(task.contains("- /workspace/web\n- /workspace/api\n"));
        assert!(task.contains("- [web] pnpm install --frozen-lockfile"));
        assert!(task.contains("cargo install sqlx-cli"));
        for out in [
            "setup.sh",
            "commands.json",
            "environment.md",
            "roles/<role>.sh",
        ] {
            assert!(task.contains(&format!("/kanbain/out/{out}")), "{out}");
        }
        assert!(task.contains("tester:\n```sh\nnpx playwright install chromium\n```"));
        assert!(task.contains("- **Headless Chromium** (asked by a tester agent): WebGL checks"));
        assert!(task.contains("Its suggestion: `npx playwright install chromium`"));
        assert!(task.contains("`conflict_resolver`"));

        let quiet = scan_task("Shop", &[], &[], &ProjectSettings::default(), &[]);
        assert!(!quiet.contains("asked for them"));
        assert!(!quiet.contains("current role setups"));
    }

    fn request() -> SetupRequest {
        SetupRequest {
            id: Uuid::new_v4(),
            project_id: Uuid::nil(),
            issue_id: None,
            execution_process_id: None,
            role: db::models::setup_request::RoleColumn(Some(AgentRole::Tester)),
            agent: Some("tester".into()),
            need: "Headless Chromium".into(),
            reason: "WebGL\nchecks".into(),
            suggestion: Some("npx playwright install chromium".into()),
            status: SetupRequestStatus::Pending,
            created_at: chrono::Utc::now(),
            resolved_at: None,
        }
    }
}
