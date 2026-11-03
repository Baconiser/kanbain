//! `/kanbain/context/environment.md`: what the sandbox has and how this
//! project builds and tests, so agents stop guessing. Without it a tester
//! burns half its turns on `ls ~/.rustup/toolchains` archaeology and asking
//! for `rustup` it was never going to get.
//!
//! Detection is plain file checks on the host worktree; no agent, no network.

use std::path::{Path, PathBuf};

use crate::project_setup::{SetupOutcome, SetupReport};

/// What the base image (`docker/agent/Dockerfile`) ships.
const BASE_IMAGE_TOOLS: &str = "\
- Node.js 22 with npm, pnpm and yarn (through `corepack`)\n\
- Python 3 with pip and venv\n\
- Rust stable (minimal profile) with cargo, clippy and rustfmt\n\
- gcc, make, pkg-config, OpenSSL headers, git, ripgrep, jq\n";

/// One repo's detected stack.
#[derive(Debug, Default, PartialEq, Eq)]
struct RepoStack {
    stacks: Vec<String>,
    test_commands: Vec<String>,
    notes: Vec<String>,
}

fn detect(root: &Path) -> RepoStack {
    let has = |name: &str| root.join(name).exists();
    let mut out = RepoStack::default();

    if has("Cargo.toml") {
        out.stacks.push("Rust (cargo)".into());
        out.test_commands.push("cargo test".into());
        for file in ["rust-toolchain.toml", "rust-toolchain"] {
            if has(file) {
                out.notes.push(format!(
                    "`{file}` pins a toolchain; it is only available if project setup \
                     installed it (see below)."
                ));
            }
        }
    }

    if has("package.json") {
        let manager = if has("pnpm-lock.yaml") {
            "pnpm"
        } else if has("yarn.lock") {
            "yarn"
        } else if has("bun.lockb") || has("bun.lock") {
            "bun"
        } else {
            "npm"
        };
        out.stacks
            .push(format!("JavaScript/TypeScript ({manager})"));
        let package = std::fs::read_to_string(root.join("package.json")).unwrap_or_default();
        let scripts = serde_json::from_str::<serde_json::Value>(&package)
            .ok()
            .and_then(|v| v.get("scripts").cloned())
            .and_then(|v| v.as_object().cloned())
            .unwrap_or_default();
        if scripts.contains_key("test") {
            out.test_commands.push(format!("{manager} test"));
        }
        for script in ["test:unit", "test:e2e", "check", "lint"] {
            if scripts.contains_key(script) {
                out.test_commands.push(format!("{manager} run {script}"));
            }
        }
        if !has("node_modules") {
            out.notes.push(format!(
                "Dependencies are not installed (`node_modules` is missing). You cannot \
                 run `{manager} install` yourself."
            ));
        }
    }

    if has("pyproject.toml") || has("requirements.txt") || has("setup.py") {
        out.stacks.push("Python".into());
        out.test_commands.push("pytest".into());
    }

    if has("go.mod") {
        out.stacks.push("Go".into());
        out.test_commands.push("go test ./...".into());
    }

    out
}

/// Markdown for `/kanbain/context/environment.md`. `repos` are
/// `(name, host worktree)` pairs; the agent sees each at `/workspace/<name>`.
/// `setup` is what project setup did before this run.
pub fn environment_md(
    repos: &[(String, PathBuf)],
    custom_image: bool,
    setup: Option<&SetupReport>,
) -> String {
    let mut md = String::from("# Sandbox environment\n\n## Installed tools\n\n");
    md.push_str(BASE_IMAGE_TOOLS);
    if custom_image {
        md.push_str("- Plus whatever this project's own Dockerfile adds on top.\n");
    }

    md.push_str("\n## Repositories\n");
    for (name, root) in repos {
        let mut stack = detect(root);
        // The host checkout has no node_modules, but the container does now.
        if setup.is_some_and(|s| s.installed_js(name)) {
            stack.notes.retain(|n| !n.contains("node_modules"));
        }
        md.push_str(&format!("\n### /workspace/{name}\n\n"));
        if stack.stacks.is_empty() {
            md.push_str("No known build system detected.\n");
            continue;
        }
        md.push_str(&format!("- Stack: {}\n", stack.stacks.join(", ")));
        if !stack.test_commands.is_empty() {
            let commands: Vec<String> = stack
                .test_commands
                .iter()
                .map(|c| format!("`{c}`"))
                .collect();
            md.push_str(&format!("- Test commands: {}\n", commands.join(", ")));
        }
        for note in &stack.notes {
            md.push_str(&format!("- {note}\n"));
        }
    }

    if let Some(setup) = setup {
        md.push_str(&setup_md(setup));
    }

    md.push_str(
        "\n## Rules\n\n\
         - You cannot install toolchains, cargo subcommands or system packages, and \
         network access is limited to package registries.\n\
         - If a tool or dependency you need is missing, do not hunt for workarounds. \
         Say exactly what is missing (e.g. \"`cargo component` is not installed\") in \
         your result and stop that line of work.\n",
    );
    md
}

/// The "Project setup" section: what was installed and whether it worked.
fn setup_md(setup: &SetupReport) -> String {
    if setup.outcome == SetupOutcome::NothingToDo {
        return String::new();
    }
    let mut md = String::from(
        "\n## Project setup\n\n\
         KanbAIn ran these before you started (again whenever a lockfile changes):\n\n",
    );
    for step in &setup.plan.steps {
        md.push_str(&format!(
            "- `/workspace/{}`: `{}`\n",
            step.repo, step.command
        ));
    }
    if let Some(commands) = &setup.plan.project_commands {
        md.push_str(&format!(
            "- Project commands, from `/workspace`:\n\n```sh\n{commands}\n```\n"
        ));
    }
    md.push_str(match &setup.outcome {
        SetupOutcome::Ran | SetupOutcome::UpToDate => {
            "\nStatus: succeeded. Dependencies are installed; don't reinstall them. \
             If you add a dependency, update the lockfile and commit it: the next run \
             installs it.\n"
        }
        _ => "",
    });
    if let SetupOutcome::Failed { failed_steps, tail } = &setup.outcome {
        md.push_str("\nStatus: **failed**");
        if !failed_steps.is_empty() {
            md.push_str(&format!(" ({})", failed_steps.join(", ")));
        }
        md.push_str(&format!(
            ". Whatever those steps install is missing; report that instead of \
             working around it.\n\n```\n{tail}\n```\n"
        ));
    }
    md
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_rust_and_pnpm_with_scripts() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("Cargo.toml"), "[package]").unwrap();
        std::fs::write(root.join("rust-toolchain.toml"), "").unwrap();
        std::fs::write(
            root.join("package.json"),
            r#"{"scripts": {"test": "vitest", "lint": "eslint ."}}"#,
        )
        .unwrap();
        std::fs::write(root.join("pnpm-lock.yaml"), "").unwrap();

        let stack = detect(root);
        assert_eq!(
            stack.stacks,
            vec!["Rust (cargo)", "JavaScript/TypeScript (pnpm)"]
        );
        assert_eq!(
            stack.test_commands,
            vec!["cargo test", "pnpm test", "pnpm run lint"]
        );
        assert!(
            stack
                .notes
                .iter()
                .any(|n| n.contains("rust-toolchain.toml"))
        );
        assert!(stack.notes.iter().any(|n| n.contains("node_modules")));
    }

    #[test]
    fn markdown_lists_each_repo_and_the_rules() {
        let empty = tempfile::tempdir().unwrap();
        let go = tempfile::tempdir().unwrap();
        std::fs::write(go.path().join("go.mod"), "module x").unwrap();

        let md = environment_md(
            &[
                ("docs".to_string(), empty.path().to_path_buf()),
                ("api".to_string(), go.path().to_path_buf()),
            ],
            true,
            None,
        );
        assert!(md.contains("### /workspace/docs\n\nNo known build system detected."));
        assert!(md.contains("### /workspace/api\n\n- Stack: Go\n- Test commands: `go test ./...`"));
        assert!(md.contains("project's own Dockerfile"));
        assert!(md.contains("do not hunt for workarounds"));
        assert!(!md.contains("## Project setup"));
    }

    #[test]
    fn setup_section_reflects_the_outcome() {
        use crate::project_setup::{SetupPlan, SetupStep};

        let web = tempfile::tempdir().unwrap();
        std::fs::write(web.path().join("package.json"), "{}").unwrap();
        let repos = [("web".to_string(), web.path().to_path_buf())];
        let plan = SetupPlan {
            steps: vec![SetupStep {
                repo: "web".into(),
                command: "npm ci".into(),
            }],
            project_commands: Some("cargo install cargo-component".into()),
            ..Default::default()
        };

        let ok = SetupReport {
            plan: plan.clone(),
            outcome: SetupOutcome::Ran,
        };
        let md = environment_md(&repos, false, Some(&ok));
        assert!(md.contains("- `/workspace/web`: `npm ci`"));
        assert!(md.contains("cargo install cargo-component"));
        assert!(md.contains("Status: succeeded"));
        assert!(!md.contains("node_modules"));

        let failed = SetupReport {
            plan,
            outcome: SetupOutcome::Failed {
                failed_steps: vec!["[/workspace/web] npm ci".into()],
                tail: "npm ERR! boom".into(),
            },
        };
        let md = environment_md(&repos, false, Some(&failed));
        assert!(md.contains("Status: **failed** ([/workspace/web] npm ci)"));
        assert!(md.contains("npm ERR! boom"));
        assert!(md.contains("node_modules"));
    }
}
