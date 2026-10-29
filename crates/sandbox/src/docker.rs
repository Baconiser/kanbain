use std::{collections::HashMap, io::Read, path::Path};

use async_trait::async_trait;
use bollard::{
    Docker, body_full,
    container::LogOutput,
    exec::{StartExecOptions, StartExecResults},
    models::{
        ContainerCreateBody, EndpointSettings, ExecConfig, HostConfig, NetworkConnectRequest,
        NetworkingConfig,
    },
    query_parameters::{
        CreateContainerOptionsBuilder, DownloadFromContainerOptionsBuilder,
        ListContainersOptionsBuilder, LogsOptionsBuilder, RemoveContainerOptionsBuilder,
        StopContainerOptionsBuilder, UploadToContainerOptionsBuilder,
    },
};
use futures::{StreamExt, TryStreamExt, stream::BoxStream};
use tokio::process::Command;

use crate::{
    ContainerSpec, ExecOptions, ExecOutput, FileEntry, LABEL_INSTANCE, LABEL_MANAGED, Result,
    Sandbox, SandboxError,
};

/// Oldest Docker Engine API KanbAIn supports (Docker 24).
pub const MIN_API_VERSION: &str = "1.43";

/// What the backend learned about the Docker daemon at startup.
#[derive(Debug, Clone, serde::Serialize)]
pub struct DockerInfo {
    pub version: String,
    pub api_version: String,
    pub operating_system: String,
    /// Docker Desktop runs containers in a VM, so the host is only reachable
    /// as `host.docker.internal` and the bridge gateway is not a host address.
    pub is_docker_desktop: bool,
}

#[derive(Clone)]
pub struct DockerSandbox {
    docker: Docker,
}

impl DockerSandbox {
    /// Connect using the platform defaults (`DOCKER_HOST`, the Unix socket or
    /// the Windows named pipe) and check the daemon is usable.
    pub async fn connect() -> Result<(Self, DockerInfo)> {
        let docker = Docker::connect_with_local_defaults()
            .map_err(|e| SandboxError::Unavailable(e.to_string()))?;
        let version = docker
            .version()
            .await
            .map_err(|e| SandboxError::Unavailable(e.to_string()))?;
        let api_version = version.api_version.clone().unwrap_or_default();
        if !api_version_at_least(&api_version, MIN_API_VERSION) {
            return Err(SandboxError::UnsupportedVersion {
                found: format!(
                    "{} (API {api_version})",
                    version.version.unwrap_or_default()
                ),
                required: MIN_API_VERSION.to_string(),
            });
        }
        let info = docker
            .info()
            .await
            .map_err(|e| SandboxError::Unavailable(e.to_string()))?;
        let operating_system = info.operating_system.unwrap_or_default();
        let docker_info = DockerInfo {
            version: version.version.unwrap_or_default(),
            api_version,
            is_docker_desktop: operating_system.contains("Docker Desktop"),
            operating_system,
        };
        Ok((Self { docker }, docker_info))
    }

    pub fn client(&self) -> &Docker {
        &self.docker
    }

    /// A `docker exec -i` command for long-running processes whose stdio the
    /// caller streams (agents, scripts, dev servers). Going through the CLI
    /// keeps them ordinary child processes, so the existing process-group,
    /// log and stdin plumbing works unchanged.
    pub fn exec_command(
        container: &str,
        program: &str,
        args: &[String],
        opts: &ExecOptions,
        tty: bool,
    ) -> Command {
        let mut cmd = Command::new("docker");
        crate::hide_console(&mut cmd).arg("exec").arg("-i");
        if tty {
            cmd.arg("-t");
        }
        if let Some(user) = &opts.user {
            cmd.args(["-u", user]);
        }
        if let Some(dir) = &opts.working_dir {
            cmd.args(["-w", dir]);
        }
        for (key, value) in &opts.env {
            cmd.arg("-e").arg(format!("{key}={value}"));
        }
        cmd.arg(container).arg(program).args(args);
        cmd
    }

    /// Ids of containers carrying the given labels (running or not).
    pub async fn list_by_labels(&self, labels: &[(&str, &str)]) -> Result<Vec<String>> {
        let filters: HashMap<String, Vec<String>> = HashMap::from([(
            "label".to_string(),
            labels.iter().map(|(k, v)| format!("{k}={v}")).collect(),
        )]);
        let options = ListContainersOptionsBuilder::default()
            .all(true)
            .filters(&filters)
            .build();
        Ok(self
            .docker
            .list_containers(Some(options))
            .await?
            .into_iter()
            .filter_map(|c| c.id)
            .collect())
    }

    /// Remove every managed container of this instance except `keep`.
    /// Called at startup to clean up after a crash.
    pub async fn remove_orphans(&self, instance: &str, keep: &[String]) -> Result<usize> {
        let ids = self
            .list_by_labels(&[(LABEL_MANAGED, "true"), (LABEL_INSTANCE, instance)])
            .await?;
        let mut removed = 0;
        for id in ids {
            if keep.iter().any(|k| id.starts_with(k.as_str())) {
                continue;
            }
            if let Err(e) = self.remove(&id).await {
                tracing::warn!("Failed to remove orphaned sandbox container {id}: {e}");
            } else {
                removed += 1;
            }
        }
        Ok(removed)
    }
}

#[async_trait]
impl Sandbox for DockerSandbox {
    async fn create(&self, spec: &ContainerSpec) -> Result<String> {
        let mut networks = spec.networks.iter();
        let first_network = networks.next();

        let host_config = HostConfig {
            nano_cpus: Some((spec.limits.cpus * 1e9) as i64),
            memory: Some((spec.limits.memory_mb * 1024 * 1024) as i64),
            pids_limit: Some(spec.limits.pids),
            cap_drop: Some(vec!["ALL".to_string()]),
            security_opt: Some(vec!["no-new-privileges".to_string()]),
            network_mode: first_network.map(|(name, _)| name.clone()),
            extra_hosts: (!spec.extra_hosts.is_empty()).then(|| spec.extra_hosts.clone()),
            init: Some(true),
            ..Default::default()
        };
        let networking_config = first_network.map(|(name, aliases)| NetworkingConfig {
            endpoints_config: Some(HashMap::from([(
                name.clone(),
                EndpointSettings {
                    aliases: (!aliases.is_empty()).then(|| aliases.clone()),
                    ..Default::default()
                },
            )])),
        });

        let body = ContainerCreateBody {
            image: Some(spec.image.clone()),
            labels: Some(spec.labels.clone()),
            env: Some(spec.env.iter().map(|(k, v)| format!("{k}={v}")).collect()),
            cmd: Some(
                spec.cmd
                    .clone()
                    .unwrap_or_else(|| vec!["sleep".into(), "infinity".into()]),
            ),
            working_dir: spec.working_dir.clone(),
            user: spec.user.clone(),
            volumes: (!spec.volumes.is_empty()).then(|| spec.volumes.clone()),
            host_config: Some(host_config),
            networking_config,
            ..Default::default()
        };
        let options = CreateContainerOptionsBuilder::default()
            .name(&spec.name)
            .build();
        let id = self.docker.create_container(Some(options), body).await?.id;

        let result: Result<()> = async {
            for (name, aliases) in networks {
                self.docker
                    .connect_network(
                        name,
                        NetworkConnectRequest {
                            container: id.clone(),
                            endpoint_config: Some(EndpointSettings {
                                aliases: (!aliases.is_empty()).then(|| aliases.clone()),
                                ..Default::default()
                            }),
                        },
                    )
                    .await?;
            }
            self.docker.start_container(&id, None).await?;
            Ok(())
        }
        .await;
        if let Err(e) = result {
            let _ = self.remove(&id).await;
            return Err(e);
        }
        Ok(id)
    }

    async fn exec(&self, id: &str, cmd: &[&str], opts: &ExecOptions) -> Result<ExecOutput> {
        let exec = self
            .docker
            .create_exec(
                id,
                ExecConfig {
                    attach_stdout: Some(true),
                    attach_stderr: Some(true),
                    cmd: Some(cmd.iter().map(|s| s.to_string()).collect()),
                    user: opts.user.clone(),
                    working_dir: opts.working_dir.clone(),
                    env: Some(opts.env.iter().map(|(k, v)| format!("{k}={v}")).collect()),
                    ..Default::default()
                },
            )
            .await?;
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        if let StartExecResults::Attached { mut output, .. } = self
            .docker
            .start_exec(
                &exec.id,
                Some(StartExecOptions {
                    output_capacity: Some(1024 * 1024),
                    ..Default::default()
                }),
            )
            .await?
        {
            while let Some(chunk) = output.next().await {
                match chunk? {
                    LogOutput::StdOut { message } | LogOutput::Console { message } => {
                        stdout.extend_from_slice(&message)
                    }
                    LogOutput::StdErr { message } => stderr.extend_from_slice(&message),
                    LogOutput::StdIn { .. } => {}
                }
            }
        }
        let exit_code = self
            .docker
            .inspect_exec(&exec.id)
            .await?
            .exit_code
            .unwrap_or(-1);
        Ok(ExecOutput {
            exit_code,
            stdout,
            stderr,
        })
    }

    fn stream_logs(&self, id: &str) -> BoxStream<'static, Result<Vec<u8>>> {
        let options = LogsOptionsBuilder::default()
            .follow(true)
            .stdout(true)
            .stderr(true)
            .build();
        let docker = self.docker.clone();
        let id = id.to_string();
        futures::stream::once(async move { docker.logs(&id, Some(options)) })
            .flatten()
            .map_ok(|chunk| chunk.into_bytes().to_vec())
            .map_err(SandboxError::from)
            .boxed()
    }

    async fn copy_in(&self, id: &str, dest_dir: &str, files: Vec<FileEntry>) -> Result<()> {
        // Make sure the destination exists; extraction does not create it.
        crate::exec_ok(
            self,
            id,
            &["mkdir", "-p", dest_dir],
            &ExecOptions {
                user: Some("root".into()),
                ..Default::default()
            },
        )
        .await?;
        let tar = build_tar(files)?;
        let options = UploadToContainerOptionsBuilder::default()
            .path(dest_dir)
            .build();
        self.docker
            .upload_to_container(id, Some(options), body_full(tar.into()))
            .await?;
        Ok(())
    }

    async fn copy_out(&self, id: &str, path: &str) -> Result<Vec<(String, Vec<u8>)>> {
        let options = DownloadFromContainerOptionsBuilder::default()
            .path(path)
            .build();
        let bytes: Vec<u8> = self
            .docker
            .download_from_container(id, Some(options))
            .try_fold(Vec::new(), |mut acc, chunk| async move {
                acc.extend_from_slice(&chunk);
                Ok(acc)
            })
            .await?;
        read_tar(&bytes)
    }

    async fn stop(&self, id: &str) -> Result<()> {
        let options = StopContainerOptionsBuilder::default().t(5).build();
        match self.docker.stop_container(id, Some(options)).await {
            Ok(()) => Ok(()),
            Err(e) if is_not_found(&e) => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    async fn remove(&self, id: &str) -> Result<()> {
        let options = RemoveContainerOptionsBuilder::default()
            .force(true)
            .v(true)
            .build();
        match self.docker.remove_container(id, Some(options)).await {
            Ok(()) => Ok(()),
            Err(e) if is_not_found(&e) => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    async fn is_running(&self, id: &str) -> Result<bool> {
        match self.docker.inspect_container(id, None).await {
            Ok(info) => Ok(info.state.and_then(|s| s.running).unwrap_or(false)),
            Err(e) if is_not_found(&e) => Ok(false),
            Err(e) => Err(e.into()),
        }
    }
}

pub(crate) fn is_not_found(e: &bollard::errors::Error) -> bool {
    matches!(
        e,
        bollard::errors::Error::DockerResponseServerError {
            status_code: 404,
            ..
        }
    )
}

fn api_version_at_least(found: &str, required: &str) -> bool {
    let parse = |v: &str| -> (u32, u32) {
        let mut parts = v.split('.').map(|p| p.parse().unwrap_or(0));
        (parts.next().unwrap_or(0), parts.next().unwrap_or(0))
    };
    parse(found) >= parse(required)
}

pub(crate) fn build_tar(files: Vec<FileEntry>) -> Result<Vec<u8>> {
    let mut builder = tar::Builder::new(Vec::new());
    for file in files {
        let mut header = tar::Header::new_gnu();
        header.set_size(file.contents.len() as u64);
        header.set_mode(file.mode);
        header.set_uid(file.uid);
        header.set_gid(file.uid);
        header.set_mtime(0);
        header.set_entry_type(tar::EntryType::Regular);
        builder.append_data(&mut header, &file.path, file.contents.as_slice())?;
    }
    Ok(builder.into_inner()?)
}

fn read_tar(bytes: &[u8]) -> Result<Vec<(String, Vec<u8>)>> {
    let mut archive = tar::Archive::new(bytes);
    let mut files = Vec::new();
    for entry in archive.entries()? {
        let mut entry = entry?;
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let path = entry.path()?.to_string_lossy().into_owned();
        let mut contents = Vec::new();
        entry.read_to_end(&mut contents)?;
        files.push((path, contents));
    }
    Ok(files)
}

/// Pack a directory on the host into a tar (used as a Docker build context).
pub(crate) fn tar_dir(dir: &Path) -> Result<Vec<u8>> {
    let mut builder = tar::Builder::new(Vec::new());
    builder.append_dir_all(".", dir)?;
    Ok(builder.into_inner()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_api_versions() {
        assert!(api_version_at_least("1.56", "1.43"));
        assert!(api_version_at_least("1.43", "1.43"));
        assert!(!api_version_at_least("1.41", "1.43"));
        assert!(!api_version_at_least("", "1.43"));
    }

    #[test]
    fn tar_round_trip() {
        let tar = build_tar(vec![
            FileEntry::new("a.txt", "hello"),
            FileEntry::new("dir/b.json", "{}"),
        ])
        .unwrap();
        let files = read_tar(&tar).unwrap();
        assert_eq!(
            files,
            vec![
                ("a.txt".to_string(), b"hello".to_vec()),
                ("dir/b.json".to_string(), b"{}".to_vec())
            ]
        );
    }
}
