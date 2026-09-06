use cdp_pdl::build::Generator;
use std::{env, path::PathBuf};

fn main() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let js_proto = env::var("CDP_JS_PROTOCOL_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|_| dir.join("pdl/js_protocol.pdl"));
    let browser_proto = env::var("CDP_BROWSER_PROTOCOL_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|_| dir.join("pdl/browser_protocol.pdl"));

    Generator::default()
        .out_dir(dir.join("src"))
        .target_mod("cdp")
        .experimental(env::var("CDP_NO_EXPERIMENTAL").is_err())
        .deprecated(env::var("CDP_DEPRECATED").is_ok())
        .allowed_deprecated_type("emulateNetworkConditions")
        .only_domains([
            "Browser",
            "CSS",
            "DOM",
            "Emulation",
            "Fetch",
            "Input",
            "Network",
            "Page",
            "Performance",
            "Target",
        ])
        .compile_pdls(&[js_proto, browser_proto])
        .unwrap();

    println!("Regenerated src/cdp.rs");
}
