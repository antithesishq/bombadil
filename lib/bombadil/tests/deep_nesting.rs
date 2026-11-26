use std::io::Write;
use std::time::UNIX_EPOCH;

use bombadil::specification::bundler::bundle;
use bombadil::specification::verifier::Verifier;
use bombadil_schema::Time;
use serde_json as json;
use tempfile::NamedTempFile;

const THREAD_STACK: usize = 2 * 1024 * 1024;

fn run_on_thread<F>(f: F)
where
    F: FnOnce() + Send + 'static,
{
    std::thread::Builder::new()
        .stack_size(THREAD_STACK)
        .spawn(f)
        .unwrap()
        .join()
        .unwrap();
}

fn bundle_specification(source: &str) -> Result<String, String> {
    let mut file = NamedTempFile::with_suffix(".ts").unwrap();
    file.write_all(source.as_bytes()).unwrap();
    bundle(".", &file.path().display().to_string()).map_err(|e| e.to_string())
}

fn time() -> Time {
    Time::from_system_time(UNIX_EPOCH)
}

#[test]
fn deeply_nested_thunk_still_evaluates() {
    run_on_thread(|| {
        let body = "(() => ".repeat(128)
            + "true"
            + &")".repeat(128)
            + &"()".repeat(128);
        let source = format!(
            r#"
            import {{ always, actions }} from "@antithesishq/bombadil";
            export const _actions = actions(() => []);
            export const deep = always(() => {body});
            "#
        );
        let code = bundle_specification(&source).unwrap();
        let mut verifier = Verifier::new(&code).unwrap();
        let result = verifier.step::<json::Value>(&[], time());
        assert!(result.is_ok(), "{}", result.err().unwrap());
    });
}

#[test]
fn unbounded_recursion_in_thunk_is_a_js_error() {
    run_on_thread(|| {
        let source = r#"
            import { always, actions } from "@antithesishq/bombadil";
            export const _actions = actions(() => []);
            function f(n: number): number {
                return n === 0 ? 0 : f(n - 1) + 1;
            }
            export const overflow = always(() => f(1000000) > 0);
        "#;
        let code = bundle_specification(source).unwrap();
        let mut verifier = Verifier::new(&code).unwrap();
        match verifier.step::<json::Value>(&[], time()) {
            Err(error) => assert!(
                error
                    .to_string()
                    .contains("Maximum call stack size exceeded"),
                "unexpected error: {error}"
            ),
            Ok(_) => panic!("expected recursion to overflow"),
        }
    });
}

#[test]
fn hundred_thousand_deep_array_literal_is_an_error() {
    run_on_thread(|| {
        let literal = "[".repeat(100_000) + &"]".repeat(100_000);
        let source = format!(
            r#"
            import {{ actions }} from "@antithesishq/bombadil";
            export const _actions = actions(() => []);
            const nested = {literal};
            "#
        );
        match bundle_specification(&source) {
            Err(error) => eprintln!("bundle rejected deep literal: {error}"),
            Ok(code) => assert!(
                Verifier::new(&code).is_err(),
                "expected deep literal to be rejected"
            ),
        }
    });
}
