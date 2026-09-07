use anyhow::{Result, anyhow, bail};
use serde::Deserialize;
use std::{
    net::{SocketAddr, TcpListener},
    path::PathBuf,
    process::{self, Stdio},
    str::FromStr,
    thread,
    time::Duration,
};
use tempfile::TempDir;
use url::Url;

pub mod locate;

#[derive(Clone)]
pub struct LaunchOptions {
    pub executable: PathBuf,
    pub headless: bool,
    pub user_data_directory: PathBuf,
    pub no_sandbox: bool,
}

pub struct Chromium {
    pub web_socket_remote_debugger: Url,
    process_child: Option<process::Child>,
}

impl Chromium {
    pub fn connect(remote_debugger: Url) -> Result<Self> {
        Ok(Chromium {
            web_socket_remote_debugger:
                web_socket_remote_debugger_get_with_attempts(
                    &remote_debugger,
                    5,
                )?,
            process_child: None,
        })
    }

    pub fn launch(launch_options: LaunchOptions) -> Result<Self> {
        let crash_dumps_dir = TempDir::new()?;

        let mut command = process::Command::new(
            launch_options
                .executable
                .to_str()
                .ok_or(anyhow!("invalid chromium executable path"))?,
        );

        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());

        if launch_options.no_sandbox {
            command.arg("--no-sandbox");
            command.arg("--disable-setuid-sandbox");
            command.arg("--disable-dev-shm-usage");
        }

        if launch_options.headless {
            command.arg("--headless");
        }

        command.arg(format!(
            "--user-data-dir={}",
            launch_options
                .user_data_directory
                .to_str()
                .ok_or(anyhow!("invalid user_data_dir"))?,
        ));

        command.arg(format!(
            "--crash-dumps-dir={}",
            crash_dumps_dir
                .path()
                .to_path_buf()
                .to_str()
                .expect("invalid tmp dir path"),
        ));

        apply_managed_chrome_arguments(&mut command);

        let remote_debugging_port: u16 = available_port().ok_or(anyhow!("failed to find available port for remote debugging server in chromium"))?;
        command
            .arg(format!("--remote-debugging-port={}", remote_debugging_port));

        log::info!(
            "spawning: {} {}",
            command.get_program().to_string_lossy(),
            command
                .get_args()
                .map(|s| s.to_string_lossy())
                .collect::<Vec<_>>()
                .join(" ")
        );
        let child = command.spawn()?;

        let mut remote_debugger = Url::from_str("http://127.0.0.1")?;
        remote_debugger
            .set_port(Some(remote_debugging_port))
            .map_err(|_| anyhow!("failed to set port"))?;

        Ok(Chromium {
            web_socket_remote_debugger:
                web_socket_remote_debugger_get_with_attempts(
                    &remote_debugger,
                    5,
                )?,
            process_child: Some(child),
        })
    }
}

fn apply_managed_chrome_arguments(command: &mut process::Command) {
    command.args([
        "--enable-logging",
        "--v=1",
        "--no-crashpad",
        "--disable-background-networking",
        "--disable-component-update",
        "--disable-domain-reliability",
        "--no-pings",
        "--disable-crash-reporter",
        "--disable-features=OptimizationHints",
    ]);
}

impl Drop for Chromium {
    fn drop(&mut self) {
        if let Some(mut child) = self.process_child.take()
            && let Err(error) = child.kill()
        {
            log::error!("failed to kill chromium/chrome process: {}", error);
        } else {
            log::info!("killed chromium/chrome process");
        }
    }
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct ChromiumVersionResponse {
    #[serde(rename = "Browser")]
    browser: String,
    #[serde(rename = "Protocol-Version")]
    protocol_version: String,
    #[serde(rename = "User-Agent")]
    user_agent: String,
    #[serde(rename = "V8-Version")]
    v8_version: String,
    #[serde(rename = "WebKit-Version")]
    webkit_version: String,
    #[serde(rename = "webSocketDebuggerUrl")]
    web_socket_remote_debugger: Url,
}

fn web_socket_remote_debugger_get_with_attempts(
    remote_debugger: &Url,
    attempts: usize,
) -> Result<Url> {
    for n in 1..=attempts {
        thread::sleep(Duration::from_millis(n as u64 * 200));
        log::debug!("get web_socket_remote_debugger attempt {n}");
        if let Ok(url) = web_socket_remote_debugger_get(remote_debugger) {
            return Ok(url);
        }
    }
    bail!(
        "failed to get web_socket_remote_debugger URL after {attempts} attempts"
    )
}

fn web_socket_remote_debugger_get(remote_debugger: &Url) -> Result<Url> {
    assert!(
        remote_debugger.path() == "/",
        "remote_debugger url must be an HTTP scheme URL without a path, e.g. http://localhost:9222"
    );
    let response: ChromiumVersionResponse =
        reqwest::blocking::get(remote_debugger.join("/json/version")?)?
            .json()?;

    log::debug!("got /json/version response: {:?}", response);
    Ok(response.web_socket_remote_debugger)
}

fn available_port() -> Option<u16> {
    TcpListener::bind("127.0.0.1:0")
        .ok()
        .and_then(|listener| listener.local_addr().ok())
        .map(|addr: SocketAddr| addr.port())
}

#[cfg(test)]
mod tests {
    use super::apply_managed_chrome_arguments;
    use std::process::Command;

    #[test]
    fn managed_chrome_arguments_disable_optimization_hints_once() {
        let mut command = Command::new("chromium");
        apply_managed_chrome_arguments(&mut command);
        let arguments = command
            .get_args()
            .map(|argument| argument.to_str().unwrap())
            .collect::<Vec<_>>();
        for expected in [
            "--enable-logging",
            "--v=1",
            "--no-crashpad",
            "--disable-background-networking",
            "--disable-component-update",
            "--disable-domain-reliability",
            "--no-pings",
            "--disable-crash-reporter",
        ] {
            assert!(arguments.contains(&expected));
        }
        let disable_features = arguments
            .iter()
            .filter(|argument| {
                **argument == "--disable-features"
                    || argument.starts_with("--disable-features=")
            })
            .copied()
            .collect::<Vec<_>>();
        assert_eq!(disable_features, ["--disable-features=OptimizationHints"]);
    }
}
