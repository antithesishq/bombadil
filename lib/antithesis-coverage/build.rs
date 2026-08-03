fn main() {
    cc::Build::new()
        .file("src/antithesis_coverage_shim.c")
        .opt_level(3)
        .compile("antithesis_coverage_shim");
    println!("cargo::rerun-if-changed=src/antithesis_coverage_shim.c");
}
