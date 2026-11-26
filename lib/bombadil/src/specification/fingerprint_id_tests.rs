//! Which element ids `getFingerprint` keeps, run from the bundle.

use std::io::Write as _;
use std::sync::OnceLock;

use boa_engine::{Context, JsValue, Source, js_string};
use hegel::TestCase;
use hegel::generators::{booleans, integers, sampled_from, text};
use tempfile::NamedTempFile;

use crate::specification::bundler::bundle;

/// The fingerprint's id of an element that has `id` and some text.
fn fingerprint_id(id: &str) -> Option<String> {
    static BUNDLE: OnceLock<String> = OnceLock::new();
    let bundle_code = BUNDLE.get_or_init(|| {
        let mut specification_file = NamedTempFile::with_suffix(".ts").unwrap();
        specification_file
            .write_all(
                br#"export { getFingerprint } from "@antithesishq/bombadil/browser";"#,
            )
            .unwrap();
        bundle(".", &specification_file.path().display().to_string()).unwrap()
    });

    let mut context = Context::default();
    context.eval(Source::from_bytes(bundle_code)).unwrap();
    let run = context
        .eval(Source::from_bytes(
            r#"(id) => __bombadilRequire("@antithesishq/bombadil/browser").getFingerprint({
                tagName: "DIV",
                textContent: "text",
                getAttribute: (name) => (name === "id" ? id : null),
            }).id"#,
        ))
        .unwrap();
    let result = run
        .as_callable()
        .unwrap()
        .call(
            &JsValue::undefined(),
            &[JsValue::from(js_string!(id))],
            &mut context,
        )
        .unwrap();
    result
        .as_string()
        .map(|string| string.to_std_string().unwrap())
}

const DIGITS: &str = "0123456789";

fn counter(tc: &TestCase) -> String {
    tc.draw_silent(text().alphabet(DIGITS).min_size(1).max_size(6))
}

/// What a library appends to the id it was given: `-label`, `-option-0`.
fn suffix(tc: &TestCase) -> String {
    if tc.draw_silent(booleans()) {
        return String::new();
    }
    let rest = tc.draw_silent(text().alphabet("abcxyz019-").max_size(12));
    format!("-{rest}")
}

/// An id as React's client `useId`, or the counters of MUI, radix and
/// headlessui, make it, with what libraries put around it.
fn id_generated(tc: &TestCase) -> String {
    let suffix = suffix(tc);
    match tc.draw_silent(integers::<u8>().max_value(3)) {
        0 => {
            let prefix = tc.draw_silent(sampled_from(
                ["", "radix-", "react-aria-", "headlessui-menu-button-"]
                    .as_slice(),
            ));
            let count = tc.draw_silent(
                text()
                    .alphabet("0123456789abcdefghijklmnopqrstuv")
                    .min_size(1)
                    .max_size(6),
            );
            let token = match tc.draw_silent(integers::<u8>().max_value(2)) {
                0 => format!(":r{count}:"),
                1 => format!("«r{count}»"),
                _ => format!("_r_{count}_"),
            };
            format!("{prefix}{token}{suffix}")
        }
        1 => format!("mui-{}{suffix}", counter(tc)),
        2 => format!("radix-{}{suffix}", counter(tc)),
        _ => {
            let component = tc.draw_silent(sampled_from(
                ["menu-button", "listbox-option", "dialog-panel", "tab"]
                    .as_slice(),
            ));
            format!("headlessui-{component}-{}", counter(tc))
        }
    }
}

const WORDS: [&str; 14] = [
    "shipping",
    "address",
    "line",
    "step",
    "continue",
    "button",
    "main",
    "content",
    "btn",
    "ctl",
    "settings",
    "mui",
    "radix",
    "headlessui",
];

/// An id as a person writes it: words, some numbered, in one naming style.
fn id_written(tc: &TestCase) -> String {
    let style = tc.draw_silent(integers::<u8>().max_value(4));
    let word_count =
        tc.draw_silent(integers::<usize>().min_value(1).max_value(6));
    let mut id = String::new();
    for index in 0..word_count {
        let word = tc.draw_silent(sampled_from(WORDS.as_slice()));
        let capitalized = match style {
            0 => index > 0,
            1 => true,
            _ => false,
        };
        if index > 0 {
            match style {
                2 | 3 => id.push('_'),
                4 => id.push('-'),
                _ => {}
            }
        }
        if style == 3 {
            id.push_str(&word.to_uppercase());
        } else if capitalized {
            id.push_str(&word[..1].to_uppercase());
            id.push_str(&word[1..]);
        } else {
            id.push_str(word);
        }
        id.push_str(&tc.draw_silent(text().alphabet(DIGITS).max_size(3)));
    }
    id
}

#[hegel::test]
fn generated_id_is_left_out_of_the_fingerprint(tc: TestCase) {
    let id = id_generated(&tc);
    tc.note(&format!("id: {id}"));
    assert_eq!(fingerprint_id(&id), None);
}

#[hegel::test]
fn written_id_is_kept_in_the_fingerprint(tc: TestCase) {
    let id = id_written(&tc);
    tc.note(&format!("id: {id}"));
    assert_eq!(fingerprint_id(&id), Some(id));
}

/// A random nanoid is recognised by its letters, which about one in thirty
/// lacks, so this pins samples rather than generating them.
#[test]
fn nanoid_is_left_out_of_the_fingerprint() {
    for id in [
        "V1StGXR8_Z5jdHi6B-myT",
        "Uakgb_J5m9g-0JDMbcJqL",
        "FwGcLB7e9_pQ2nX-hY4Kd",
    ] {
        assert_eq!(fingerprint_id(id), None, "{id}");
    }
}
