fn main() {
    cc::Build::new()
        .file("src/libvoidstar_shim.c")
        .opt_level(3)
        .warnings(true)
        .warnings_into_errors(true)
        // `-Wextra` is a GCC/Clang flag; MSVC's cl.exe rejects it. Skip it there.
        .flag_if_supported("-Wextra")
        .compile("libvoidstar_shim");
    println!("cargo::rerun-if-changed=src/libvoidstar_shim.c");
}
