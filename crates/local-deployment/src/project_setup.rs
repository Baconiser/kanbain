//! Project setup in the sandbox: KanbAIn itself (not the agent, so no bash
//! allowlist games) installs a project's dependencies after the code is
//! synced and before the agent starts.
//!
//! The setup script is generated from the repos' lockfiles plus the
//! project's own extra commands. Every input (lockfiles, toolchain files and
//! the scripts themselves) is hashed inside the container and compared with
//! `/kanbain/state/setup.sha`; setup only runs when that hash moved. That
//! also catches an agent adding a dependency mid-run: the next run sees the
//! new lockfile and reinstalls. The hash lives in the container's `/kanbain`
//! volume, never in the repo, so it dies together with the `node_modules` it
//! vouches for.
//!
//! Package caches sit on per-project named volumes ([`cache_volumes`]), so a
//! fresh container installs near-offline. Host `node_modules`/`target` are
//! deliberately NOT mounted: wrong-platform native binaries on macOS and
//! Windows, agents mutating the real checkout, and parallel subtasks on
//! different lockfiles clobbering each other. Hard pass.

use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use db::models::project::{Project, ProjectSettings};
use sandbox::{AGENT_USER, ExecOptions, FileEntry, NamedVolume, Sandbox, WORKSPACE_DIR};

/// Root-owned: the agent can read and run the scripts but not rewrite them
/// or forge the hash.
pub const STATE_DIR: &str = "/kanbain/state";
const SETUP_SCRIPT: &str = "setup.sh";
const PROJECT_SCRIPT: &str = "project-setup.sh";
const HASH_FILE: &str = "setup.sha";

/// Package caches (corepack, pnpm store, yarn, pip, npm). The base image
/// points the tools here (`docker/agent/Dockerfile`).
pub const CACHE_DIR: &str = "/home/node/.kanbain-cache";
/// `CARGO_TARGET_DIR` when the project shares one target dir across runs.
pub const SHARED_TARGET_DIR: &str = "/home/node/.kanbain-target";

/// A cold `cargo fetch` plus a big pnpm install fits easily; a hung
/// postinstall doesn't get to eat the whole run.
const SETUP_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// Files whose change means "install again". Checked at each repo root.
const INPUT_FILES: &[&str] = &[
    "pnpm-lock.yaml",
    "package-lock.json",
    "yarn.lock",
    "Cargo.lock",
    "rust-toolchain.toml",
    "rust-toolchain",
    "requirements.txt",
    "pyproject.toml",
    "go.sum",
];

/// One install command, run in `/workspace/<repo>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetupStep {
    pub repo: String,
    pub command: String,
}

/// What setup will do for a run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SetupPlan {
    pub steps: Vec<SetupStep>,
    /// The project's extra commands, run from `/workspace` after the installs.
    pub project_commands: Option<String>,
    /// Every repo of the run: their lockfiles feed the hash.
    pub repos: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SetupOutcome {
    /// No lockfiles, no project commands.
    NothingToDo,
    /// The inputs match the last successful setup in this container.
    UpToDate,
    Ran,
    Failed {
        /// The steps that failed, as `[repo] command`.
        failed_steps: Vec<String>,
        /// Last lines of output, for the run log and the agent.
        tail: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetupReport {
    pub plan: SetupPlan,
    pub outcome: SetupOutcome,
}

impl SetupReport {
    /// Whether the JavaScript dependencies of `repo` are installed, so
    /// environment.md can drop its "node_modules is missing" warning.
    pub fn installed_js(&self, repo: &str) -> bool {
        matches!(self.outcome, SetupOutcome::Ran | SetupOutcome::UpToDate)
            && self.plan.steps.iter().any(|step| {
                step.repo == repo
                    && ["pnpm ", "npm ", "yarn "]
                        .iter()
                        .any(|tool| step.command.starts_with(tool))
            })
    }
}

/// Install commands for one repo, from the files at its root.
fn detect_steps(root: &Path) -> Vec<String> {
    let has = |name: &str| root.join(name).exists();
    let mut steps = Vec::new();

    if has("package.json") {
        // npm's cache goes to the cache volume explicitly: the default
        // `~/.npm` holds the image's warm npx cache for the agent CLIs.
        if has("pnpm-lock.yaml") {
            steps.push("pnpm install --frozen-lockfile".to_string());
        } else if has("yarn.lock") {
            steps.push("yarn install --frozen-lockfile".to_string());
        } else if has("package-lock.json") {
            steps.push(format!("npm ci --cache {CACHE_DIR}/npm"));
        } else if !has("bun.lock") && !has("bun.lockb") {
            steps.push(format!(
                "npm install --no-package-lock --cache {CACHE_DIR}/npm"
            ));
        }
    }

    if has("rust-toolchain.toml") || has("rust-toolchain") {
        // rustup >= 1.28 installs the pinned toolchain with a bare
        // `toolchain install`; older ones do it as a side effect of `show`.
        steps.push("rustup toolchain install || rustup show".to_string());
    }
    if has("Cargo.toml") {
        steps.push("cargo fetch".to_string());
    }

    // Debian's pip refuses to touch the system site-packages (PEP 668).
    // `--user` lands in ~/.local, which python finds on its own.
    if has("requirements.txt") {
        steps.push("pip install --user --break-system-packages -r requirements.txt".to_string());
    } else if has("pyproject.toml") {
        steps.push("pip install --user --break-system-packages -e .".to_string());
    }

    if has("go.mod") {
        // Go isn't in the base image; only a project Dockerfile brings it.
        steps.push("if command -v go >/dev/null; then go mod download; fi".to_string());
    }
    steps
}

/// The setup for a run's repos, `(name, host worktree)` pairs.
pub fn plan(repos: &[(String, PathBuf)], settings: Option<&ProjectSettings>) -> SetupPlan {
    let auto_install = !settings.is_some_and(|s| s.sandbox_setup_skip_install);
    let steps = if auto_install {
        repos
            .iter()
            .flat_map(|(name, root)| {
                detect_steps(root).into_iter().map(|command| SetupStep {
                    repo: name.clone(),
                    command,
                })
            })
            .collect()
    } else {
        Vec::new()
    };
    let project_commands = settings
        .and_then(|s| s.sandbox_setup_commands.as_deref())
        .map(str::trim)
        .filter(|c| !c.is_empty())
        .map(str::to_string);
    SetupPlan {
        steps,
        project_commands,
        repos: repos.iter().map(|(name, _)| name.clone()).collect(),
    }
}

/// Single-quote for sh.
fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r#"'\''"#))
}

impl SetupPlan {
    pub fn is_empty(&self) -> bool {
        self.steps.is_empty() && self.project_commands.is_none()
    }

    /// `setup.sh`. Every step runs even after one fails, so one broken
    /// postinstall doesn't leave the other repos uninstalled; the exit code
    /// says whether all of them made it.
    pub fn script(&self) -> String {
        let mut script = String::from(
            "#!/usr/bin/env bash\n\
             # Generated by KanbAIn. Runs before the agent whenever a dependency\n\
             # file or this script changes.\n\
             set -u\n\
             failed=0\n\
             step() {\n  \
               local dir=\"$1\"; shift\n  \
               echo \"==> [$dir] $*\"\n  \
               if ! (cd \"$dir\" && eval \"$*\"); then\n    \
                 echo \"!! failed: [$dir] $*\"\n    \
                 failed=1\n  \
               fi\n\
             }\n",
        );
        for step in &self.steps {
            script.push_str(&format!(
                "step {} {}\n",
                sh_quote(&format!("{WORKSPACE_DIR}/{}", step.repo)),
                sh_quote(&step.command)
            ));
        }
        if self.project_commands.is_some() {
            script.push_str(&format!(
                "step {WORKSPACE_DIR} {}\n",
                sh_quote(&format!("bash -e {STATE_DIR}/{PROJECT_SCRIPT}"))
            ));
        }
        script.push_str("exit $failed\n");
        script
    }

    /// Prints the hash of every setup input, then the stored hash (if any).
    fn hash_command(&self) -> String {
        let files: Vec<String> = self
            .repos
            .iter()
            .flat_map(|repo| {
                INPUT_FILES
                    .iter()
                    .map(move |f| sh_quote(&format!("{repo}/{f}")))
            })
            .collect();
        format!(
            "cd {WORKSPACE_DIR} && {{ for f in {}; do if [ -f \"$f\" ]; then \
             printf '== %s\\n' \"$f\"; cat \"$f\"; fi; done; \
             cat {STATE_DIR}/{SETUP_SCRIPT} {STATE_DIR}/{PROJECT_SCRIPT}; }} \
             | sha256sum | cut -d' ' -f1; cat {STATE_DIR}/{HASH_FILE} 2>/dev/null || true",
            if files.is_empty() {
                "''".to_string()
            } else {
                files.join(" ")
            }
        )
    }
}

/// Run setup in `container` if its inputs changed. Never fails the run:
/// a broken setup is reported (log, environment.md) and the agent gets to
/// say what's missing instead of us blocking it outright. `env` is the
/// egress proxy (registries) plus anything the build needs.
pub async fn run(
    docker: &dyn Sandbox,
    container: &str,
    plan: SetupPlan,
    env: Vec<(String, String)>,
    log: &(dyn Fn(String) + Send + Sync),
) -> SetupReport {
    if plan.is_empty() {
        return SetupReport {
            plan,
            outcome: SetupOutcome::NothingToDo,
        };
    }
    let outcome = match run_inner(docker, container, &plan, env, log).await {
        Ok(outcome) => outcome,
        Err(e) => SetupOutcome::Failed {
            failed_steps: Vec::new(),
            tail: e.to_string(),
        },
    };
    if let SetupOutcome::Failed { failed_steps, tail } = &outcome {
        log(format!(
            "Project setup failed{}. The agent starts anyway.\n{tail}",
            if failed_steps.is_empty() {
                String::new()
            } else {
                format!(": {}", failed_steps.join(", "))
            }
        ));
    }
    SetupReport { plan, outcome }
}

async fn run_inner(
    docker: &dyn Sandbox,
    container: &str,
    plan: &SetupPlan,
    env: Vec<(String, String)>,
    log: &(dyn Fn(String) + Send + Sync),
) -> Result<SetupOutcome, sandbox::SandboxError> {
    let mut script = FileEntry::new(SETUP_SCRIPT, plan.script()).owned_by_root();
    script.mode = 0o755;
    // Always written, even empty, so removing the commands changes the hash.
    let project = FileEntry::new(
        PROJECT_SCRIPT,
        plan.project_commands.clone().unwrap_or_default(),
    )
    .owned_by_root();
    // `docker exec` runs with umask 000, so a plain mkdir hands the agent a
    // world-writable dir it could delete the hash from. Root owns it, so
    // chmod works even without capabilities.
    sandbox::exec_ok(
        docker,
        container,
        &[
            "sh",
            "-c",
            &format!("mkdir -p {STATE_DIR} && chmod 755 {STATE_DIR}"),
        ],
        &ExecOptions {
            user: Some("root".into()),
            ..Default::default()
        },
    )
    .await?;
    docker
        .copy_in(container, STATE_DIR, vec![script, project])
        .await?;

    let agent = ExecOptions {
        user: Some(AGENT_USER.into()),
        working_dir: Some(WORKSPACE_DIR.into()),
        ..Default::default()
    };
    let hashes = sandbox::exec_ok(
        docker,
        container,
        &["sh", "-c", &plan.hash_command()],
        &agent,
    )
    .await?
    .stdout_str();
    let mut lines = hashes.lines().map(str::trim);
    let current = lines.next().unwrap_or_default().to_string();
    let stored = lines.next().unwrap_or_default();
    if !current.is_empty() && current == stored {
        return Ok(SetupOutcome::UpToDate);
    }

    log("Dependency files changed; running project setup".into());
    let script_path = format!("{STATE_DIR}/{SETUP_SCRIPT}");
    let output = tokio::time::timeout(
        SETUP_TIMEOUT,
        docker.exec(
            container,
            &["bash", &script_path],
            &ExecOptions { env, ..agent },
        ),
    )
    .await
    .map_err(|_| {
        sandbox::SandboxError::Other(format!(
            "Project setup did not finish within {} minutes",
            SETUP_TIMEOUT.as_secs() / 60
        ))
    })??;

    let stdout = output.stdout_str();
    let stderr = String::from_utf8_lossy(&output.stderr);
    for line in stdout.lines().filter(|l| l.starts_with("==> ")) {
        log(line.to_string());
    }
    if output.exit_code != 0 {
        let failed_steps = stdout
            .lines()
            .filter_map(|l| l.strip_prefix("!! failed: "))
            .map(str::to_string)
            .collect();
        return Ok(SetupOutcome::Failed {
            failed_steps,
            tail: tail(&format!("{stdout}\n{stderr}"), 20),
        });
    }

    docker
        .copy_in(
            container,
            STATE_DIR,
            vec![FileEntry::new(HASH_FILE, format!("{current}\n")).owned_by_root()],
        )
        .await?;
    log("Project setup finished".into());
    Ok(SetupOutcome::Ran)
}

fn tail(text: &str, lines: usize) -> String {
    let all: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    all[all.len().saturating_sub(lines)..].join("\n")
}

/// Per-project cache volumes for a workspace container. The mount points
/// exist in the base image owned by the agent user, so Docker seeds fresh
/// volumes with the right owner (root can't chown later: all caps dropped).
pub fn cache_volumes(project: &Project, instance: &str, image: &str) -> Vec<NamedVolume> {
    let prefix = format!(
        "kanbain-cache-{}-{instance}",
        &project.id.simple().to_string()[..8]
    );
    // rustup is seeded from the image's toolchains; keyed by image so a new
    // image's stable isn't shadowed by a stale copy forever.
    let image_key = format!("{:08x}", sandbox::image::stable_hash(&[image]));
    let mut volumes = vec![
        (format!("{prefix}-deps"), CACHE_DIR.to_string()),
        (
            format!("{prefix}-cargo-registry"),
            "/home/node/.cargo/registry".to_string(),
        ),
        (
            format!("{prefix}-cargo-git"),
            "/home/node/.cargo/git".to_string(),
        ),
        (
            format!("{prefix}-rustup-{image_key}"),
            "/home/node/.rustup".to_string(),
        ),
    ];
    if project.settings.sandbox_shared_cargo_target {
        volumes.push((format!("{prefix}-target"), SHARED_TARGET_DIR.to_string()));
    }
    volumes
        .into_iter()
        .map(|(name, target)| NamedVolume { name, target })
        .collect()
}

/// Environment the project's builds need in every exec (setup and agent).
pub fn build_env(project: Option<&Project>) -> Vec<(String, String)> {
    match project {
        Some(p) if p.settings.sandbox_shared_cargo_target => {
            vec![("CARGO_TARGET_DIR".into(), SHARED_TARGET_DIR.into())]
        }
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo(dir: &tempfile::TempDir, name: &str) -> (String, PathBuf) {
        (name.to_string(), dir.path().to_path_buf())
    }

    #[test]
    fn detects_installs_per_stack() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("package.json"), "{}").unwrap();
        std::fs::write(root.join("pnpm-lock.yaml"), "").unwrap();
        std::fs::write(root.join("Cargo.toml"), "").unwrap();
        std::fs::write(root.join("rust-toolchain.toml"), "").unwrap();
        std::fs::write(root.join("requirements.txt"), "").unwrap();
        assert_eq!(
            detect_steps(root),
            vec![
                "pnpm install --frozen-lockfile",
                "rustup toolchain install || rustup show",
                "cargo fetch",
                "pip install --user --break-system-packages -r requirements.txt",
            ]
        );

        let npm = tempfile::tempdir().unwrap();
        std::fs::write(npm.path().join("package.json"), "{}").unwrap();
        std::fs::write(npm.path().join("package-lock.json"), "{}").unwrap();
        assert_eq!(
            detect_steps(npm.path()),
            vec![format!("npm ci --cache {CACHE_DIR}/npm")]
        );
        assert!(detect_steps(tempfile::tempdir().unwrap().path()).is_empty());
    }

    #[test]
    fn plan_honours_project_settings() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "").unwrap();
        let repos = [repo(&dir, "api")];

        let auto = plan(&repos, None);
        assert_eq!(
            auto.steps,
            vec![SetupStep {
                repo: "api".into(),
                command: "cargo fetch".into()
            }]
        );
        assert!(auto.project_commands.is_none());

        let settings = ProjectSettings {
            sandbox_setup_skip_install: true,
            sandbox_setup_commands: Some("  cargo install cargo-component \n".into()),
            ..Default::default()
        };
        let custom = plan(&repos, Some(&settings));
        assert!(custom.steps.is_empty());
        assert_eq!(
            custom.project_commands.as_deref(),
            Some("cargo install cargo-component")
        );
        assert!(!custom.is_empty());

        let blank = ProjectSettings {
            sandbox_setup_skip_install: true,
            sandbox_setup_commands: Some("   ".into()),
            ..Default::default()
        };
        assert!(plan(&repos, Some(&blank)).is_empty());
    }

    #[test]
    fn script_quotes_and_runs_every_step() {
        let plan = SetupPlan {
            steps: vec![SetupStep {
                repo: "it's".into(),
                command: "rustup toolchain install || rustup show".into(),
            }],
            project_commands: Some("echo hi".into()),
            repos: vec!["it's".into()],
        };
        let script = plan.script();
        assert!(
            script
                .contains(r#"step '/workspace/it'\''s' 'rustup toolchain install || rustup show'"#)
        );
        assert!(script.contains("step /workspace 'bash -e /kanbain/state/project-setup.sh'"));
        assert!(script.trim_end().ends_with("exit $failed"));

        let hash = plan.hash_command();
        assert!(hash.contains(r#"'it'\''s/pnpm-lock.yaml'"#));
        assert!(hash.contains("/kanbain/state/setup.sha"));
    }

    #[test]
    fn js_counts_as_installed_only_after_success() {
        let report = |outcome| SetupReport {
            plan: SetupPlan {
                steps: vec![SetupStep {
                    repo: "web".into(),
                    command: "pnpm install --frozen-lockfile".into(),
                }],
                ..Default::default()
            },
            outcome,
        };
        assert!(report(SetupOutcome::Ran).installed_js("web"));
        assert!(report(SetupOutcome::UpToDate).installed_js("web"));
        assert!(!report(SetupOutcome::Ran).installed_js("api"));
        assert!(
            !report(SetupOutcome::Failed {
                failed_steps: vec![],
                tail: String::new()
            })
            .installed_js("web")
        );
    }

    /// The generated script is valid bash and keeps going past a failure.
    #[test]
    fn script_runs_in_bash() {
        let Ok(bash) = which_bash() else { return };
        let dir = tempfile::tempdir().unwrap();
        let plan = SetupPlan {
            steps: vec![
                SetupStep {
                    repo: "a".into(),
                    command: "false".into(),
                },
                SetupStep {
                    repo: "a".into(),
                    command: "echo ok > done.txt".into(),
                },
            ],
            ..Default::default()
        };
        let script = plan
            .script()
            .replace(WORKSPACE_DIR, &dir.path().to_string_lossy());
        std::fs::create_dir(dir.path().join("a")).unwrap();
        let out = std::process::Command::new(bash)
            .arg("-c")
            .arg(&script)
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert_eq!(out.status.code(), Some(1));
        assert!(stdout.contains("!! failed: ["));
        assert!(dir.path().join("a/done.txt").exists());
    }

    /// Full loop against a real container: setup runs, the hash makes the
    /// second run a no-op, a lockfile change reruns it, and a failing step
    /// leaves no hash behind. Needs Docker and an agent image:
    /// `KANBAIN_SETUP_TEST_IMAGE=kanbain-agent:<tag> cargo test -- --ignored`.
    #[tokio::test]
    #[ignore]
    async fn setup_runs_once_per_input_change() {
        let image =
            std::env::var("KANBAIN_SETUP_TEST_IMAGE").expect("set KANBAIN_SETUP_TEST_IMAGE");
        let (docker, _) = sandbox::DockerSandbox::connect().await.unwrap();
        let spec = sandbox::ContainerSpec {
            name: "kanbain-setup-test".into(),
            image,
            volumes: vec![sandbox::KANBAIN_DIR.into()],
            named_volumes: vec![NamedVolume {
                name: "kanbain-setup-test-cache".into(),
                target: CACHE_DIR.into(),
            }],
            user: Some(AGENT_USER.into()),
            ..Default::default()
        };
        docker.remove(&spec.name).await.unwrap();
        let container = docker.create(&spec).await.unwrap();
        let agent = ExecOptions {
            user: Some(AGENT_USER.into()),
            ..Default::default()
        };
        sandbox::exec_ok(
            &docker,
            &container,
            &[
                "sh",
                "-c",
                "mkdir -p /workspace/app && echo v1 > /workspace/app/go.sum",
            ],
            &agent,
        )
        .await
        .unwrap();
        // The cache volume is seeded with the image's owner, so it's writable.
        sandbox::exec_ok(
            &docker,
            &container,
            &["touch", &format!("{CACHE_DIR}/probe")],
            &agent,
        )
        .await
        .unwrap();

        let make_plan = |commands: &str| SetupPlan {
            steps: vec![SetupStep {
                repo: "app".into(),
                command: "echo run >> runs.txt".into(),
            }],
            project_commands: Some(commands.into()),
            repos: vec!["app".into()],
        };
        let runs = || async {
            sandbox::exec_ok(
                &docker,
                &container,
                &["cat", "/workspace/app/runs.txt"],
                &agent,
            )
            .await
            .unwrap()
            .stdout_str()
            .lines()
            .count()
        };
        let setup = |plan| run(&docker, &container, plan, Vec::new(), &|_| {});

        assert_eq!(setup(make_plan("true")).await.outcome, SetupOutcome::Ran);
        assert_eq!(
            setup(make_plan("true")).await.outcome,
            SetupOutcome::UpToDate
        );
        assert_eq!(runs().await, 1);

        sandbox::exec_ok(
            &docker,
            &container,
            &["sh", "-c", "echo v2 > /workspace/app/go.sum"],
            &agent,
        )
        .await
        .unwrap();
        assert_eq!(setup(make_plan("true")).await.outcome, SetupOutcome::Ran);
        assert_eq!(runs().await, 2);

        // The agent can't forge the hash.
        let forged = docker
            .exec(
                &container,
                &["rm", "-f", "/kanbain/state/setup.sha"],
                &agent,
            )
            .await
            .unwrap();
        assert_ne!(forged.exit_code, 0);

        let failed = setup(make_plan("exit 3")).await.outcome;
        assert!(
            matches!(&failed, SetupOutcome::Failed { failed_steps, .. } if failed_steps.len() == 1),
            "{failed:?}"
        );
        // No hash written, so the same inputs try again next run.
        assert!(matches!(
            setup(make_plan("exit 3")).await.outcome,
            SetupOutcome::Failed { .. }
        ));
        assert_eq!(runs().await, 4);

        docker.remove(&container).await.unwrap();
    }

    fn which_bash() -> Result<&'static str, ()> {
        ["/bin/bash", "/usr/bin/bash"]
            .into_iter()
            .find(|p| Path::new(p).exists())
            .ok_or(())
    }
}
