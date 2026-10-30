//! "Connect Claude account" (D33): runs `claude setup-token` in a
//! throwaway sandbox container, shows its sign-in URL in the UI, passes the
//! code the user pastes back, and stores the resulting long-lived token
//! encrypted. The model proxy then uses it instead of an API key.
//!
//! `setup-token` is an interactive terminal program, so it runs under a
//! host pseudo-terminal (`docker exec -it`). The terminal is very wide so
//! the URL and token are never wrapped.

use std::{
    io::{Read, Write},
    sync::{Arc, Mutex},
    time::Duration,
};

use executors::versions::{CLAUDE_CODE_PACKAGE, CLAUDE_CODE_VERSION};
use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};
use sandbox::AGENT_USER;
use serde::Serialize;
use services::services::secrets::{CLAUDE_OAUTH_TOKEN, SecretStore};
use ts_rs::TS;

use crate::sandbox::{LoginContainer, SandboxRuntime};

/// How long a login may wait for the user.
const LOGIN_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const PTY_COLS: u16 = 1000;
/// Quiet time after which the terminal UI is taken as settled.
const SETTLE: Duration = Duration::from_millis(1500);

#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ClaudeLoginState {
    Idle,
    Starting,
    /// Open `url`, sign in, and paste the code shown afterwards.
    WaitingForCode {
        url: String,
    },
    Exchanging,
    Connected,
    Failed {
        message: String,
    },
}

struct Session {
    id: u64,
    writer: Box<dyn Write + Send>,
    killer: Box<dyn portable_pty::ChildKiller + Send + Sync>,
}

#[derive(Clone)]
pub struct ClaudeLogin {
    sandbox: SandboxRuntime,
    secrets: Arc<SecretStore>,
    state: Arc<Mutex<ClaudeLoginState>>,
    session: Arc<Mutex<Option<Session>>>,
    next_id: Arc<Mutex<u64>>,
}

/// The sign-in URL in `setup-token`'s output, once complete. A narrow
/// terminal wraps it over several lines (and the terminal UI may draw the
/// breaks as cursor movements), so read it with whitespace removed, up to
/// the paste prompt that follows it.
fn find_url(output: &str) -> Option<String> {
    let compact: String = output.chars().filter(|c| !c.is_whitespace()).collect();
    let start = compact.rfind("https://")?;
    let rest = &compact[start..];
    let url = &rest[..rest.find("Pastecodehere")?];
    let url = url.trim_end_matches(['.', ',', ')']);
    (url.contains("/oauth/authorize?") && url.contains("state=")).then(|| url.to_string())
}

/// Whether `setup-token` shows its "Paste code here if prompted >" input.
/// Cursor movements stand in for its spaces, so compare without them.
fn paste_prompt_shown(output: &str) -> bool {
    let compact: String = output.chars().filter(|c| !c.is_whitespace()).collect();
    compact.contains("Pastecodehereifprompted>")
}

/// The OAuth token in `setup-token`'s output.
fn find_token(output: &str) -> Option<String> {
    let start = output.find("sk-ant-oat")?;
    let token: String = output[start..]
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    (token.len() > 40).then_some(token)
}

/// The `OAuth error: …` line `setup-token` shows when the exchange fails.
/// Its spaces may be cursor movements, which stripping removes, and the
/// line may end in cursor movements too; once the output has `settled`,
/// the rest of it is the message.
fn find_error(output: &str, settled: bool) -> Option<String> {
    let start = output
        .rfind("OAuth error:")
        .or_else(|| output.rfind("OAutherror:"))?;
    let rest = &output[start..];
    let line = match rest.find(['\r', '\n']) {
        Some(end) => &rest[..end],
        None if settled => rest,
        None => return None,
    }
    .trim();
    let message = line
        .trim_start_matches("OAuth error:")
        .trim_start_matches("OAutherror:")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let message = message
        .split("Press Enter")
        .next()
        .unwrap_or_default()
        .trim();
    Some(format!("Claude sign-in failed: {message}"))
}

/// The last few non-empty lines, for error messages.
fn tail(output: &str) -> String {
    let lines: Vec<&str> = output
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    lines[lines.len().saturating_sub(4)..].join("\n")
}

impl ClaudeLogin {
    pub fn new(sandbox: SandboxRuntime, secrets: Arc<SecretStore>) -> Self {
        Self {
            sandbox,
            secrets,
            state: Arc::new(Mutex::new(ClaudeLoginState::Idle)),
            session: Arc::new(Mutex::new(None)),
            next_id: Arc::new(Mutex::new(0)),
        }
    }

    pub fn state(&self) -> ClaudeLoginState {
        self.state.lock().unwrap().clone()
    }

    fn set_state(&self, id: u64, state: ClaudeLoginState) {
        // A cancelled or replaced session must not overwrite a newer one.
        if *self.next_id.lock().unwrap() == id {
            *self.state.lock().unwrap() = state;
        }
    }

    /// Start a login, replacing any login in progress.
    pub async fn start(&self) -> ClaudeLoginState {
        self.cancel();
        let id = {
            let mut next = self.next_id.lock().unwrap();
            *next += 1;
            *next
        };
        *self.state.lock().unwrap() = ClaudeLoginState::Starting;
        let login = self.clone();
        tokio::spawn(async move {
            let container = match login.sandbox.create_login_container().await {
                Ok(container) => container,
                Err(e) => {
                    login.set_state(
                        id,
                        ClaudeLoginState::Failed {
                            message: e.to_string(),
                        },
                    );
                    return;
                }
            };
            let result = login.run(id, &container).await;
            login.sandbox.remove_login_container(&container).await;
            let state = match result {
                Ok(token) => match login.secrets.set(CLAUDE_OAUTH_TOKEN, &token).await {
                    Ok(()) => ClaudeLoginState::Connected,
                    Err(e) => ClaudeLoginState::Failed {
                        message: format!("Storing the token failed: {e}"),
                    },
                },
                Err(message) => ClaudeLoginState::Failed { message },
            };
            login.session.lock().unwrap().take_if(|s| s.id == id);
            login.set_state(id, state);
        });
        self.state()
    }

    /// Drive `setup-token` until it prints a token or fails.
    async fn run(&self, id: u64, container: &LoginContainer) -> Result<String, String> {
        let pty = NativePtySystem::default()
            .openpty(PtySize {
                rows: 40,
                cols: PTY_COLS,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| e.to_string())?;
        let mut cmd = CommandBuilder::new(sandbox::DOCKER_CLI);
        cmd.args(["exec", "-it", "-u", AGENT_USER]);
        for (key, value) in &container.env {
            cmd.args(["-e", &format!("{key}={value}")]);
        }
        cmd.args(["-e", "TERM=xterm-256color"]);
        cmd.args(["-e", &format!("COLUMNS={PTY_COLS}")]);
        cmd.arg(&container.container);
        cmd.args([
            "npx",
            "--offline",
            "-y",
            &format!("{CLAUDE_CODE_PACKAGE}@{CLAUDE_CODE_VERSION}"),
            "setup-token",
        ]);
        let mut child = pty
            .slave
            .spawn_command(cmd)
            .map_err(|e| spawn_error(&e.to_string()))?;
        drop(pty.slave);
        let writer = pty.master.take_writer().map_err(|e| e.to_string())?;
        let mut reader = pty.master.try_clone_reader().map_err(|e| e.to_string())?;
        *self.session.lock().unwrap() = Some(Session {
            id,
            writer,
            killer: child.clone_killer(),
        });

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                    break;
                }
            }
        });

        let mut raw = Vec::new();
        let mut shown_url = false;
        let mut pending_url = None;
        let deadline = tokio::time::Instant::now() + LOGIN_TIMEOUT;
        loop {
            // Keys typed while the terminal UI redraws are lost, so ask for
            // the code only once the paste prompt has been on screen and the
            // output quiet for a moment.
            let wait = tokio::time::Instant::now() + SETTLE;
            let chunk = match tokio::time::timeout_at(wait.min(deadline), rx.recv()).await {
                Ok(Some(chunk)) => chunk,
                Ok(None) => break,
                Err(_) if tokio::time::Instant::now() >= deadline => {
                    let _ = child.kill();
                    let text = strip_ansi_escapes::strip_str(String::from_utf8_lossy(&raw));
                    return Err(format!(
                        "The login timed out. Last output:\n{}",
                        tail(&text)
                    ));
                }
                Err(_) => {
                    let text = strip_ansi_escapes::strip_str(String::from_utf8_lossy(&raw));
                    if let Some(error) = find_error(&text, true) {
                        let _ = child.kill();
                        return Err(error);
                    }
                    if let Some(url) = pending_url.take() {
                        shown_url = true;
                        self.set_state(id, ClaudeLoginState::WaitingForCode { url });
                    }
                    continue;
                }
            };
            raw.extend_from_slice(&chunk);
            let text = strip_ansi_escapes::strip_str(String::from_utf8_lossy(&raw));
            if let Some(token) = find_token(&text) {
                let _ = child.kill();
                return Ok(token);
            }
            // On a bad code it shows the error and waits for Enter to retry;
            // the user starts over from the UI instead.
            if let Some(error) = find_error(&text, false) {
                let _ = child.kill();
                return Err(error);
            }
            if !shown_url && paste_prompt_shown(&text) {
                pending_url = find_url(&text);
            }
        }
        let _ = tokio::task::spawn_blocking(move || child.wait()).await;
        let text = strip_ansi_escapes::strip_str(String::from_utf8_lossy(&raw));
        if self.state() == ClaudeLoginState::Idle {
            return Err("Cancelled.".into());
        }
        Err(format!(
            "claude setup-token ended without a token:\n{}",
            tail(&text)
        ))
    }

    /// Pass the code from the sign-in page to `setup-token`: type it, then
    /// press Enter separately (sent together, the terminal UI takes both as
    /// one paste and doesn't submit).
    pub async fn submit_code(&self, code: &str) -> Result<ClaudeLoginState, String> {
        self.type_into_session(code)?;
        tokio::time::sleep(Duration::from_millis(400)).await;
        self.press_enter()?;
        Ok(self.state())
    }

    fn press_enter(&self) -> Result<(), String> {
        let mut session = self.session.lock().unwrap();
        let session = session.as_mut().ok_or("The login ended.")?;
        session
            .writer
            .write_all(b"\r")
            .and_then(|_| session.writer.flush())
            .map_err(|e| e.to_string())
    }

    fn type_into_session(&self, code: &str) -> Result<(), String> {
        let code = code.trim();
        if code.is_empty() || code.contains(['\r', '\n']) {
            return Err("Paste the code shown after signing in.".into());
        }
        let mut session = self.session.lock().unwrap();
        let Some(session) = session.as_mut() else {
            return Err("No login is waiting for a code.".into());
        };
        if !matches!(self.state(), ClaudeLoginState::WaitingForCode { .. }) {
            return Err("No login is waiting for a code.".into());
        }
        session
            .writer
            .write_all(code.as_bytes())
            .and_then(|_| session.writer.flush())
            .map_err(|e| e.to_string())?;
        *self.state.lock().unwrap() = ClaudeLoginState::Exchanging;
        Ok(())
    }

    /// Stop a login in progress; its container is removed when it exits.
    pub fn cancel(&self) {
        if let Some(mut session) = self.session.lock().unwrap().take() {
            let _ = session.killer.kill();
        }
        *self.state.lock().unwrap() = ClaudeLoginState::Idle;
    }

    /// Forget the stored token.
    pub async fn disconnect(&self) -> Result<(), String> {
        self.cancel();
        self.secrets
            .remove(CLAUDE_OAUTH_TOKEN)
            .await
            .map_err(|e| e.to_string())
    }
}

/// A PTY spawn error without the command line: `portable_pty` quotes the
/// whole `docker exec`, including the login's proxy credential.
fn spawn_error(error: &str) -> String {
    let reason = error.rsplit_once(" failed: ").map_or(error, |(_, r)| r);
    format!("Starting the Docker CLI failed: {reason}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spawn_error_hides_the_command_line() {
        let raw = "CreateProcessW `\"docker\" exec -e HTTPS_PROXY=http://login:kbl-secret@gw:4001 \
                   x npx setup-token` in cwd `C:\\Users` failed: %1 ist keine zulässige \
                   Win32-Anwendung. (os error 193)";
        let shown = spawn_error(raw);
        assert!(!shown.contains("kbl-secret"), "{shown}");
        assert!(
            shown.ends_with("Win32-Anwendung. (os error 193)"),
            "{shown}"
        );
        assert_eq!(spawn_error("boom"), "Starting the Docker CLI failed: boom");
    }

    #[test]
    fn finds_url_and_token() {
        let out = "Browser didn't open? Use the url below to sign in:\n\n\
                   https://claude.com/cai/oauth/authorize?code=true&client_id=x&state=abc\n\n\
                   Paste code here if prompted >";
        assert_eq!(
            find_url(out).as_deref(),
            Some("https://claude.com/cai/oauth/authorize?code=true&client_id=x&state=abc")
        );
        assert_eq!(
            find_url("https://claude.com/cai/oauth/authorize?code=true&client"),
            None
        );
        // Wrapped at 80 columns, as a terminal whose size didn't reach the
        // container shows it.
        let wrapped = "Browser didn't open? Use the url below to sign in (c to copy)\r\n\r\n\
                       https://claude.com/cai/oauth/authorize?code=true&client_id=9d1c\r\n\
                       &redirect_uri=x&state=hXKL8B\r\n\
                       _QJr7\r\n\r\n\r\nPaste code here if prompted >";
        assert_eq!(
            find_url(wrapped).as_deref(),
            Some(
                "https://claude.com/cai/oauth/authorize?code=true&client_id=9d1c\
                 &redirect_uri=x&state=hXKL8B_QJr7"
            )
        );
        assert_eq!(
            find_url("https://claude.com/cai/oauth/authorize?code=true&state=ab"),
            None
        );
        let token = format!("sk-ant-oat01-{}", "a".repeat(80));
        assert_eq!(
            find_token(&format!("Your token:\n{token}\n\nStore it")).as_deref(),
            Some(token.as_str())
        );
        assert_eq!(find_token("sk-ant-oat01-short"), None);
        assert!(paste_prompt_shown("x\nPastecodehereifprompted>\n"));
        assert!(!paste_prompt_shown("Browser didn't open?"));
        assert_eq!(
            find_error(
                "x\nOAuth error: Invalid code\n\nPress Enter to retry.",
                false
            )
            .as_deref(),
            Some("Claude sign-in failed: Invalid code")
        );
        assert_eq!(
            find_error("\rOAutherror:ECONNREFUSED\r\rPressEntertoretry.\r\n", false).as_deref(),
            Some("Claude sign-in failed: ECONNREFUSED")
        );
        assert_eq!(find_error("OAuth error: Inva", false), None);
        assert_eq!(
            find_error(
                "OAuth error: Request failed  with status code 400   Press Enter to retry.",
                true
            )
            .as_deref(),
            Some("Claude sign-in failed: Request failed with status code 400")
        );
    }
}
