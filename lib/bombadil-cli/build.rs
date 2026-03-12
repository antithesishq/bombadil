use std::fs;
use std::path::Path;
use std::process::Stdio;

fn main() {
    let dist_directory = Path::new("../../target/debug-ui");

    println!("cargo:rerun-if-changed=../bombadil-debug-ui/src");
    println!("cargo:rerun-if-changed=../bombadil-debug-ui/Cargo.toml");
    println!("cargo:rerun-if-changed=../bombadil-debug-ui/index.html");
    println!("cargo:rerun-if-changed=../bombadil-debug-ui/Trunk.toml");

    let profile = std::env::var("PROFILE").unwrap_or_else(|_| "debug".into());
    if profile == "release" {
        ensure_placeholder(dist_directory);
        return;
    }

    build_debug_ui(dist_directory);
}

fn build_debug_ui(dist_directory: &Path) {
    let debug_ui_directory = Path::new("../bombadil-debug-ui");

    if !debug_ui_directory.join("Cargo.toml").exists() {
        ensure_placeholder(dist_directory);
        return;
    }

    let dist_absolute = fs::canonicalize("../../")
        .expect("Failed to resolve workspace root")
        .join("target/debug-ui");

    let wasm_target_directory = fs::canonicalize("../../")
        .expect("Failed to resolve workspace root")
        .join("target/debug-ui-wasm");

    let mut command = std::process::Command::new("trunk");
    command
        .arg("build")
        .arg("--offline")
        .arg("--dist")
        .arg(&dist_absolute)
        .env("CARGO_TARGET_DIR", &wasm_target_directory)
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .current_dir(debug_ui_directory);

    let profile = std::env::var("PROFILE").unwrap_or_default();
    if profile == "release" {
        command.arg("--release");
    }

    let status = command.status().expect("trunk command failed");

    if !status.success() {
        panic!("cargo:warning=trunk build failed");
    }
}

fn ensure_placeholder(dist_directory: &Path) {
    if dist_directory.join("index.html").exists() {
        return;
    }
    fs::create_dir_all(dist_directory)
        .expect("Failed to create debug-ui dist directory");
    fs::write(
        dist_directory.join("index.html"),
        "<!DOCTYPE html>\
         <html><body>\
         <h1>Bombadil Debug UI</h1>\
         <p>Debug UI not built. \
         Install trunk, then rebuild.</p>\
         </body></html>",
    )
    .expect("Failed to write placeholder index.html");
}
