//! The browser defaults' `consoleErrorEntries` and `consoleErrors` cells, run
//! from the bundle.

use std::io::Write as _;
use std::sync::OnceLock;

use boa_engine::{Context, JsValue, Source};
use hegel::TestCase;
use hegel::generators::{
    Generator, booleans, integers, just, one_of, sampled_from, text, vecs,
};
use serde_json::{self as json, json};
use tempfile::NamedTempFile;

use crate::specification::bundler::bundle;

fn console_errors(console: &json::Value) -> (json::Value, json::Value) {
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

    let mut context = Context::default();
    context.eval(Source::from_bytes(bundle_code)).unwrap();
    // In registration order, as `runExtractors` runs them.
    let run = context
        .eval(Source::from_bytes(
            r#"(console) => __bombadilRequire("@antithesishq/bombadil").runtime.extractors
                .filter((extractor) =>
                    extractor.name === "consoleErrorEntries" || extractor.name === "consoleErrors")
                .map((extractor) => [extractor.name, extractor.run({ console })])"#,
        ))
        .unwrap();
    let console = JsValue::from_json(console, &mut context).unwrap();
    let values = run
        .as_callable()
        .unwrap()
        .call(&JsValue::undefined(), &[console], &mut context)
        .unwrap()
        .to_json(&mut context)
        .unwrap()
        .unwrap();
    let [(detailed_name, detailed), (abstract_name, abstracted)]: [(
        String,
        json::Value,
    ); 2] = json::from_value(values).unwrap();
    assert_eq!(detailed_name, "consoleErrorEntries");
    assert_eq!(abstract_name, "consoleErrors");
    (detailed, abstracted)
}

#[hegel::composite]
fn stack_trace(tc: &TestCase) -> json::Value {
    if tc.draw_silent(booleans()) {
        return json::Value::Null;
    }
    let frame_count = tc.draw_silent(integers::<usize>().max_value(3));
    (0..frame_count)
        .map(|_| {
            json!({
                "name": tc.draw_silent(text()),
                "line": tc.draw_silent(integers::<i32>()),
                "column": tc.draw_silent(integers::<i32>()),
                "url": tc.draw_silent(text()),
            })
        })
        .collect()
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

/// Console entries as the driver passes them, and those of level `error`.
#[hegel::composite]
fn consoles(tc: &TestCase) -> (json::Value, Vec<json::Value>) {
    let entry_count = tc.draw_silent(integers::<usize>().max_value(5));
    let mut console = Vec::with_capacity(entry_count);
    let mut errors = Vec::new();
    for _ in 0..entry_count {
        let level =
            tc.draw_silent(sampled_from(["error", "warning"].as_slice()));
        // Microseconds since the epoch, as a float like boa gives them back.
        let timestamp = tc.draw_silent(
            integers::<u64>()
                .min_value(1_500_000_000_000_000)
                .max_value(2_500_000_000_000_000),
        ) as f64;
        let arg_count = tc.draw_silent(integers::<usize>().max_value(4));
        let args: Vec<json::Value> = (0..arg_count).map(|_| args(tc)).collect();
        let entry = json!({
            "timestamp": timestamp,
            "level": level,
            "args": args,
            "stackTrace": tc.draw_silent(stack_trace()),
        });
        if level == "error" {
            errors.push(entry.clone());
        }
        console.push(entry);
    }
    let console = json::Value::Array(console);
    tc.note(&format!("console: {console}"));
    (console, errors)
}

#[hegel::test]
fn console_error_entries_are_the_whole_error_entries(tc: TestCase) {
    let (console, errors) = tc.draw_silent(consoles());

    let (entries, _) = console_errors(&console);
    assert_eq!(entries, json::Value::Array(errors));
}

#[hegel::test]
fn console_errors_are_the_level_and_first_lines_of_the_error_entries(
    tc: TestCase,
) {
    let (console, errors) = tc.draw_silent(consoles());
    let expected: Vec<json::Value> = errors
        .iter()
        .map(|entry| {
            let message = entry["args"]
                .as_array()
                .unwrap()
                .iter()
                .map(|arg| match arg {
                    json::Value::String(text) => {
                        text.split('\n').next().unwrap().to_string()
                    }
                    other => other.to_string(),
                })
                .collect::<Vec<_>>()
                .join(" ");
            json!({ "level": "error", "message": message })
        })
        .collect();

    let (_, abstracted) = console_errors(&console);
    assert_eq!(abstracted, json::Value::Array(expected));
}
