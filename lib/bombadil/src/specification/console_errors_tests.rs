//! The browser defaults' `consoleErrors` extract, run from the bundle.

use std::io::Write as _;
use std::sync::OnceLock;

use hegel::TestCase;
use hegel::generators::{
    Generator, booleans, integers, just, one_of, sampled_from, text, vecs,
};
use serde_json::{self as json, json};
use tempfile::NamedTempFile;

use rquickjs::{Function, Value};

use crate::specification::bundler::bundle;
use crate::specification::engine::Engine;
use crate::specification::result::JsResultExt;

fn console_errors(console: &json::Value) -> json::Value {
    static BUNDLE: OnceLock<String> = OnceLock::new();
    let bundle_code = BUNDLE.get_or_init(|| {
        let mut specification_file = NamedTempFile::with_suffix(".ts").unwrap();
        specification_file
            .write_all(
                br#"export { noConsoleErrors } from "@antithesishq/bombadil/browser/defaults/properties";"#,
            )
            .unwrap();
        bundle(".", &specification_file.path().display().to_string()).unwrap()
    });

    let engine = Engine::new().unwrap();
    engine.context.with(|ctx| {
        ctx.eval::<Value, _>(bundle_code.as_str()).js(&ctx).unwrap();
        let run: Function = ctx
            .eval(
                r#"(console) => __bombadilRequire("@antithesishq/bombadil").runtime.extractors
                .find((extractor) => extractor.name === "consoleErrors")
                .run({ console })"#,
            )
            .js(&ctx)
            .unwrap();
        let console = ctx
            .json_parse(json::to_string(console).unwrap())
            .js(&ctx)
            .unwrap();
        let result: Value = run.call((console,)).js(&ctx).unwrap();
        let text = ctx
            .json_stringify(result)
            .js(&ctx)
            .unwrap()
            .unwrap()
            .to_string()
            .unwrap();
        json::from_str(&text).unwrap()
    })
}

fn args(tc: &TestCase) -> json::Value {
    if tc.draw_silent(booleans()) {
        let mut string = tc.draw_silent(text());
        if tc.draw_silent(booleans()) {
            string.push('\n');
            string.push_str(&tc.draw_silent(text()));
        }
        return json::Value::String(string);
    }
    tc.draw_silent(one_of([
        integers::<i32>().map(json::Value::from).boxed(),
        booleans().map(json::Value::Bool).boxed(),
        just(json::Value::Null).boxed(),
        vecs(text()).max_size(3).map(json::Value::from).boxed(),
        text().map(|value| json!({ "detail": value })).boxed(),
    ]))
}

#[hegel::test]
fn console_errors_are_the_level_and_first_lines_of_error_entries(tc: TestCase) {
    let entry_count = tc.draw_silent(integers::<usize>().max_value(5));
    let mut console = Vec::with_capacity(entry_count);
    let mut expected = Vec::new();
    for _ in 0..entry_count {
        let level =
            tc.draw_silent(sampled_from(["error", "warning"].as_slice()));
        let timestamp = tc.draw_silent(integers::<u32>());
        let arg_count = tc.draw_silent(integers::<usize>().max_value(4));
        let args: Vec<json::Value> =
            (0..arg_count).map(|_| args(&tc)).collect();
        if level == "error" {
            let message = args
                .iter()
                .map(|arg| match arg {
                    json::Value::String(text) => {
                        text.split('\n').next().unwrap().to_string()
                    }
                    other => other.to_string(),
                })
                .collect::<Vec<_>>()
                .join(" ");
            expected.push(json!({ "level": "error", "message": message }));
        }
        console.push(
            json!({ "timestamp": timestamp, "level": level, "args": args }),
        );
    }
    let console = json::Value::Array(console);
    tc.note(&format!("console: {console}"));

    assert_eq!(console_errors(&console), json::Value::Array(expected));
}
