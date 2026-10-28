//! The agent base image (`docker/agent/Dockerfile`), built locally on first
//! use, and optional per-project images built `FROM` it.

use std::{collections::HashMap, path::Path};

use bollard::{body_full, query_parameters::BuildImageOptionsBuilder};
use futures::StreamExt;

use crate::{DockerSandbox, Result, SandboxError, docker::is_not_found};

const BASE_DOCKERFILE: &str = include_str!("../../../docker/agent/Dockerfile");
pub const BASE_IMAGE_REPO: &str = "kanbain-agent";

/// Versions baked into the base image.
#[derive(Debug, Clone)]
pub struct BaseImageArgs {
    pub claude_code_package: String,
    pub claude_code_version: String,
    pub opencode_package: String,
    pub opencode_version: String,
}

impl BaseImageArgs {
    fn build_args(&self) -> HashMap<String, String> {
        HashMap::from([
            (
                "CLAUDE_CODE_PACKAGE".into(),
                self.claude_code_package.clone(),
            ),
            (
                "CLAUDE_CODE_VERSION".into(),
                self.claude_code_version.clone(),
            ),
            ("OPENCODE_PACKAGE".into(), self.opencode_package.clone()),
            ("OPENCODE_VERSION".into(), self.opencode_version.clone()),
        ])
    }
}

/// `kanbain-agent:<app version>-<hash>`; the hash covers the Dockerfile and
/// the CLI versions, so changing either produces a new tag and a rebuild.
pub fn base_image_tag(app_version: &str, args: &BaseImageArgs) -> String {
    let hash = stable_hash(&[
        BASE_DOCKERFILE,
        &args.claude_code_package,
        &args.claude_code_version,
        &args.opencode_package,
        &args.opencode_version,
    ]);
    format!("{BASE_IMAGE_REPO}:{app_version}-{hash:08x}")
}

/// Tag for a project's own Dockerfile, which may start `FROM kanbain-agent`.
/// The `KANBAIN_BASE_IMAGE` build arg carries the exact base tag.
pub fn project_image_tag(project_id: &str, dockerfile: &str, base_tag: &str) -> String {
    let hash = stable_hash(&[dockerfile, base_tag]);
    format!("kanbain-project-{project_id}:{hash:08x}")
}

/// FNV-1a over the parts, so tags stay the same across Rust versions.
pub fn stable_hash(parts: &[&str]) -> u32 {
    let mut hash: u32 = 0x811c9dc5;
    for part in parts {
        for byte in part.bytes().chain([0]) {
            hash ^= byte as u32;
            hash = hash.wrapping_mul(0x01000193);
        }
    }
    hash
}

impl DockerSandbox {
    pub async fn image_exists(&self, tag: &str) -> Result<bool> {
        match self.client().inspect_image(tag).await {
            Ok(_) => Ok(true),
            Err(e) if is_not_found(&e) => Ok(false),
            Err(e) => Err(e.into()),
        }
    }

    /// Build the base image unless `tag` already exists. `progress` receives
    /// build output lines.
    pub async fn ensure_base_image(
        &self,
        tag: &str,
        args: &BaseImageArgs,
        progress: impl FnMut(&str),
    ) -> Result<()> {
        if self.image_exists(tag).await? {
            return Ok(());
        }
        let context = crate::docker::build_tar(vec![
            crate::FileEntry::new("Dockerfile", BASE_DOCKERFILE).owned_by_root(),
        ])?;
        self.build(tag, context, args.build_args(), progress).await
    }

    /// Build a project image from the directory holding its Dockerfile.
    pub async fn ensure_project_image(
        &self,
        tag: &str,
        context_dir: &Path,
        base_tag: &str,
        progress: impl FnMut(&str),
    ) -> Result<()> {
        if self.image_exists(tag).await? {
            return Ok(());
        }
        let dir = context_dir.to_path_buf();
        let context = tokio::task::spawn_blocking(move || crate::docker::tar_dir(&dir))
            .await
            .map_err(|e| SandboxError::Other(e.to_string()))??;
        let build_args = HashMap::from([("KANBAIN_BASE_IMAGE".into(), base_tag.to_string())]);
        self.build(tag, context, build_args, progress).await
    }

    async fn build(
        &self,
        tag: &str,
        context: Vec<u8>,
        build_args: HashMap<String, String>,
        mut progress: impl FnMut(&str),
    ) -> Result<()> {
        tracing::info!("Building sandbox image {tag}");
        let options = BuildImageOptionsBuilder::default()
            .dockerfile("Dockerfile")
            .t(tag)
            .rm(true)
            .buildargs(&build_args)
            .build();
        let mut stream = self
            .client()
            .build_image(options, None, Some(body_full(context.into())));
        while let Some(item) = stream.next().await {
            let info = item?;
            if let Some(error) = info.error_detail.and_then(|d| d.message) {
                return Err(SandboxError::Other(format!(
                    "building image {tag} failed: {error}"
                )));
            }
            if let Some(line) = info.stream {
                let line = line.trim_end();
                if !line.is_empty() {
                    progress(line);
                }
            }
        }
        if !self.image_exists(tag).await? {
            return Err(SandboxError::Other(format!(
                "building image {tag} produced no image"
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(version: &str) -> BaseImageArgs {
        BaseImageArgs {
            claude_code_package: "@anthropic-ai/claude-code".into(),
            claude_code_version: version.into(),
            opencode_package: "opencode-ai".into(),
            opencode_version: "1.0.0".into(),
        }
    }

    #[test]
    fn tag_changes_with_cli_versions() {
        let a = base_image_tag("0.1.0", &args("2.0.0"));
        let b = base_image_tag("0.1.0", &args("2.0.1"));
        assert!(a.starts_with("kanbain-agent:0.1.0-"));
        assert_ne!(a, b);
        assert_eq!(a, base_image_tag("0.1.0", &args("2.0.0")));
    }
}
