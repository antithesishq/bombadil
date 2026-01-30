use glob::glob;
use std::process::Command;

fn main() {
    build_browser_action_scripts();
    build_specification_module();
    build_specification_module_types();
}

fn build_browser_action_scripts() {
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

fn build_specification_module() {
    println!("cargo:rerun-if-changed=src/specification/**/*.ts");

    let status = Command::new("esbuild")
        .args(["src/specification/bombadil/index.ts"])
        .arg("--bundle")
        .arg("--format=esm")
        .arg("--outdir=target/specification/")
        .status()
        .expect("Failed to execute esbuild");

    if !status.success() {
        panic!("esbuild failed with status: {}", status);
    }
}

fn build_specification_module_types() {
    println!("cargo:rerun-if-changed=src/specification/bombadil/**/*.ts");

    let status = Command::new("tsc")
        .args(["--lib", "es2021,dom"])
        .args(["--target", "es6"])
        .arg("--declaration")
        .arg("--emitDeclarationOnly")
        .args(["--outDir", "./target/specification-types"])
        .arg("--declaration")
        .arg("src/specification/bombadil/index.ts")
        .status()
        .expect("Failed to execute esbuild");

    if !status.success() {
        panic!("tsc failed with status: {}", status);
    }
}
