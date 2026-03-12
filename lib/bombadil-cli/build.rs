use std::fs;
use std::path::Path;

fn main() {
    let dist_directory = Path::new("../../target/debug-ui");

    println!("cargo:rerun-if-changed=../bombadil-debug-ui/src");
    println!("cargo:rerun-if-changed=../bombadil-debug-ui/Cargo.toml");
    println!("cargo:rerun-if-changed=../bombadil-debug-ui/index.html");
    println!("cargo:rerun-if-changed=../bombadil-debug-ui/Trunk.toml");

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

    let status = std::process::Command::new("trunk")
        .arg("build")
        .arg("--offline")
        .arg("--dist")
        .arg(&dist_absolute)
        .env("CARGO_TARGET_DIR", &wasm_target_directory)
        .current_dir(debug_ui_directory)
        .status();

    match status {
        Ok(status) if status.success() => {}
        Ok(_) => {
            println!(
                "cargo:warning=trunk build failed, \
                 using placeholder"
            );
            ensure_placeholder(dist_directory);
        }
        Err(error) => {
            println!(
                "cargo:warning=trunk not found ({error}), \
                 using placeholder"
            );
            ensure_placeholder(dist_directory);
        }
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
