use std::fs;
use std::path::Path;
use std::process::Stdio;

fn main() {
    let dist_directory = Path::new("../../target/inspect");

    println!("cargo:rerun-if-changed=../bombadil-inspect/src");
    println!("cargo:rerun-if-changed=../bombadil-inspect/Cargo.toml");
    println!("cargo:rerun-if-changed=../bombadil-inspect/index.html");
    println!("cargo:rerun-if-changed=../bombadil-inspect/Trunk.toml");

    build_inspect(dist_directory);
}

fn build_inspect(dist_directory: &Path) {
    let inspect_directory = Path::new("../bombadil-inspect");

    if !inspect_directory.join("Cargo.toml").exists() {
        ensure_placeholder(dist_directory);
        return;
    }

    let dist_absolute = fs::canonicalize("../../")
        .expect("Failed to resolve workspace root")
        .join("target/inspect");

    let wasm_target_directory = fs::canonicalize("../../")
        .expect("Failed to resolve workspace root")
        .join("target/inspect-wasm");

    let mut command = std::process::Command::new("trunk");
    command
        .arg("build")
        // trunk 0.21 treats --offline as optional true/false; pass explicitly.
        .arg("--offline")
        .arg("true")
        .arg("--dist")
        .arg(&dist_absolute)
        .env("CARGO_TARGET_DIR", &wasm_target_directory)
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .current_dir(inspect_directory);

    // Trunk 0.21 maps NO_COLOR to --no-color and only accepts true/false.
    // Normalize common truthy values (e.g. "1") so the child does not fail.
    match std::env::var("NO_COLOR") {
        Ok(value) if is_truthy(&value) => {
            command.env("NO_COLOR", "true");
        }
        Ok(_) => {
            command.env_remove("NO_COLOR");
        }
        Err(_) => {}
    }

    let profile = std::env::var("PROFILE").unwrap_or_default();
    if profile == "release" {
        command.arg("--release");
    }

    let status = command.status().expect("trunk command failed");

    if !status.success() {
        panic!("cargo:warning=trunk build failed");
    }
}

/// Whether an env value should be treated as enabling NO_COLOR (non-empty and
/// not an explicit false-like token).
fn is_truthy(value: &str) -> bool {
    let value = value.trim();
    if value.is_empty() {
        return false;
    }
    !matches!(
        value.to_ascii_lowercase().as_str(),
        "0" | "false" | "no" | "off"
    )
}

fn ensure_placeholder(dist_directory: &Path) {
    if dist_directory.join("index.html").exists() {
        return;
    }
    fs::create_dir_all(dist_directory)
        .expect("Failed to create inspect dist directory");
    fs::write(
        dist_directory.join("index.html"),
        "<!DOCTYPE html>\
         <html><body>\
         <h1>Bombadil Inspect</h1>\
         <p>Inspect UI not built. \
         Install trunk, then rebuild.</p>\
         </body></html>",
    )
    .expect("Failed to write placeholder index.html");
}
