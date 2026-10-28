//! Sandbox networking (D38).
//!
//! Agent containers sit on an `internal` Docker network with no route to the
//! host or the internet. A gateway sidecar is attached to that network (as
//! `kanbain-gateway`) and to a normal bridge network, and forwards two fixed
//! ports to the backend's model proxy and egress proxy. Agents can reach
//! nothing else.
//!
//! The backend has to listen where the sidecar can reach it:
//! - native Linux Docker: the egress network's bridge gateway IP, which is a
//!   host address only containers on that network (and the host) can reach;
//! - Docker Desktop (Windows/macOS): containers run in a VM, so the sidecar
//!   dials `host.docker.internal` and the backend listens on 127.0.0.1.

use std::{collections::HashMap, net::IpAddr};

use bollard::{models::NetworkCreateRequest, query_parameters::InspectNetworkOptions};

use crate::{
    ContainerSpec, DockerSandbox, LABEL_INSTANCE, LABEL_KIND, LABEL_MANAGED, Limits, Result,
    Sandbox, SandboxError, docker::is_not_found,
};

/// DNS name of the gateway inside the sandbox network.
pub const GATEWAY_HOST: &str = "kanbain-gateway";
/// Gateway port that forwards to the model proxy.
pub const GATEWAY_MODEL_PORT: u16 = 4000;
/// Gateway port that forwards to the egress proxy.
pub const GATEWAY_EGRESS_PORT: u16 = 4001;

#[derive(Debug, Clone)]
pub struct SandboxNetwork {
    pub instance: String,
    /// Internal network the agent containers join.
    pub internal_network: String,
    /// Bridge network the gateway uses to reach the backend.
    pub egress_network: String,
    /// Address the backend's proxies must listen on.
    pub bind_ip: IpAddr,
    /// Host the gateway forwards to.
    pub upstream_host: String,
}

impl SandboxNetwork {
    pub fn model_proxy_url(&self) -> String {
        format!("http://{GATEWAY_HOST}:{GATEWAY_MODEL_PORT}")
    }

    /// Proxy URL for `HTTPS_PROXY`, carrying the run's egress credentials.
    pub fn egress_proxy_url(&self, user: &str, token: &str) -> String {
        format!("http://{user}:{token}@{GATEWAY_HOST}:{GATEWAY_EGRESS_PORT}")
    }
}

impl DockerSandbox {
    /// Create (or reuse) the internal and egress networks for `instance` and
    /// work out where the backend's proxies must listen.
    pub async fn ensure_networks(
        &self,
        instance: &str,
        is_docker_desktop: bool,
    ) -> Result<SandboxNetwork> {
        let internal_network = format!("kanbain-sandbox-{instance}");
        let egress_network = format!("kanbain-egress-{instance}");
        self.ensure_network(&internal_network, instance, true)
            .await?;
        let egress_gateway = self
            .ensure_network(&egress_network, instance, false)
            .await?;

        let (bind_ip, upstream_host) = match (
            std::env::var("KANBAIN_SANDBOX_BIND_IP").ok(),
            std::env::var("KANBAIN_SANDBOX_UPSTREAM_HOST").ok(),
        ) {
            (Some(ip), Some(host)) => (
                ip.parse().map_err(|_| {
                    SandboxError::Other(format!("bad KANBAIN_SANDBOX_BIND_IP {ip}"))
                })?,
                host,
            ),
            _ if is_docker_desktop => (
                IpAddr::from([127, 0, 0, 1]),
                "host.docker.internal".to_string(),
            ),
            _ => {
                let ip = egress_gateway.ok_or_else(|| {
                    SandboxError::Other(format!("network {egress_network} has no gateway"))
                })?;
                (ip, ip.to_string())
            }
        };

        Ok(SandboxNetwork {
            instance: instance.to_string(),
            internal_network,
            egress_network,
            bind_ip,
            upstream_host,
        })
    }

    /// Returns the network's gateway IP.
    async fn ensure_network(
        &self,
        name: &str,
        instance: &str,
        internal: bool,
    ) -> Result<Option<IpAddr>> {
        let existing = match self
            .client()
            .inspect_network(name, None::<InspectNetworkOptions>)
            .await
        {
            Ok(network) => Some(network),
            Err(e) if is_not_found(&e) => None,
            Err(e) => return Err(e.into()),
        };
        let network = match existing {
            Some(network) if network.internal == Some(internal) => network,
            other => {
                if other.is_some() {
                    // Never reuse a network whose isolation differs from what we need.
                    self.client().remove_network(name).await?;
                }
                self.client()
                    .create_network(NetworkCreateRequest {
                        name: name.to_string(),
                        driver: Some("bridge".into()),
                        internal: Some(internal),
                        labels: Some(HashMap::from([
                            (LABEL_MANAGED.to_string(), "true".to_string()),
                            (LABEL_INSTANCE.to_string(), instance.to_string()),
                        ])),
                        ..Default::default()
                    })
                    .await?;
                self.client()
                    .inspect_network(name, None::<InspectNetworkOptions>)
                    .await?
            }
        };
        Ok(network
            .ipam
            .and_then(|ipam| ipam.config)
            .and_then(|configs| configs.into_iter().find_map(|c| c.gateway))
            .and_then(|gw| gw.parse().ok()))
    }

    /// Start the gateway sidecar forwarding to the proxies on the host.
    /// Any previous gateway for this instance is replaced, since the proxy
    /// ports change with every backend launch.
    pub async fn start_gateway(
        &self,
        network: &SandboxNetwork,
        image: &str,
        model_proxy_port: u16,
        egress_proxy_port: u16,
    ) -> Result<String> {
        let name = format!("kanbain-gateway-{}", network.instance);
        self.remove(&name).await?;
        let upstream = &network.upstream_host;
        let script = format!(
            "socat TCP-LISTEN:{GATEWAY_MODEL_PORT},fork,reuseaddr TCP:{upstream}:{model_proxy_port} & \
             socat TCP-LISTEN:{GATEWAY_EGRESS_PORT},fork,reuseaddr TCP:{upstream}:{egress_proxy_port} & \
             wait"
        );
        let spec = ContainerSpec {
            name,
            image: image.to_string(),
            labels: HashMap::from([
                (LABEL_MANAGED.to_string(), "true".to_string()),
                (LABEL_INSTANCE.to_string(), network.instance.clone()),
                (LABEL_KIND.to_string(), "gateway".to_string()),
            ]),
            limits: Limits {
                cpus: 0.5,
                memory_mb: 128,
                pids: 256,
            },
            networks: vec![
                (network.egress_network.clone(), vec![]),
                (
                    network.internal_network.clone(),
                    vec![GATEWAY_HOST.to_string()],
                ),
            ],
            cmd: Some(vec!["sh".into(), "-c".into(), script]),
            extra_hosts: vec!["host.docker.internal:host-gateway".into()],
            ..Default::default()
        };
        self.create(&spec).await
    }

    /// Remove the gateway and both networks of `instance` (tests and
    /// uninstall). Agent containers must be gone first.
    pub async fn remove_network_setup(&self, instance: &str) -> Result<()> {
        self.remove(&format!("kanbain-gateway-{instance}")).await?;
        for name in [
            format!("kanbain-sandbox-{instance}"),
            format!("kanbain-egress-{instance}"),
        ] {
            match self.client().remove_network(&name).await {
                Ok(()) => {}
                Err(e) if is_not_found(&e) => {}
                Err(e) => return Err(e.into()),
            }
        }
        Ok(())
    }
}
