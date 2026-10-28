use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use git::GitService;
use tokio::process::Command;

use crate::command::CmdOverrides;

/// Repository context for executor operations
#[derive(Debug, Clone, Default)]
pub struct RepoContext {
    pub workspace_root: PathBuf,
    /// Names of repositories in the workspace (subdirectory names)
    pub repo_names: Vec<String>,
}

impl RepoContext {
    pub fn new(workspace_root: PathBuf, repo_names: Vec<String>) -> Self {
        Self {
            workspace_root,
            repo_names,
        }
    }

    pub fn repo_paths(&self) -> Vec<PathBuf> {
        self.repo_names
            .iter()
            .map(|name| self.workspace_root.join(name))
            .collect()
    }

    /// Check all repos for uncommitted changes.
    /// Returns a formatted string describing any uncommitted changes found,
    /// or an empty string if all repos are clean.
    pub async fn check_uncommitted_changes(&self) -> String {
        let repo_paths = self.repo_paths();
        if repo_paths.is_empty() {
            return String::new();
        }

        tokio::task::spawn_blocking(move || {
            let git = GitService::new();
            let mut all_status = String::new();

            for repo_path in &repo_paths {
                // Skip if not a git repository
                if !repo_path.join(".git").exists() {
                    continue;
                }

                match git.get_worktree_status(repo_path) {
                    Ok(status) if !status.entries.is_empty() => {
                        let mut status_output = String::new();
                        for entry in &status.entries {
                            status_output.push(entry.staged);
                            status_output.push(entry.unstaged);
                            status_output.push(' ');
                            status_output.push_str(&String::from_utf8_lossy(&entry.path));
                            status_output.push('\n');
                        }
                        all_status.push_str(&format!(
                            "\n{}:\n{}",
                            repo_path.display(),
                            status_output
                        ));
                    }
                    _ => {}
                }
            }

            all_status
        })
        .await
        .unwrap_or_default()
    }
}

/// Directory inside a sandbox container where each process records its pid,
/// so the backend can kill the whole process group on stop.
pub const SANDBOX_PID_DIR: &str = "/tmp/kanbain-exec";

/// Runs executor processes inside a sandbox container instead of on the
/// host. Commands become `docker exec` invocations; nothing from the host
/// environment is passed in except the variables named here and in
/// [`ExecutionEnv::vars`].
#[derive(Debug, Clone)]
pub struct SandboxTarget {
    pub container: String,
    /// Host workspace root, mapped to `container_root` inside the container.
    pub host_root: PathBuf,
    pub container_root: String,
    pub user: String,
    /// Container-only variables (proxy URLs, run token). They override all
    /// other variables.
    pub vars: HashMap<String, String>,
    /// Names the pid file of this process (the execution process id).
    pub process_key: String,
}

impl SandboxTarget {
    /// Map a host path under the workspace root to the container path.
    pub fn container_path(&self, host: &Path) -> String {
        let rel = host.strip_prefix(&self.host_root).unwrap_or(Path::new(""));
        let mut path = self.container_root.trim_end_matches('/').to_string();
        for part in rel.components() {
            path.push('/');
            path.push_str(&part.as_os_str().to_string_lossy());
        }
        path
    }

    pub fn pid_file(&self) -> String {
        format!("{SANDBOX_PID_DIR}/{}.pid", self.process_key)
    }

    /// `docker exec -i` running `program args` in the container directory
    /// matching `host_dir`. A small `sh` wrapper records the pid first; each
    /// `docker exec` starts its own session, so that pid is also the process
    /// group to kill.
    pub fn command(
        &self,
        program: &str,
        args: &[String],
        host_dir: &Path,
        vars: &HashMap<String, String>,
    ) -> Command {
        let mut cmd = Command::new("docker");
        cmd.args(["exec", "-i", "-u", &self.user, "-w"])
            .arg(self.container_path(host_dir));
        let mut merged: Vec<(&String, &String)> = vars
            .iter()
            .filter(|(key, _)| !self.vars.contains_key(*key))
            .chain(self.vars.iter())
            .collect();
        merged.sort();
        for (key, value) in merged {
            cmd.arg("-e").arg(format!("{key}={value}"));
        }
        let wrapper = format!(
            "mkdir -p {SANDBOX_PID_DIR} && echo $$ > {} && exec \"$0\" \"$@\"",
            self.pid_file()
        );
        cmd.arg(&self.container)
            .args(["sh", "-c", &wrapper, program])
            .args(args);
        cmd
    }
}

/// Environment variables to inject into executor processes
#[derive(Debug, Clone)]
pub struct ExecutionEnv {
    pub vars: HashMap<String, String>,
    pub repo_context: RepoContext,
    pub commit_reminder: bool,
    pub commit_reminder_prompt: String,
    /// Run inside this container instead of on the host.
    pub sandbox: Option<SandboxTarget>,
    /// Extra CLI arguments from a KanbAIn agent definition's adapter.
    pub agent_args: Vec<String>,
}

impl ExecutionEnv {
    pub fn new(
        repo_context: RepoContext,
        commit_reminder: bool,
        commit_reminder_prompt: String,
    ) -> Self {
        Self {
            vars: HashMap::new(),
            repo_context,
            commit_reminder,
            commit_reminder_prompt,
            sandbox: None,
            agent_args: Vec::new(),
        }
    }

    /// Insert an environment variable
    pub fn insert(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.vars.insert(key.into(), value.into());
    }

    /// Merge additional vars into this env. Incoming keys overwrite existing ones.
    pub fn merge(&mut self, other: &HashMap<String, String>) {
        self.vars
            .extend(other.iter().map(|(k, v)| (k.clone(), v.clone())));
    }

    /// Return a new env with overrides applied. Overrides take precedence.
    pub fn with_overrides(mut self, overrides: &HashMap<String, String>) -> Self {
        self.merge(overrides);
        self
    }

    /// Return a new env with profile env from CmdOverrides merged in.
    pub fn with_profile(self, cmd: &CmdOverrides) -> Self {
        if let Some(ref profile_env) = cmd.env {
            self.with_overrides(profile_env)
        } else {
            self
        }
    }

    /// Apply all environment variables to a Command
    pub fn apply_to_command(&self, command: &mut Command) {
        for (key, value) in &self.vars {
            command.env(key, value);
        }
    }

    pub fn contains_key(&self, key: &str) -> bool {
        self.vars.contains_key(key)
    }

    pub fn get(&self, key: &str) -> Option<&String> {
        self.vars.get(key)
    }

    /// Build the command for `program args` in `dir`, on the host or in the
    /// sandbox. Host programs are resolved on `PATH`; sandbox programs are
    /// resolved inside the container.
    pub async fn command(
        &self,
        program: &str,
        args: &[String],
        dir: &Path,
    ) -> Result<Command, crate::executors::ExecutorError> {
        match &self.sandbox {
            Some(target) => Ok(target.command(program, args, dir, &self.vars)),
            None => {
                let (executable, args) =
                    crate::command::CommandParts::new(program.to_string(), args.to_vec())
                        .into_resolved()
                        .await?;
                let mut command = Command::new(executable);
                command.args(args).current_dir(dir);
                self.apply_to_command(&mut command);
                Ok(command)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sandbox_command_maps_paths_and_env() {
        let target = SandboxTarget {
            container: "c1".into(),
            host_root: PathBuf::from("/home/u/worktrees/ws"),
            container_root: "/workspace".into(),
            user: "node".into(),
            vars: HashMap::from([("TOKEN".to_string(), "sandbox".to_string())]),
            process_key: "p1".into(),
        };
        assert_eq!(
            target.container_path(Path::new("/home/u/worktrees/ws/app/src")),
            "/workspace/app/src"
        );
        assert_eq!(
            target.container_path(Path::new("/home/u/worktrees/ws")),
            "/workspace"
        );
        let vars = HashMap::from([
            ("TOKEN".to_string(), "host".to_string()),
            ("A".to_string(), "1".to_string()),
        ]);
        let cmd = target.command(
            "npx",
            &["-y".into(), "pkg".into()],
            Path::new("/home/u/worktrees/ws/app"),
            &vars,
        );
        let args: Vec<String> = cmd
            .as_std()
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(cmd.as_std().get_program(), "docker");
        assert_eq!(
            args,
            vec![
                "exec",
                "-i",
                "-u",
                "node",
                "-w",
                "/workspace/app",
                "-e",
                "A=1",
                "-e",
                "TOKEN=sandbox",
                "c1",
                "sh",
                "-c",
                "mkdir -p /tmp/kanbain-exec && echo $$ > /tmp/kanbain-exec/p1.pid && exec \"$0\" \"$@\"",
                "npx",
                "-y",
                "pkg",
            ]
        );
    }

    #[test]
    fn profile_overrides_runtime_env() {
        let mut base = ExecutionEnv::new(RepoContext::default(), false, String::new());
        base.insert("KANBAIN_PROJECT_NAME", "runtime");
        base.insert("FOO", "runtime");

        let mut profile = HashMap::new();
        profile.insert("FOO".to_string(), "profile".to_string());
        profile.insert("BAR".to_string(), "profile".to_string());

        let merged = base.with_overrides(&profile);

        assert_eq!(merged.vars.get("KANBAIN_PROJECT_NAME").unwrap(), "runtime");
        assert_eq!(merged.vars.get("FOO").unwrap(), "profile"); // overrides
        assert_eq!(merged.vars.get("BAR").unwrap(), "profile");
    }
}
