use std::sync::LazyLock;

use memchr::{memchr, memmem::Finder};

static ESCAPE: LazyLock<Finder<'static>> =
    LazyLock::new(|| Finder::new(br"\u"));

const TRAILING_FIRST: u16 = 0xDC00;

/// JSON text with the escape of each lone UTF-16 surrogate replaced by
/// `\ufffd`, which is as long, or `None` if there is none.
pub fn replace_lone(json: &str) -> Option<String> {
    let bytes = json.as_bytes();
    let mut at = memchr(b'\\', bytes)?;
    let mut replaced: Option<String> = None;
    let mut copied = 0;
    while let Some(offset) = bytes.get(at..).and_then(|rest| ESCAPE.find(rest))
    {
        at += offset;
        let found = at;
        // After an odd number of backslashes this one is itself escaped.
        // Later escapes in a run follow a hex digit, so only the first can be.
        let backslashes = bytes[..at]
            .iter()
            .rev()
            .take_while(|&&byte| byte == b'\\')
            .count();
        while backslashes % 2 == 0
            && let Some(&[b'\\', b'u', first, a, b, c]) =
                bytes.get(at..at + 6).and_then(<[u8]>::as_array)
        {
            if first != b'd' && first != b'D' {
                // Not a surrogate, and nothing can start inside it.
                if first == b'\\' || a == b'\\' || b == b'\\' || c == b'\\' {
                    break;
                }
                at += 6;
                continue;
            }
            let Some(unit) = surrogate(a, b, c) else {
                break;
            };
            let escape = at;
            at += 6;
            if unit < TRAILING_FIRST
                && let Some(&[b'\\', b'u', b'd' | b'D', a, b, c]) =
                    bytes.get(at..at + 6).and_then(<[u8]>::as_array)
                && surrogate(a, b, c).is_some_and(|next| next >= TRAILING_FIRST)
            {
                at += 6;
                continue;
            }
            let replaced = replaced
                .get_or_insert_with(|| String::with_capacity(json.len()));
            replaced.push_str(&json[copied..escape]);
            replaced.push_str("\\ufffd");
            copied = at;
        }
        if at == found {
            at += 2;
        }
    }
    let mut replaced = replaced?;
    replaced.push_str(&json[copied..]);
    Some(replaced)
}

static HEX: [u8; 256] = {
    let mut values = [0xFF; 256];
    let mut byte = 0;
    while byte < 256 {
        if let Some(value) = (byte as u8 as char).to_digit(16) {
            values[byte] = value as u8;
        }
        byte += 1;
    }
    values
};

/// The surrogate whose escape is `\ud` and these three digits.
#[inline]
fn surrogate(a: u8, b: u8, c: u8) -> Option<u16> {
    let (a, b, c) = (
        HEX[usize::from(a)],
        HEX[usize::from(b)],
        HEX[usize::from(c)],
    );
    if a | b | c == 0xFF || a < 8 {
        return None;
    }
    Some(0xD000 | u16::from(a) << 8 | u16::from(b) << 4 | u16::from(c))
}

#[cfg(test)]
mod tests {
    use hegel::{
        TestCase,
        generators::{booleans, integers, sampled_from, vecs},
    };

    use super::*;

    #[hegel::composite]
    fn chunks(tc: &TestCase) -> Vec<u16> {
        match tc.draw(integers::<u8>().max_value(5)) {
            0 => {
                vec![tc.draw(
                    integers::<u16>().min_value(0xD800).max_value(0xDBFF),
                )]
            }
            1 => {
                vec![tc.draw(
                    integers::<u16>().min_value(0xDC00).max_value(0xDFFF),
                )]
            }
            2 => vec![
                u16::from(b'\\');
                tc.draw(integers::<usize>().min_value(1).max_value(3))
            ],
            3 => {
                let unit = tc.draw(
                    integers::<u16>().min_value(0xD800).max_value(0xDFFF),
                );
                let escape = if tc.draw(booleans()) {
                    format!("\\u{unit:04x}")
                } else {
                    format!("\\u{unit:04X}")
                };
                escape.encode_utf16().collect()
            }
            4 => vec![u16::from(
                tc.draw(sampled_from(b"\"/ud8Dc0fF\n\t\x00 ".to_vec())),
            )],
            _ => vec![tc.draw(integers::<u16>())],
        }
    }

    #[hegel::composite]
    fn json_strings(tc: &TestCase, units: Vec<u16>) -> String {
        let mut json = String::from('"');
        for decoded in char::decode_utf16(units.iter().copied()) {
            let mut buffer = [0; 2];
            let encoded: &[u16] = match decoded {
                Ok(c) if tc.draw(booleans()) => {
                    let quoted = serde_json::to_string(&c).unwrap();
                    json.push_str(&quoted[1..quoted.len() - 1]);
                    continue;
                }
                Ok(c) => c.encode_utf16(&mut buffer),
                Err(error) => {
                    buffer[0] = error.unpaired_surrogate();
                    &buffer[..1]
                }
            };
            for unit in encoded {
                if tc.draw(booleans()) {
                    json.push_str(&format!("\\u{unit:04x}"));
                } else {
                    json.push_str(&format!("\\u{unit:04X}"));
                }
            }
        }
        json.push('"');
        json
    }

    #[hegel::test(test_cases = 1000)]
    fn decodes_as_lossy_utf16(tc: TestCase) {
        let units = tc.draw(vecs(chunks())).concat();
        let json = tc.draw(json_strings(units.clone()));
        let lossy = String::from_utf16_lossy(&units);
        let lone = char::decode_utf16(units.iter().copied())
            .any(|decoded| decoded.is_err());

        let replaced = replace_lone(&json);
        assert_eq!(replaced.is_some(), lone, "{json}");
        assert_eq!(
            serde_json::from_str::<String>(&json).is_err(),
            lone,
            "{json}"
        );
        let replaced = replaced.as_deref().unwrap_or(&json);
        assert_eq!(replaced.len(), json.len(), "{json}");
        assert_eq!(
            serde_json::from_str::<String>(replaced).unwrap(),
            lossy,
            "{json} as {replaced}"
        );
    }

    /// One escape at a time from the start, with no searching.
    fn model(text: &str) -> String {
        fn unit(bytes: &[u8], at: usize) -> Option<u16> {
            let escape = bytes.get(at..at + 6)?;
            let digits = std::str::from_utf8(&escape[2..]).ok()?;
            (escape.starts_with(br"\u")
                && digits.bytes().all(|digit| digit.is_ascii_hexdigit()))
            .then(|| u16::from_str_radix(digits, 16).unwrap())
        }
        let bytes = text.as_bytes();
        let mut replaced = Vec::with_capacity(bytes.len());
        let mut at = 0;
        while at < bytes.len() {
            let length = if bytes[at] != b'\\' {
                1
            } else {
                match unit(bytes, at) {
                    Some(0xD800..=0xDBFF)
                        if matches!(
                            unit(bytes, at + 6),
                            Some(0xDC00..=0xDFFF)
                        ) =>
                    {
                        12
                    }
                    Some(0xD800..=0xDFFF) => {
                        replaced.extend_from_slice(br"\ufffd");
                        at += 6;
                        continue;
                    }
                    Some(_) => 6,
                    None => 2.min(bytes.len() - at),
                }
            };
            replaced.extend_from_slice(&bytes[at..at + length]);
            at += length;
        }
        String::from_utf8(replaced).unwrap()
    }

    #[hegel::test(test_cases = 1000)]
    fn agrees_with_model_on_any_text(tc: TestCase) {
        let text = tc
            .draw(vecs(sampled_from(vec![
                r#"\"#,
                r#"\\"#,
                r#"\ud83d"#,
                r#"\uDE00"#,
                r#"\udc00"#,
                r#"\uD800"#,
                r#"\u0041"#,
                r#"\ud8"#,
                r#"\u"#,
                r#"u"#,
                r#"d83d"#,
                r#"x"#,
                r#"""#,
                r#"é"#,
                r#"\n"#,
            ])))
            .concat();
        let replaced = replace_lone(&text);
        assert_eq!(replaced.as_deref().unwrap_or(&text), model(&text));
        assert_eq!(replaced.is_some(), model(&text) != text);
    }

    #[test]
    fn replaces_lone_surrogates_in_every_position() {
        let replaced = |json| replace_lone(json).unwrap_or_else(|| json.into());
        insta::assert_snapshot!(
            [
                r#""\ud83d""#,
                r#""\ud83dx""#,
                r#""x\ude00""#,
                r#""\ud83d\n""#,
                r#""\ud83d\\ude00""#,
                r#""\ud83d\u0041""#,
                r#""\ud83d\ud83d\ude00""#,
                r#""\ude00\ud83d""#,
                r#""\ud83d\ude00""#,
                r#""\uD83D\uDE00\uDE00""#,
                r#""\\ud83d""#,
                r#""\\\ud83d""#,
                r#""\ud83d\ude0""#,
                r#""\ud83d\"#,
            ]
            .map(|json| format!("{json} -> {}", replaced(json)))
            .join("\n"),
            @r#"
        "\ud83d" -> "\ufffd"
        "\ud83dx" -> "\ufffdx"
        "x\ude00" -> "x\ufffd"
        "\ud83d\n" -> "\ufffd\n"
        "\ud83d\\ude00" -> "\ufffd\\ude00"
        "\ud83d\u0041" -> "\ufffd\u0041"
        "\ud83d\ud83d\ude00" -> "\ufffd\ud83d\ude00"
        "\ude00\ud83d" -> "\ufffd\ufffd"
        "\ud83d\ude00" -> "\ud83d\ude00"
        "\uD83D\uDE00\uDE00" -> "\uD83D\uDE00\ufffd"
        "\\ud83d" -> "\\ud83d"
        "\\\ud83d" -> "\\\ufffd"
        "\ud83d\ude0" -> "\ufffd\ude0"
        "\ud83d\ -> "\ufffd\
        "#
        );
    }
}
