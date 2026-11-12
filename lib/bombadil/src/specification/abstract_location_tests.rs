//! The browser module's `abstractLocation` extract, run from the bundle of a
//! specification that does not import the defaults.

use std::fmt::Write as _;
use std::io::Write as _;
use std::sync::OnceLock;

use boa_engine::{Context, JsValue, Source};
use hegel::TestCase;
use hegel::generators::{
    Generator, booleans, from_regex, integers, one_of, optional, sampled_from,
    vecs,
};
use serde_json::{self as json, json};
use tempfile::NamedTempFile;

use crate::specification::bundler::bundle;

const TRACKING_PARAMETERS: [&str; 13] = [
    "_gl",
    "gclid",
    "dclid",
    "gbraid",
    "wbraid",
    "fbclid",
    "msclkid",
    "twclid",
    "yclid",
    "igshid",
    "mc_cid",
    "mc_eid",
    "hsCtaTracking",
];

fn abstract_location(pathname: &str, search: &str, hash: &str) -> json::Value {
    static BUNDLE: OnceLock<String> = OnceLock::new();
    let bundle_code = BUNDLE.get_or_init(|| {
        let mut specification_file = NamedTempFile::with_suffix(".ts").unwrap();
        specification_file
            .write_all(
                br#"import { always } from "@antithesishq/bombadil";
import { extract } from "@antithesishq/bombadil/browser";
const title = extract((state) => state.document.title);
export const titled = always(() => title.current !== "");"#,
            )
            .unwrap();
        bundle(".", &specification_file.path().display().to_string()).unwrap()
    });

    let mut context = Context::default();
    context.eval(Source::from_bytes(bundle_code)).unwrap();
    let run = context
        .eval(Source::from_bytes(
            r#"(location) => __bombadilRequire("@antithesishq/bombadil").runtime.extractors
                .find((extractor) => extractor.name === "abstractLocation")
                .run({ window: { location } })"#,
        ))
        .unwrap();
    let location = JsValue::from_json(
        &json!({ "pathname": pathname, "search": search, "hash": hash }),
        &mut context,
    )
    .unwrap();
    run.as_callable()
        .unwrap()
        .call(&JsValue::undefined(), &[location], &mut context)
        .unwrap()
        .to_json(&mut context)
        .unwrap()
        .unwrap()
}

/// A path segment, fragment segment or query value: a word that is kept, or
/// an id that is masked. The string is the decoded text.
#[derive(Debug, Clone)]
enum Part {
    Word(String),
    Id(String),
}

impl Part {
    fn text(&self) -> &str {
        match self {
            Part::Word(text) | Part::Id(text) => text,
        }
    }

    fn abstracted(&self) -> &str {
        match self {
            Part::Word(text) => text,
            Part::Id(_) => "#id",
        }
    }
}

fn ids() -> impl Generator<String> {
    one_of([
        from_regex("[0-9]{1,20}").boxed(),
        from_regex(
            "[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}",
        )
        .boxed(),
    ])
}

/// Draws an id or a word from `words`, which must not generate ids.
fn parts(tc: &TestCase, words: &str) -> Part {
    if tc.draw_silent(booleans()) {
        Part::Id(tc.draw_silent(ids()))
    } else {
        Part::Word(tc.draw_silent(from_regex(words)))
    }
}

/// URL path characters, starting with one that is neither a digit nor a hex
/// digit, so the segment is not an id. Includes near-ids such as `x42`.
const SEGMENT_WORDS: &str = "[g-zG-Z._~%-][a-zA-Z0-9._~%-]{0,8}";

/// Any printable ASCII, or nothing, with the same first character rule.
const VALUE_WORDS: &str = "([g-zG-Z _&=+%#/?.-][ -~]{0,6})?";

/// Any printable ASCII starting with a character no tracking parameter
/// starts with. Keys are never masked, so they may be all digits.
const KEY_WORDS: &str = "[a-cejklnopqrsvxzA-Z0-9 &=+%#][ -~]{0,6}";

/// Encodes a decoded key or value for a query string, choosing between the
/// spellings a URL allows: `+` or `%20` for a space, and unreserved
/// characters plain or percent-encoded.
fn encode(tc: &TestCase, decoded: &str) -> String {
    let space_as_plus = tc.draw_silent(booleans());
    let encode_all = tc.draw_silent(booleans());
    let mut encoded = String::new();
    for byte in decoded.bytes() {
        let unreserved =
            byte.is_ascii_alphanumeric() || b"-._~".contains(&byte);
        if byte == b' ' && space_as_plus {
            encoded.push('+');
        } else if unreserved && !encode_all {
            encoded.push(byte as char);
        } else {
            write!(encoded, "%{byte:02X}").unwrap();
        }
    }
    encoded
}

fn tracking_pairs(tc: &TestCase) -> String {
    let key = if tc.draw_silent(booleans()) {
        tc.draw_silent(sampled_from(TRACKING_PARAMETERS.as_slice()))
            .to_string()
    } else {
        tc.draw_silent(from_regex("(utm_|_ga|_hs)[a-zA-Z0-9_]{0,8}"))
    };
    let value = tc.draw_silent(from_regex("[a-zA-Z0-9%+=._-]{0,8}"));
    format!("{}={value}", encode(tc, &key))
}

/// Joins the pairs into a search string, with tracking parameters and empty
/// pairs drawn in between.
fn search(tc: &TestCase, pairs: &[String]) -> String {
    let mut search = String::new();
    let mut push = |pair: &str| {
        search.push(if search.is_empty() { '?' } else { '&' });
        search.push_str(pair);
    };
    for index in 0..=pairs.len() {
        while tc.draw_silent(booleans()) {
            if tc.draw_silent(booleans()) {
                push(&tracking_pairs(tc));
            } else {
                push("");
            }
        }
        if let Some(pair) = pairs.get(index) {
            push(pair);
        }
    }
    search
}

#[hegel::test]
fn abstract_location_masks_ids_and_drops_tracking_parameters(tc: TestCase) {
    let segment_count = tc.draw_silent(integers::<usize>().max_value(4));
    let segments: Vec<Part> = (0..segment_count)
        .map(|_| parts(&tc, SEGMENT_WORDS))
        .collect();
    let fragment_count = tc.draw_silent(integers::<usize>().max_value(3));
    let fragment: Vec<Part> = (0..fragment_count)
        .map(|_| parts(&tc, SEGMENT_WORDS))
        .collect();
    let parameter_count = tc.draw_silent(integers::<usize>().max_value(4));
    let parameters: Vec<(String, Option<Part>)> = (0..parameter_count)
        .map(|_| {
            let key = tc.draw_silent(from_regex(KEY_WORDS));
            let value =
                tc.draw_silent(booleans()).then(|| parts(&tc, VALUE_WORDS));
            (key, value)
        })
        .collect();

    let mut pathname = String::new();
    for segment in &segments {
        pathname.push('/');
        if tc.draw_silent(booleans()) {
            pathname.push('/');
        }
        pathname.push_str(segment.text());
    }
    if segments.is_empty() || tc.draw_silent(booleans()) {
        pathname.push('/');
    }
    let mut hash = String::new();
    for (index, segment) in fragment.iter().enumerate() {
        hash.push(if index == 0 { '#' } else { '/' });
        if index == 0 {
            hash.push('/');
        }
        hash.push_str(segment.text());
    }
    let pairs: Vec<String> = parameters
        .iter()
        .map(|(key, value)| match value {
            Some(value) => {
                format!("{}={}", encode(&tc, key), encode(&tc, value.text()))
            }
            None => encode(&tc, key),
        })
        .collect();
    let search = search(&tc, &pairs);
    tc.note(&format!("location: {pathname}{search}{hash}"));

    let mut query: Vec<(&str, &str)> = parameters
        .iter()
        .map(|(key, value)| {
            (key.as_str(), value.as_ref().map_or("", Part::abstracted))
        })
        .collect();
    query.sort();
    let abstracted = |parts: &[Part]| {
        parts
            .iter()
            .map(Part::abstracted)
            .collect::<Vec<_>>()
            .join("/")
    };
    let expected = json!({
        "pathname": format!("/{}", abstracted(&segments)),
        "hash": if fragment.is_empty() {
            String::new()
        } else {
            format!("#/{}", abstracted(&fragment))
        },
        "query": query,
    });

    assert_eq!(abstract_location(&pathname, &search, &hash), expected);
}

/// Any query, including malformed escapes, repeated `=` and empty keys.
#[hegel::test]
fn abstract_location_ignores_tracking_parameters_in_any_query(tc: TestCase) {
    let pairs =
        tc.draw(vecs(from_regex("[a-zA-Z0-9%+=_. -]{0,8}")).max_size(5));
    let pathname = tc.draw(optional(from_regex("/[ -~]{0,10}")));
    let pathname = pathname.as_deref().unwrap_or("/");

    let plain = if pairs.is_empty() {
        String::new()
    } else {
        format!("?{}", pairs.join("&"))
    };
    let tracked = search(&tc, &pairs);
    tc.note(&format!("tracked: {tracked}"));

    assert_eq!(
        abstract_location(pathname, &tracked, ""),
        abstract_location(pathname, &plain, ""),
    );
}
