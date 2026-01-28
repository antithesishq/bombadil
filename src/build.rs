use glob::glob;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=src/browser/*.ts");
    println!("cargo:rerun-if-changed=src/browser/actions/*.ts");

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
