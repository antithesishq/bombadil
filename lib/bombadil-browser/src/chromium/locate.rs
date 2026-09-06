use anyhow::{Result, bail};
use std::env;
use std::path::{Path, PathBuf};

/// Try to locate a Chrome or Chromium executable on the system.
pub fn executable() -> Result<PathBuf> {
    if let Some(p) = env::var_os("CHROME") {
        let p = PathBuf::from(p);
        if is_executable(&p) {
            return Ok(p);
        }
    }

    // 2. Search PATH for common binary names
    let candidates_in_path = [
        "google-chrome-stable",
        "google-chrome",
        "chromium-browser",
        "chromium",
        "chrome",
    ];
    if let Some(path_var) = env::var_os("PATH") {
        for dir in env::split_paths(&path_var) {
            for name in &candidates_in_path {
                let full = exe_name(dir.join(name));
                if is_executable(&full) {
                    return Ok(full);
                }
            }
        }
    }

    // 3. Platform-specific standard install locations
    for p in platform_locations() {
        if is_executable(&p) {
            return Ok(p);
        }
    }

    bail!("failed to locate chromium/chrome executable")
}

#[cfg(target_os = "windows")]
fn exe_name(p: PathBuf) -> PathBuf {
    p.with_extension("exe")
}

#[cfg(not(target_os = "windows"))]
fn exe_name(p: PathBuf) -> PathBuf {
    p
}

#[cfg(target_os = "macos")]
fn platform_locations() -> Vec<PathBuf> {
    let mut v = vec![
        PathBuf::from(
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        ),
        PathBuf::from("/Applications/Chromium.app/Contents/MacOS/Chromium"),
        PathBuf::from(
            "/Applications/Google Chrome Canary.app/Contents/MacOS/Google Chrome Canary",
        ),
        PathBuf::from(
            "/Applications/Google Chrome Beta.app/Contents/MacOS/Google Chrome Beta",
        ),
    ];
    // Per-user installs under ~/Applications
    if let Some(home) = env::var_os("HOME") {
        let home = PathBuf::from(home);
        v.push(home.join(
            "Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        ));
        v.push(home.join("Applications/Chromium.app/Contents/MacOS/Chromium"));
    }
    v
}

#[cfg(target_os = "linux")]
fn platform_locations() -> Vec<PathBuf> {
    vec![
        PathBuf::from("/usr/bin/google-chrome-stable"),
        PathBuf::from("/usr/bin/google-chrome"),
        PathBuf::from("/usr/bin/chromium-browser"),
        PathBuf::from("/usr/bin/chromium"),
        PathBuf::from("/snap/bin/chromium"),
        PathBuf::from("/opt/google/chrome/chrome"),
        PathBuf::from("/usr/local/bin/chrome"),
        PathBuf::from("/usr/local/bin/chromium"),
    ]
}

#[cfg(target_os = "windows")]
fn platform_locations() -> Vec<PathBuf> {
    let mut v = Vec::new();
    let program_files_dirs = [
        env::var_os("PROGRAMFILES"),
        env::var_os("PROGRAMFILES(X86)"),
        env::var_os("LOCALAPPDATA"),
    ];
    let rel_paths = [
        r"Google\Chrome\Application\chrome.exe",
        r"Chromium\Application\chrome.exe",
    ];
    for pf in program_files_dirs.into_iter().flatten() {
        for rel in &rel_paths {
            v.push(PathBuf::from(&pf).join(rel));
        }
    }
    v
}

#[cfg(not(any(
    target_os = "macos",
    target_os = "linux",
    target_os = "windows"
)))]
fn platform_locations() -> Vec<PathBuf> {
    Vec::new()
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(windows)]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}
