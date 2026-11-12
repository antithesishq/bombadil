//! The browser defaults' `widthThresholds`, which `mediaBand` reads media
//! conditions with, run from the bundle.

use std::collections::BTreeSet;
use std::io::Write as _;
use std::sync::OnceLock;

use boa_engine::{Context, JsValue, Source, js_string};
use hegel::TestCase;
use hegel::generators::{booleans, integers, sampled_from};
use tempfile::NamedTempFile;

use crate::specification::bundler::bundle;

fn width_thresholds(condition: &str) -> BTreeSet<String> {
    static BUNDLE: OnceLock<String> = OnceLock::new();
    let bundle_code = BUNDLE.get_or_init(|| {
        let mut specification_file = NamedTempFile::with_suffix(".ts").unwrap();
        specification_file
            .write_all(
                br#"export { mediaBand } from "@antithesishq/bombadil/browser/defaults/extractors";"#,
            )
            .unwrap();
        bundle(".", &specification_file.path().display().to_string()).unwrap()
    });

    let mut context = Context::default();
    context.eval(Source::from_bytes(bundle_code)).unwrap();
    let run = context
        .eval(Source::from_bytes(
            r#"(condition) => {
                const thresholds = new Set();
                __bombadilRequire("@antithesishq/bombadil/browser/defaults/extractors")
                    .widthThresholds(condition, thresholds);
                return [...thresholds];
            }"#,
        ))
        .unwrap();
    let thresholds = run
        .as_callable()
        .unwrap()
        .call(
            &JsValue::undefined(),
            &[JsValue::from(js_string!(condition))],
            &mut context,
        )
        .unwrap()
        .to_json(&mut context)
        .unwrap()
        .unwrap();
    serde_json::from_value(thresholds).unwrap()
}

fn length(tc: &TestCase) -> String {
    let whole = tc.draw_silent(integers::<u16>().max_value(4000));
    let unit =
        tc.draw_silent(sampled_from(["px", "em", "rem", "vw"].as_slice()));
    match tc.draw_silent(integers::<u8>().max_value(2)) {
        0 => format!("{whole}{unit}"),
        1 => format!(
            "{whole}.{}{unit}",
            tc.draw_silent(integers::<u8>().max_value(99))
        ),
        _ => {
            format!(".{}{unit}", tc.draw_silent(integers::<u8>().max_value(99)))
        }
    }
}

fn space(tc: &TestCase) -> &'static str {
    tc.draw_silent(sampled_from(["", " ", "  "].as_slice()))
}

/// The query `widthThresholds` should give for a bound on the width: the
/// viewport is past a lower bound exactly where it holds, and past an upper
/// bound exactly where it does not.
fn threshold(length: &str, lower_bound: bool, holds_at_length: bool) -> String {
    if lower_bound == holds_at_length {
        format!("(min-width: {length})")
    } else {
        format!("(width > {length})")
    }
}

/// Writes one media feature and adds the thresholds it should give.
fn feature(tc: &TestCase, expected: &mut BTreeSet<String>) -> String {
    let (a, b) = (space(tc), space(tc));
    let length_first = length(tc);
    match tc.draw_silent(integers::<u8>().max_value(5)) {
        0 => {
            let lower_bound = tc.draw_silent(booleans());
            expected.insert(threshold(&length_first, lower_bound, true));
            let prefix = if lower_bound { "min" } else { "max" };
            format!("({prefix}-width{a}:{b}{length_first})")
        }
        1 => {
            let (operator, lower_bound, holds) = comparison(tc);
            expected.insert(threshold(&length_first, lower_bound, holds));
            format!("(width{a}{operator}{b}{length_first})")
        }
        2 => {
            // `600px < width` bounds the width from below, as `width > 600px`.
            let (operator, upper_bound, holds) = comparison(tc);
            expected.insert(threshold(&length_first, !upper_bound, holds));
            format!("({length_first}{a}{operator}{b}width)")
        }
        3 => {
            let length_second = length(tc);
            let holds_first = tc.draw_silent(booleans());
            let holds_second = tc.draw_silent(booleans());
            expected.insert(threshold(&length_first, true, holds_first));
            expected.insert(threshold(&length_second, false, holds_second));
            format!(
                "({length_first}{a}{}{b}width{a}{}{b}{length_second})",
                if holds_first { "<=" } else { "<" },
                if holds_second { "<=" } else { "<" },
            )
        }
        4 => {
            let name = tc.draw_silent(sampled_from(
                [
                    "min-height",
                    "max-height",
                    "min-device-width",
                    "max-device-width",
                    "min-inline-size",
                ]
                .as_slice(),
            ));
            format!("({name}{a}:{b}{length_first})")
        }
        _ => tc
            .draw_silent(sampled_from(
                [
                    "screen",
                    "print",
                    "(orientation: landscape)",
                    "(height >= 600px)",
                    "(400px <= height <= 700px)",
                    "(prefers-reduced-motion)",
                ]
                .as_slice(),
            ))
            .to_string(),
    }
}

/// An operator, whether `width <operator> length` bounds the width from
/// below, and whether it holds at the length.
fn comparison(tc: &TestCase) -> (&'static str, bool, bool) {
    tc.draw_silent(sampled_from(
        [
            (">=", true, true),
            (">", true, false),
            ("<=", false, true),
            ("<", false, false),
        ]
        .as_slice(),
    ))
}

#[hegel::test]
fn width_thresholds_are_the_bounds_on_the_viewport_width(tc: TestCase) {
    let mut expected = BTreeSet::new();
    let mut condition = String::new();
    let feature_count = tc.draw_silent(integers::<usize>().max_value(6));
    for index in 0..feature_count {
        if index > 0 {
            condition.push_str(tc.draw_silent(sampled_from(
                [" and ", ", ", " or ", " and not "].as_slice(),
            )));
        }
        condition.push_str(&feature(&tc, &mut expected));
    }
    tc.note(&format!("condition: {condition}"));
    assert_eq!(width_thresholds(&condition), expected);
}
