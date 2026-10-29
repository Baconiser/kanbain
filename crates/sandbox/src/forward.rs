//! Host port forwarding into a sandbox container, for dev server previews.
//!
//! The sandbox network is internal, and on Docker Desktop the host can't
//! reach container addresses at all. Instead, each connection to the
//! forwarded host port runs `docker exec -i <container> socat - TCP:…` and
//! pipes the bytes through, which works on every platform.

use std::{net::SocketAddr, process::Stdio};

use tokio::{
    io::{AsyncWriteExt, copy},
    net::{TcpListener, TcpStream},
    process::Command,
    task::JoinHandle,
};

/// Ports of local URLs in a line of dev server output, such as
/// `http://localhost:5173/` or `0.0.0.0:3000`.
pub fn detect_local_ports(line: &str) -> Vec<u16> {
    let mut ports = Vec::new();
    for host in ["localhost:", "127.0.0.1:", "0.0.0.0:", "[::]:", "[::1]:"] {
        let mut rest = line;
        while let Some(idx) = rest.find(host) {
            rest = &rest[idx + host.len()..];
            let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            if let Ok(port) = digits.parse::<u16>()
                && port >= 1024
                && !ports.contains(&port)
            {
                ports.push(port);
            }
        }
    }
    ports
}

/// Listen on `127.0.0.1:port` and tunnel each connection to the same port
/// inside `container`. Fails if the host port is taken.
pub async fn forward_port(container: String, port: u16) -> std::io::Result<JoinHandle<()>> {
    let listener = TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], port))).await?;
    Ok(serve(listener, container, port))
}

/// Tunnel a free `127.0.0.1` port to `port` inside `container`; returns the
/// host port. For servers the backend itself talks to, such as
/// `opencode serve`.
pub async fn forward_ephemeral(
    container: String,
    port: u16,
) -> std::io::Result<(u16, JoinHandle<()>)> {
    let listener = TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0))).await?;
    let host_port = listener.local_addr()?.port();
    Ok((host_port, serve(listener, container, port)))
}

fn serve(listener: TcpListener, container: String, port: u16) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                continue;
            };
            let container = container.clone();
            tokio::spawn(async move {
                if let Err(e) = tunnel(stream, &container, port).await {
                    tracing::debug!("Sandbox port forward {port} ended: {e}");
                }
            });
        }
    })
}

async fn tunnel(stream: TcpStream, container: &str, port: u16) -> std::io::Result<()> {
    let mut child = crate::hide_console(&mut Command::new("docker"))
        .args(["exec", "-i", container, "socat", "-"])
        .arg(format!("TCP:127.0.0.1:{port}"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()?;
    let mut child_in = child.stdin.take().expect("piped stdin");
    let mut child_out = child.stdout.take().expect("piped stdout");
    let (mut read, mut write) = stream.into_split();
    let upstream = async {
        copy(&mut read, &mut child_in).await?;
        child_in.shutdown().await
    };
    let downstream = async {
        copy(&mut child_out, &mut write).await?;
        write.shutdown().await
    };
    let _ = tokio::try_join!(upstream, downstream);
    let _ = child.kill().await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_dev_server_ports() {
        assert_eq!(
            detect_local_ports("  ➜  Local:   http://localhost:5173/"),
            vec![5173]
        );
        assert_eq!(
            detect_local_ports("Server listening on 0.0.0.0:3000 and http://127.0.0.1:3001"),
            vec![3001, 3000]
        );
        assert_eq!(detect_local_ports("ready on http://[::]:8080"), vec![8080]);
        assert!(detect_local_ports("http://localhost:80/ and localhost:abc").is_empty());
    }
}
