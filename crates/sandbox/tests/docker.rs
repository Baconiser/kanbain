//! End-to-end sandbox test against a real Docker daemon.
//!
//! Skipped unless `KANBAIN_DOCKER_TESTS=1`. Set `KANBAIN_TEST_AGENT_IMAGE` to
//! reuse an already built agent image; otherwise the base image is built.

use std::{
    collections::HashMap,
    path::Path,
    process::Command,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use sandbox::{
    ContainerSpec, DockerSandbox, ExecOptions, LABEL_INSTANCE, LABEL_KIND, LABEL_MANAGED, Sandbox,
    egress::{self, Allowlist, EgressPolicy, EgressVerdict},
    image::{BaseImageArgs, base_image_tag},
    network::{GATEWAY_HOST, GATEWAY_MODEL_PORT},
    transfer::{self, PullOutcome},
};

struct Policy {
    allowlist: Allowlist,
    blocked: Mutex<Vec<String>>,
}

#[async_trait]
impl EgressPolicy for Policy {
    async fn check(
        &self,
        user: &str,
        token: &str,
        host: &str,
        port: u16,
        plain_http: bool,
    ) -> EgressVerdict {
        if user != "run" || token != "secret" {
            return EgressVerdict::Unauthorized;
        }
        if !plain_http && port == 443 && self.allowlist.allows(host) {
            return EgressVerdict::Allowed;
        }
        self.blocked.lock().unwrap().push(host.to_string());
        EgressVerdict::Blocked {
            reason: format!("{host}:{port} is not allowlisted"),
        }
    }

    async fn report_blocked(&self, _user: &str, host: &str, _port: u16, _reason: &str) {
        self.blocked.lock().unwrap().push(host.to_string());
    }
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

async fn sh(sandbox: &DockerSandbox, id: &str, script: &str) -> (i64, String) {
    let out = sandbox
        .exec(
            id,
            &["sh", "-c", script],
            &ExecOptions {
                user: Some("node".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    (
        out.exit_code,
        format!(
            "{}{}",
            out.stdout_str(),
            String::from_utf8_lossy(&out.stderr)
        ),
    )
}

#[tokio::test]
async fn sandbox_isolation_and_git_round_trip() {
    if std::env::var("KANBAIN_DOCKER_TESTS").as_deref() != Ok("1") {
        eprintln!("skipping: set KANBAIN_DOCKER_TESTS=1");
        return;
    }
    let (sandbox, info) = DockerSandbox::connect().await.expect("docker");
    let image = match std::env::var("KANBAIN_TEST_AGENT_IMAGE") {
        Ok(image) => image,
        Err(_) => {
            let args = BaseImageArgs {
                claude_code_package: "@anthropic-ai/claude-code".into(),
                claude_code_version: "2.1.119".into(),
                opencode_package: "opencode-ai".into(),
                opencode_version: "1.4.7".into(),
            };
            let tag = base_image_tag("test", &args);
            sandbox
                .ensure_base_image(&tag, &args, |line| eprintln!("{line}"))
                .await
                .unwrap();
            tag
        }
    };

    let instance = format!("t{}", std::process::id());
    let network = sandbox
        .ensure_networks(&instance, info.is_docker_desktop)
        .await
        .unwrap();

    // Stand-in model proxy and the real egress proxy on the bind address.
    let model_listener = tokio::net::TcpListener::bind((network.bind_ip, 0))
        .await
        .unwrap();
    let model_port = model_listener.local_addr().unwrap().port();
    let app = axum::Router::new().fallback(|| async { "model-proxy-ok" });
    tokio::spawn(async move { axum::serve(model_listener, app).await });

    let egress_listener = egress::bind(network.bind_ip).await.unwrap();
    let egress_port = egress_listener.local_addr().unwrap().port();
    let policy = Arc::new(Policy {
        allowlist: Allowlist::with_defaults(Vec::<String>::new()),
        blocked: Mutex::new(vec![]),
    });
    tokio::spawn(egress::serve(egress_listener, policy.clone()));

    let gateway = sandbox
        .start_gateway(&network, &image, model_port, egress_port)
        .await
        .unwrap();

    let proxy_url = network.egress_proxy_url("run", "secret");
    let agent = sandbox
        .create(&ContainerSpec {
            name: format!("kanbain-test-agent-{instance}"),
            image: image.clone(),
            labels: HashMap::from([
                (LABEL_MANAGED.into(), "true".into()),
                (LABEL_INSTANCE.into(), instance.clone()),
                (LABEL_KIND.into(), "agent".into()),
            ]),
            env: vec![
                ("HTTPS_PROXY".into(), proxy_url.clone()),
                ("HTTP_PROXY".into(), proxy_url.clone()),
                ("NO_PROXY".into(), GATEWAY_HOST.into()),
            ],
            networks: vec![(network.internal_network.clone(), vec![])],
            volumes: vec!["/kanbain".into()],
            user: Some("node".into()),
            ..Default::default()
        })
        .await
        .unwrap();

    let result = std::panic::AssertUnwindSafe(async {
        // The model proxy is reachable through the gateway.
        let (code, out) = sh(
            &sandbox,
            &agent,
            &format!("curl -sS --max-time 10 http://{GATEWAY_HOST}:{GATEWAY_MODEL_PORT}/v1/x"),
        )
        .await;
        assert_eq!(code, 0, "{out}");
        assert_eq!(out, "model-proxy-ok");

        // A non-allowlisted host is refused by the egress proxy and logged.
        let (code, out) = sh(
            &sandbox,
            &agent,
            "curl -sS --max-time 10 https://example.com",
        )
        .await;
        assert_ne!(code, 0, "{out}");
        assert!(out.contains("403"), "{out}");
        assert_eq!(*policy.blocked.lock().unwrap(), vec!["example.com"]);

        // An allowlisted registry works through the proxy.
        let (code, out) = sh(
            &sandbox,
            &agent,
            "curl -sS --max-time 20 -o /dev/null -w '%{http_code}' https://registry.npmjs.org/",
        )
        .await;
        assert_eq!((code, out.as_str()), (0, "200"));

        // Bypassing the proxy goes nowhere: no route out, not even to the host.
        let (code, _) = sh(
            &sandbox,
            &agent,
            "curl -sS --noproxy '*' --max-time 5 https://1.1.1.1",
        )
        .await;
        assert_ne!(code, 0);
        let (code, _) = sh(
            &sandbox,
            &agent,
            &format!(
                "curl -sS --noproxy '*' --max-time 5 http://{}:{model_port}/",
                network.bind_ip
            ),
        )
        .await;
        assert_ne!(code, 0);

        // No host mounts; only anonymous volumes.
        let inspect = sandbox
            .client()
            .inspect_container(&agent, None)
            .await
            .unwrap();
        for mount in inspect.mounts.unwrap_or_default() {
            assert_eq!(
                mount.typ,
                Some("volume".to_string()),
                "{mount:?}"
            );
        }
        assert!(
            inspect
                .host_config
                .and_then(|h| h.binds)
                .unwrap_or_default()
                .is_empty()
        );

        // Git round trip.
        let repo = tempfile::tempdir().unwrap();
        let repo = repo.path();
        git(repo, &["init", "-q", "-b", "main"]);
        git(repo, &["config", "user.email", "t@example.com"]);
        git(repo, &["config", "user.name", "T"]);
        std::fs::write(repo.join("a.txt"), "a").unwrap();
        git(repo, &["add", "."]);
        git(repo, &["commit", "-q", "-m", "init"]);
        git(repo, &["checkout", "-q", "-b", "kanbain/KAN-1-test"]);

        let head = transfer::push_branch(&sandbox, &agent, repo, "app", "kanbain/KAN-1-test")
            .await
            .unwrap();
        assert_eq!(head, git(repo, &["rev-parse", "HEAD"]));
        assert_eq!(
            transfer::pull_branch(&sandbox, &agent, repo, "app", "kanbain/KAN-1-test")
                .await
                .unwrap(),
            PullOutcome::UpToDate
        );

        let (code, out) = sh(
            &sandbox,
            &agent,
            "cd /workspace/app && echo agent > b.txt && git add b.txt && git commit -qm 'KAN-1: add b'",
        )
        .await;
        assert_eq!(code, 0, "{out}");
        sh(&sandbox, &agent, "echo dirty > /workspace/app/c.txt").await;
        assert!(
            transfer::commit_all(&sandbox, &agent, "app", "KAN-1: leftover changes")
                .await
                .unwrap()
        );

        let outcome = transfer::pull_branch(&sandbox, &agent, repo, "app", "kanbain/KAN-1-test")
            .await
            .unwrap();
        assert!(matches!(outcome, PullOutcome::Updated { .. }));
        assert_eq!(std::fs::read_to_string(repo.join("b.txt")).unwrap(), "agent\n");
        assert_eq!(std::fs::read_to_string(repo.join("c.txt")).unwrap(), "dirty\n");
        assert_eq!(git(repo, &["log", "--format=%s", "-1"]), "KAN-1: leftover changes");

        // A host-side commit flows back in on the next push.
        std::fs::write(repo.join("d.txt"), "host").unwrap();
        git(repo, &["add", "."]);
        git(repo, &["commit", "-q", "-m", "host edit"]);
        transfer::push_branch(&sandbox, &agent, repo, "app", "kanbain/KAN-1-test")
            .await
            .unwrap();
        let (_, out) = sh(&sandbox, &agent, "cat /workspace/app/d.txt").await;
        assert_eq!(out, "host");

        // Context dir: root-owned files are read-only for the agent.
        sandbox
            .copy_in(
                &agent,
                "/kanbain/context",
                vec![sandbox::FileEntry::new("ticket.md", "# KAN-1").owned_by_root()],
            )
            .await
            .unwrap();
        let (code, _) = sh(&sandbox, &agent, "echo x >> /kanbain/context/ticket.md").await;
        assert_ne!(code, 0);
        let (code, out) = sh(&sandbox, &agent, "echo '{}' > /kanbain/out/plan.json").await;
        assert_eq!(code, 0, "{out}");
        let files = sandbox.copy_out(&agent, "/kanbain/out").await.unwrap();
        assert_eq!(files, vec![("out/plan.json".to_string(), b"{}\n".to_vec())]);
    })
    .catch_unwind_compat()
    .await;

    sandbox.remove(&agent).await.unwrap();
    sandbox.remove(&gateway).await.unwrap();
    sandbox.remove_network_setup(&instance).await.unwrap();
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}

/// `catch_unwind` for futures without pulling in `futures::FutureExt` names
/// that clash with tokio's.
trait CatchUnwindCompat: std::future::Future + Sized {
    fn catch_unwind_compat(self) -> futures::future::CatchUnwind<Self>
    where
        Self: std::panic::UnwindSafe,
    {
        futures::FutureExt::catch_unwind(self)
    }
}

impl<F: std::future::Future> CatchUnwindCompat for F {}
