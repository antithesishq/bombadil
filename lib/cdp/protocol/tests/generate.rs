use cdp_pdl::build::Generator;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

/// Check that the generated files are up to date
#[ignore]
#[test]
fn generated_code_is_fresh() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let js_proto = env::var("CDP_JS_PROTOCOL_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|_| dir.join("pdl/js_protocol.pdl"));

    let browser_proto = env::var("CDP_BROWSER_PROTOCOL_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|_| dir.join("pdl/browser_protocol.pdl"));

    let tmp = tempfile::tempdir().unwrap();
    Generator::default()
        .out_dir(tmp.path())
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

    let new = fs::read_to_string(tmp.path().join("cdp.rs")).unwrap();
    let src = dir.join("src/cdp.rs");
    let old = fs::read_to_string(&src).unwrap();

    for (i, (l1, l2)) in new.lines().zip(old.lines()).enumerate() {
        if l1 != l2 {
            eprintln!("line {} differs:\n\n{}\n\n!=\n\n{}", i, l1, l2);
        }
    }

    if new != old {
        panic!("generated code in the repository is outdated, updating...");
    }
}
