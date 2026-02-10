use glob::glob;
use std::process::Command;

fn main() {
    build_browser_action_scripts();

    println!("cargo:rerun-if-changed=src/specification/**/*.ts");
    build_specification_modules();
}

fn build_browser_action_scripts() {
    for typescript_file in glob("src/browser/**/*.ts")
        .expect("failed to read glob pattern")
        .filter_map(Result::ok)
        .collect::<Vec<_>>()
    {
        println!("cargo:rerun-if-changed={}", typescript_file.display());
    }

    let entry_points: Vec<_> = glob("src/browser/actions/*.ts")
        .expect("Failed to read glob pattern")
        .filter_map(Result::ok)
        .collect();

    if entry_points.is_empty() {
        return;
    }

    let status = Command::new("esbuild")
        .args(&entry_points)
        .arg("--bundle")
        .arg("--format=iife")
        .arg("--minify")
        .arg("--banner:js=(function() { var result; ")
        .arg("--footer:js=return result; })")
        .arg("--outdir=target/actions/")
        .status()
        .expect("Failed to execute esbuild");

    if !status.success() {
        panic!("esbuild failed with status: {}", status);
    }
}

fn build_specification_modules() {
    let entry_points: Vec<_> = glob("src/specification/**/*.ts")
        .expect("Failed to read glob pattern")
        .filter_map(Result::ok)
        .collect();

    let status = Command::new("esbuild")
        .args(&entry_points)
        .arg("--format=esm")
        .arg("--outdir=target/specification")
        .status()
        .expect("Failed to execute esbuild");

    if !status.success() {
        panic!("esbuild failed with status: {}", status);
    }
}
