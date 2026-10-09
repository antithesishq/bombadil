use anyhow::{Result, anyhow, bail};
use crossbeam_channel as mpmc;
use serde::Deserialize;
use std::{
    io::{BufRead, BufReader},
    net::{SocketAddr, TcpListener},
    path::PathBuf,
    process::{self, Stdio},
    str::FromStr,
    thread,
    time::Duration,
};
use tempfile::TempDir;
use url::Url;

#[cfg(unix)]
use std::os::unix::process::CommandExt;

pub mod locate;

#[derive(Clone)]
pub struct LaunchOptions {
    pub executable: PathBuf,
    pub headless: bool,
    pub no_sandbox: bool,
}

pub struct Chromium {
    pub web_socket_remote_debugger: Url,
    _process: Option<ChromiumProcess>,
}

/// A launched browser process and its temporary directories. Dropping it
/// kills the process and then removes the directories.
struct ChromiumProcess {
    child: process::Child,
    _user_data_directory: TempDir,
    _crash_dumps_dir: TempDir,
}

impl Chromium {
    pub fn connect(remote_debugger: &Url) -> Result<Self> {
        Ok(Chromium {
            web_socket_remote_debugger:
                web_socket_remote_debugger_get_with_attempts(
                    remote_debugger,
                    5,
                )?,
            _process: None,
        })
    }

    pub fn launch(launch_options: &LaunchOptions) -> Result<Self> {
        let user_data_directory = TempDir::with_prefix("chrome_user_data_")?;
        log::info!(
            "storing chromium/chrome user data in {}",
            user_data_directory.path().display()
        );

        let crash_dumps_dir = TempDir::with_prefix("chrome_chrash_dumps_")?;

        let mut command = process::Command::new(
            launch_options
                .executable
                .to_str()
                .ok_or(anyhow!("invalid chromium executable path"))?,
        );

        #[cfg(unix)]
        command.process_group(0); // Spawn in a new process group.

        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());

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
            user_data_directory
                .path()
                .to_path_buf()
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

        command.arg("--enable-logging");
        command.arg("--v=1");
        command.arg("--no-crashpad");
        command.arg("--disable-background-networking");
        command.arg("--disable-component-update");
        command.arg("--disable-domain-reliability");
        command.arg("--no-pings");
        command.arg("--disable-crash-reporter");

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
        // Owned from here on so that any early return below kills the
        // process instead of orphaning it.
        let mut process = ChromiumProcess {
            child: command.spawn()?,
            _user_data_directory: user_data_directory,
            _crash_dumps_dir: crash_dumps_dir,
        };

        let (listening_tx, listening_rx) = mpmc::bounded(1);
        {
            let stderr = process.child.stderr.take().ok_or(anyhow!(
                "failed to get stderr from chromium/chrome process"
            ))?;
            thread::spawn(move || {
                let stderr = BufReader::new(stderr);
                for line_result in stderr.lines() {
                    let Ok(line) = line_result else {
                        break;
                    };
                    log::debug!("chromium stderr: {line}");
                    if line.starts_with("DevTools listening on")
                        && let Err(error) = listening_tx.send(())
                    {
                        log::error!("failed sending listening signal: {error}");
                    }
                }
            });
        }

        if listening_rx.recv_timeout(Duration::from_secs(5)).is_err() {
            bail!(
                "timed out while waiting for chrome/chromium to log 'DevTools listening on ...' message"
            );
        }

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
            _process: Some(process),
        })
    }
}

impl Drop for ChromiumProcess {
    fn drop(&mut self) {
        let child = &mut self.child;
        // Kill the whole process group, not just the main process, so
        // that helper processes (renderer, GPU, network service) don't
        // outlive it. These linger on macOS and keep writing to the user
        // data directory.
        #[cfg(unix)]
        let result = {
            // SAFETY: plain syscall; the child was spawned as the leader
            // of its own process group, so its pid is the group id.
            let pid = child.id() as libc::pid_t;
            if unsafe { libc::killpg(pid, libc::SIGKILL) } == 0 {
                Ok(())
            } else {
                Err(std::io::Error::last_os_error())
            }
        };
        #[cfg(not(unix))]
        let result = child.kill();
        if let Err(error) = result {
            log::error!("failed to kill chromium/chrome process: {}", error);
        } else {
            log::info!("killed chromium/chrome process");
        }
        if let Err(error) = child.wait() {
            log::error!(
                "failed to await killed chromium/chrome process: {}",
                error
            );
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
        match web_socket_remote_debugger_get(remote_debugger) {
            Ok(url) => return Ok(url),
            Err(error) => {
                log::debug!(
                    "get web_socket_remote_debugger ({remote_debugger}) attempt {n} failed: {error:#}"
                );
            }
        }
    }
    bail!(
        "failed to get web_socket_remote_debugger URL ({remote_debugger}) after {attempts} attempts",
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
