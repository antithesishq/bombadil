use glob::glob;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=src/specification/**/*.ts");
    build_specification_modules();
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
