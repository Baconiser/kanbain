//! Docker sandbox for agent runs.
//!
//! Every agent run gets its own container on an internal Docker network with
//! no route out. The only reachable endpoint is the gateway sidecar, which
//! forwards to the backend's model proxy and egress proxy (see [`network`]).

pub mod docker;
pub mod egress;
pub mod forward;
pub mod image;
pub mod network;
pub mod transfer;

use std::collections::HashMap;

/// Keeps a child process from opening a console window. KanbAIn is a GUI
/// app on Windows, where every `git` or `docker` child would otherwise pop
/// up a terminal (the port forward starts one per connection).
pub(crate) fn hide_console(cmd: &mut tokio::process::Command) -> &mut tokio::process::Command {
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

use async_trait::async_trait;
pub use docker::DockerSandbox;
use futures::stream::BoxStream;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Label set on every container, network and volume KanbAIn creates.
pub const LABEL_MANAGED: &str = "dev.kanbain.managed";
/// Label scoping resources to one KanbAIn data folder, so a dev instance and
/// the desktop app sharing a Docker daemon never clean up each other's runs.
pub const LABEL_INSTANCE: &str = "dev.kanbain.instance";
/// Label naming what a container is for (`agent` or `gateway`).
pub const LABEL_KIND: &str = "dev.kanbain.kind";
pub const LABEL_WORKSPACE: &str = "dev.kanbain.workspace";
pub const LABEL_EXECUTION: &str = "dev.kanbain.execution";

/// Mount point of the per-run context volume. `context/` is read-only for the
/// agent, `out/` is writable and harvested when the run ends.
pub const KANBAIN_DIR: &str = "/kanbain";
pub const CONTEXT_DIR: &str = "/kanbain/context";
pub const OUT_DIR: &str = "/kanbain/out";
/// Where repositories are cloned inside the container.
pub const WORKSPACE_DIR: &str = "/workspace";
/// Unprivileged user agents run as (the `node` user of the base image).
pub const AGENT_USER: &str = "node";
pub const AGENT_UID: u64 = 1000;
/// Git author name of agent commits (set in the base image). The history
/// check tells them apart from people's commits.
pub const AGENT_GIT_NAME: &str = "KanbAIn agent";

#[derive(Debug, Error)]
pub enum SandboxError {
    #[error("Docker is not available: {0}")]
    Unavailable(String),
    #[error("Docker {found} is too old; KanbAIn needs API version {required} or newer")]
    UnsupportedVersion { found: String, required: String },
    #[error(transparent)]
    Docker(#[from] bollard::errors::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("command `{command}` failed with exit code {code}: {stderr}")]
    ExecFailed {
        command: String,
        code: i64,
        stderr: String,
    },
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, SandboxError>;

/// Resource limits applied to a container.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Limits {
    pub cpus: f64,
    pub memory_mb: u64,
    pub pids: i64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            cpus: 2.0,
            memory_mb: 4096,
            pids: 1024,
        }
    }
}

/// Everything needed to create a sandbox container.
#[derive(Debug, Clone, Default)]
pub struct ContainerSpec {
    pub name: String,
    pub image: String,
    pub labels: HashMap<String, String>,
    pub env: Vec<(String, String)>,
    pub limits: Limits,
    /// Networks to attach, with optional DNS aliases. The first one is used
    /// at creation, the rest are connected before start.
    pub networks: Vec<(String, Vec<String>)>,
    /// Anonymous volumes (container paths). Never bind mounts.
    pub volumes: Vec<String>,
    pub working_dir: Option<String>,
    pub user: Option<String>,
    /// Defaults to a process that sleeps forever so the container stays up
    /// for `exec`.
    pub cmd: Option<Vec<String>>,
    /// `host:ip` entries for `/etc/hosts` (the gateway uses
    /// `host.docker.internal:host-gateway`).
    pub extra_hosts: Vec<String>,
}

/// A file to place into a container.
#[derive(Debug, Clone)]
pub struct FileEntry {
    /// Path relative to the destination directory.
    pub path: String,
    pub contents: Vec<u8>,
    pub mode: u32,
    pub uid: u64,
}

impl FileEntry {
    pub fn new(path: impl Into<String>, contents: impl Into<Vec<u8>>) -> Self {
        Self {
            path: path.into(),
            contents: contents.into(),
            mode: 0o644,
            uid: AGENT_UID,
        }
    }

    pub fn owned_by_root(mut self) -> Self {
        self.uid = 0;
        self
    }
}

#[derive(Debug, Clone, Default)]
pub struct ExecOptions {
    pub user: Option<String>,
    pub working_dir: Option<String>,
    pub env: Vec<(String, String)>,
}

#[derive(Debug, Clone)]
pub struct ExecOutput {
    pub exit_code: i64,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl ExecOutput {
    pub fn stdout_str(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }
}

/// A container runtime for agent runs.
#[async_trait]
pub trait Sandbox: Send + Sync {
    /// Create and start a container; returns its id.
    async fn create(&self, spec: &ContainerSpec) -> Result<String>;

    /// Run a command to completion and collect its output.
    async fn exec(&self, id: &str, cmd: &[&str], opts: &ExecOptions) -> Result<ExecOutput>;

    /// Follow the container's main process output.
    fn stream_logs(&self, id: &str) -> BoxStream<'static, Result<Vec<u8>>>;

    /// Write files into `dest_dir` (created if missing).
    async fn copy_in(&self, id: &str, dest_dir: &str, files: Vec<FileEntry>) -> Result<()>;

    /// Read the regular files under `path` (a file or a directory), keyed by
    /// path relative to `path`'s parent.
    async fn copy_out(&self, id: &str, path: &str) -> Result<Vec<(String, Vec<u8>)>>;

    async fn stop(&self, id: &str) -> Result<()>;

    /// Force-remove the container and its anonymous volumes.
    async fn remove(&self, id: &str) -> Result<()>;

    /// Whether the container exists and is running.
    async fn is_running(&self, id: &str) -> Result<bool>;
}

/// Run `cmd` and fail unless it exits with 0.
pub async fn exec_ok(
    sandbox: &dyn Sandbox,
    id: &str,
    cmd: &[&str],
    opts: &ExecOptions,
) -> Result<ExecOutput> {
    let out = sandbox.exec(id, cmd, opts).await?;
    if out.exit_code != 0 {
        return Err(SandboxError::ExecFailed {
            command: cmd.join(" "),
            code: out.exit_code,
            stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
        });
    }
    Ok(out)
}

/// Stop the process group whose leader's pid is in `pid_file`: TERM, then
/// KILL after about three seconds. Runs as the agent user, because
/// containers drop all capabilities and root may not signal its processes.
pub async fn kill_process_group(sandbox: &dyn Sandbox, id: &str, pid_file: &str) {
    let script = format!(
        "pid=$(cat {pid_file} 2>/dev/null) || exit 0; \
         kill -TERM -$pid 2>/dev/null; \
         for i in 1 2 3 4 5 6 7 8 9 10; do kill -0 -$pid 2>/dev/null || exit 0; sleep 0.3; done; \
         kill -KILL -$pid 2>/dev/null; exit 0"
    );
    let _ = sandbox
        .exec(
            id,
            &["sh", "-c", &script],
            &ExecOptions {
                user: Some(AGENT_USER.into()),
                ..Default::default()
            },
        )
        .await;
}
