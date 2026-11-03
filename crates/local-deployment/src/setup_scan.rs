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

use std::path::PathBuf;

use db::{
    DBService,
    models::{
        agent_job::JOB_KIND_SETUP,
        project::{Project, ProjectRepo, ProjectSettings, UpdateProject},
        project_setup_scan::{ProjectSetupScan, SetupProposal, SetupScanStatus},
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
         3. /kanbain/out/environment.md: short notes for the agents that work on this \
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
    if script.is_none() && commands.is_none() && notes.is_none() {
        return Err(
            "The setup agent wrote none of setup.sh, commands.json and environment.md".to_string(),
        );
    }
    let commands = match commands.filter(|c| !c.is_empty()) {
        Some(text) => extract_json_object(text)
            .ok_or_else(|| "commands.json is not a JSON object".to_string())?,
        None => serde_json::Value::Null,
    };
    // A shebang-only script is an empty one; don't make the user review it.
    let script = script
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.starts_with("#!"))
        .collect::<Vec<_>>()
        .join("\n");
    Ok(SetupProposal {
        setup_script: truncate(script.trim(), MAX_SCRIPT_CHARS),
        test_commands: command_list(commands.get("test")),
        run_commands: command_list(commands.get("run")),
        environment_md: truncate(notes.unwrap_or_default(), MAX_NOTES_CHARS),
        skip_auto_install: commands
            .get("skip_auto_install")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
    })
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
        if !ProjectSetupScan::begin(pool, project_id, &stack).await? {
            return Err(SetupScanError::AlreadyScanning);
        }
        let service = self.clone();
        tokio::spawn(async move {
            if let Err(e) = service.scan(project, repos).await {
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
    ) -> Result<(), SetupScanError> {
        let pool = &self.db.pool;
        let project_id = project.id;
        let names: Vec<String> = repos.iter().map(|(name, _, _)| name.clone()).collect();
        let steps = auto_steps(&Self::roots(&repos), &project.settings);
        let task = scan_task(&project.name, &names, &steps, &project.settings);
        let name = project.name.clone();
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
        let proposal = read_proposal(&output.files).map_err(SetupScanError::Invalid)?;
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
        Ok(Project::update(
            pool,
            project_id,
            &UpdateProject {
                settings: Some(settings),
                ..Default::default()
            },
        )
        .await?)
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
    project_setup::plan(roots, Some(&settings))
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
            }
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
                ..Default::default()
            },
        );
        assert!(task.contains("- /workspace/web\n- /workspace/api\n"));
        assert!(task.contains("- [web] pnpm install --frozen-lockfile"));
        assert!(task.contains("cargo install sqlx-cli"));
        for out in ["setup.sh", "commands.json", "environment.md"] {
            assert!(task.contains(&format!("/kanbain/out/{out}")), "{out}");
        }
    }
}
