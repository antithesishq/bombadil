//! `bombadil trace`: show the states of a binary trace.

use std::{
    fmt,
    fs::File,
    io::{self, Read, Write},
    ops::Range,
    path::{Path, PathBuf},
    str::FromStr,
};

use anyhow::{Result, bail};
use bombadil_trace::{State, TRACE_FILE_NAME, open_trace};
use serde::{
    Deserialize, Deserializer,
    de::{IgnoredAny, MapAccess, Visitor},
};
use serde_json::value::RawValue;

#[derive(clap::Args)]
pub struct TraceArgs {
    /// Trace file (`trace.bin`), or the run directory containing it
    trace_path: PathBuf,
    /// Only show this state, or a range of states such as `40..50` (from
    /// 40 up to but not including 50), `40..` or `..50`
    #[arg(long, value_name = "INDEX|START..END")]
    state: Option<StateRange>,
    /// Print one JSON object per state: its index, metadata and payload
    /// size
    #[arg(long)]
    json: bool,
}

/// States to show, by index.
#[derive(Clone, Debug, PartialEq)]
struct StateRange(Range<usize>);

impl FromStr for StateRange {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let index = |text: &str| {
            text.parse::<usize>()
                .map_err(|_| format!("invalid state index `{text}`"))
        };
        let range = match value.split_once("..") {
            None => {
                let index = index(value)?;
                let end = index
                    .checked_add(1)
                    .ok_or_else(|| format!("invalid state index `{value}`"))?;
                index..end
            }
            Some((start, end)) => {
                let start = if start.is_empty() { 0 } else { index(start)? };
                let end = if end.is_empty() {
                    usize::MAX
                } else {
                    index(end)?
                };
                if start >= end {
                    return Err(format!(
                        "empty state range `{value}` (the end is exclusive)"
                    ));
                }
                start..end
            }
        };
        Ok(StateRange(range))
    }
}

pub fn run(args: TraceArgs) -> Result<()> {
    // Stdout is line buffered and every line ends with a newline, so
    // each line shows (or fails to write) as it comes, and warnings
    // logged on stderr while reading come after the states before them,
    // even through `2>&1 | …`.
    ignore_broken_pipe(show(&args, &mut io::stdout().lock()))
}

/// `result`, or success if it failed because the reader of the output
/// went away, e.g. `bombadil trace … | head`.
fn ignore_broken_pipe(result: Result<()>) -> Result<()> {
    match result {
        Err(error)
            if error.downcast_ref::<io::Error>().map(io::Error::kind)
                == Some(io::ErrorKind::BrokenPipe) =>
        {
            Ok(())
        }
        result => result,
    }
}

/// Said of a JSON lines trace.
const READ_JSON_DIRECTLY: &str = "which can be read directly, e.g. with jq \
                                  or a text editor";

/// Writes the states to `out`.
fn show(args: &TraceArgs, out: &mut impl Write) -> Result<()> {
    let path = &args.trace_path;
    if path.is_dir() {
        let json_trace = path.join("trace.jsonl");
        if !path.join(TRACE_FILE_NAME).exists() && json_trace.is_file() {
            bail!(
                "{} is a JSON lines trace, {READ_JSON_DIRECTLY}",
                json_trace.display()
            );
        }
    }
    let mut states = match open_trace::<Box<RawValue>>(path) {
        Ok(states) => states.without_payloads(),
        Err(_) if looks_like_json_trace(path) => bail!(
            "{} looks like a JSON lines trace, {READ_JSON_DIRECTLY}",
            path.display()
        ),
        Err(error) => return Err(error),
    };
    let range = args
        .state
        .as_ref()
        .map_or(0..usize::MAX, |range| range.0.clone());
    if !args.json {
        writeln!(out, "{} trace", states.driver)?;
    }
    let mut start_time = None;
    let mut states_read = 0;
    // Stop at the end of the range, without reading (and failing on) the
    // records after it.
    while states_read < range.end {
        let Some(state) = states.next() else {
            break;
        };
        let state = state?;
        states_read += 1;
        let time = read_object::<Timed>(state.metadata.get())
            .and_then(|timed| timed.timestamp);
        start_time = start_time.or(time);
        if state.index < range.start {
            continue;
        }
        if args.json {
            // The metadata as written, so its numbers aren't rounded.
            let line = format!(
                r#"{{"index":{},"metadata":{},"payload_bytes":{}}}"#,
                state.index,
                state.metadata.get(),
                state
                    .payload_length
                    .map_or("null".to_string(), |length| length.to_string())
            );
            writeln!(out, "{}", escape_json_line(&line))?;
        } else {
            write_state(out, &state, time, start_time)?;
        }
    }
    if args.state.is_some() && states_read <= range.start {
        let noun = if states_read == 1 { "state" } else { "states" };
        bail!(
            "no state {} in a trace of {states_read} {noun}",
            range.start
        );
    }
    Ok(())
}

/// Whether the file at `path`, which `open_trace` failed on, looks like
/// a legacy JSON lines trace: a regular file not named `trace.bin` (a
/// damaged binary trace is reported as one) starting with `{` (a
/// bombadil trace starts with its magic). Pipes aren't read again.
fn looks_like_json_trace(path: &Path) -> bool {
    if path.file_name() == Some(TRACE_FILE_NAME.as_ref())
        || !path.metadata().is_ok_and(|metadata| metadata.is_file())
    {
        return false;
    }
    let mut first = [0];
    File::open(path)
        .and_then(|mut file| file.read_exact(&mut first))
        .is_ok()
        && first == *b"{"
}

/// A JSON line without the JSON whitespace outside strings, so metadata
/// written over several lines (passed through as written) stays on one,
/// and with the control and format characters [`printable`] escapes in
/// text escaped too (only C0 controls must be escaped in JSON; the
/// backslashes [`printable`] escapes are left as they are). These can
/// only occur inside JSON strings, where `\uXXXX` (two of them, a UTF-16
/// surrogate pair, above U+FFFF) stands for them.
fn escape_json_line(line: &str) -> String {
    let mut escaped = String::with_capacity(line.len());
    for c in compact_json(line).chars() {
        if c.is_control() || is_format_character(c) {
            for unit in c.encode_utf16(&mut [0; 2]) {
                escaped.push_str(&format!("\\u{unit:04x}"));
            }
        } else {
            escaped.push(c);
        }
    }
    escaped
}

/// `json` without the JSON whitespace outside strings.
fn compact_json(json: &str) -> String {
    let mut compact = String::with_capacity(json.len());
    let (mut in_string, mut after_backslash) = (false, false);
    for c in json.chars() {
        if in_string {
            if after_backslash {
                after_backslash = false;
            } else if c == '\\' {
                after_backslash = true;
            } else if c == '"' {
                in_string = false;
            }
        } else if c == '"' {
            in_string = true;
        } else if matches!(c, ' ' | '\t' | '\n' | '\r') {
            continue;
        }
        compact.push(c);
    }
    compact
}

// The metadata is only ever read into these types (and `String`s and
// [`Members`]), never into a `serde_json::Value`: with
// serde_json's `raw_value` feature, a `Value` reads an object whose first
// key is `$serde_json::private::RawValue` as the JSON in its value, so it
// would disagree with the metadata as written. To these types that key
// is just a key.

/// The timestamp of a state's metadata, read on its own so a state with
/// otherwise unexpected metadata still has a time.
#[derive(Deserialize)]
struct Timed {
    #[serde(default, deserialize_with = "present")]
    timestamp: Option<u64>,
}

/// A state's metadata, as shown. Other members are left out.
#[derive(Deserialize)]
struct Metadata<'a> {
    /// Only checked: the time is read with [`Timed`].
    #[serde(rename = "timestamp", default, deserialize_with = "present")]
    _timestamp: Option<u64>,
    /// `None` if absent or null: the initial state.
    #[serde(borrow, default)]
    action: Option<&'a RawValue>,
    /// Each a [`Violation`].
    #[serde(borrow, default, deserialize_with = "present")]
    violations: Option<Vec<&'a RawValue>>,
    /// Whether the metadata has the key `Value` reads specially; such
    /// metadata is shown as unexpected, as its meaning depends on how
    /// it is read.
    #[serde(rename = "$serde_json::private::RawValue")]
    #[serde(default, deserialize_with = "present")]
    raw_value_token: Option<IgnoredAny>,
}

#[derive(Deserialize)]
struct Violation {
    name: String,
}

/// `Some` of a member that is present, whatever it is (even null).
fn present<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

/// `json` read as a `T` if it is a JSON object (a derived `T` reads an
/// array too) that reads as one.
fn read_object<'a, T: Deserialize<'a>>(json: &'a str) -> Option<T> {
    if !json.trim_start().starts_with('{') {
        return None;
    }
    serde_json::from_str(json).ok()
}

/// The metadata `json` and the names of its violations, if it is an
/// object whose timestamp (if any) is a `u64` and whose violations (if
/// any) are an array of objects with a string `name`, so showing its
/// action and violations leaves nothing out.
fn read_metadata(json: &str) -> Option<(Metadata<'_>, Vec<String>)> {
    let metadata: Metadata = read_object(json)?;
    if metadata.raw_value_token.is_some() {
        return None;
    }
    let violations = metadata
        .violations
        .iter()
        .flatten()
        .map(|violation| {
            read_object::<Violation>(violation.get()).map(|v| v.name)
        })
        .collect::<Option<Vec<String>>>()?;
    Some((metadata, violations))
}

/// Writes a state as text, with the time since `start_time`, the time of
/// the first state with a timestamp. Metadata without the expected shape
/// is shown as written.
fn write_state(
    out: &mut impl Write,
    state: &State<Box<RawValue>>,
    time: Option<u64>,
    start_time: Option<u64>,
) -> Result<()> {
    let elapsed = match (time, start_time) {
        // Signed, so a clock that went backwards shows.
        (Some(time), Some(start)) => {
            format!("{:+.3}s", (time as i128 - start as i128) as f64 / 1e6)
        }
        _ => "?".to_string(),
    };
    let metadata = read_metadata(state.metadata.get());
    let action = match metadata.as_ref().map(|(metadata, _)| metadata.action) {
        None => {
            format!("(unexpected metadata) {}", printable(state.metadata.get()))
        }
        Some(None) => "(initial state)".to_string(),
        Some(Some(action)) => describe_action(action),
    };
    write!(out, "#{:<5} {elapsed:>10}  {action}", state.index)?;
    if let Some(length) = state.payload_length {
        write!(out, "  ({} payload)", format_bytes(length))?;
    }
    writeln!(out)?;

    let violations: Vec<String> = metadata
        .iter()
        .flat_map(|(_, violations)| violations)
        .map(|name| quoted(name))
        .collect();
    if !violations.is_empty() {
        writeln!(out, "       violated: {}", violations.join(", "))?;
    }
    Ok(())
}

/// `text` with control characters escaped, so text recorded from the
/// system under test can't send escape sequences to the terminal, and
/// with invisible and bidi format characters escaped, so it can't make
/// names display misleadingly. Backslashes are escaped too, so escaped
/// text doesn't look like these escapes.
fn printable(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_control() || c == '\\' {
                c.escape_debug().to_string()
            } else if is_format_character(c) {
                c.escape_unicode().to_string()
            } else {
                c.to_string()
            }
        })
        .collect()
}

/// Unicode format characters (Cf) plus U+2028/2029 (the line and
/// paragraph separators): invisible, bidi and other characters that
/// change how the text around them displays.
fn is_format_character(c: char) -> bool {
    matches!(
        c,
        '\u{ad}'
            | '\u{600}'..='\u{605}'
            | '\u{61c}'
            | '\u{6dd}'
            | '\u{70f}'
            | '\u{890}'..='\u{891}'
            | '\u{8e2}'
            | '\u{180e}'
            | '\u{200b}'..='\u{200f}'
            | '\u{2028}'..='\u{202e}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{206f}'
            | '\u{feff}'
            | '\u{fff9}'..='\u{fffb}'
            | '\u{110bd}'
            | '\u{110cd}'
            | '\u{13430}'..='\u{1343f}'
            | '\u{1bca0}'..='\u{1bca3}'
            | '\u{1d173}'..='\u{1d17a}'
            | '\u{e0001}'
            | '\u{e0020}'..='\u{e007f}'
    )
}

/// [`printable`] `text` in double quotes, with double quotes in it
/// escaped. (Backslashes are escaped by [`printable`].)
fn quoted(text: &str) -> String {
    format!("\"{}\"", printable(text).replace('"', "\\\""))
}

/// A serde-tagged action (`{"Name": {..fields}}`) as `Name {..fields}`,
/// and a variant without fields (`"Name"`) as `Name`, [`printable`]. A
/// name other than an identifier (as serde writes them) is [`quoted`],
/// so it can't look like another action or the initial state. Fields and
/// other actions are shown as written, [`compact_json`], so their
/// numbers aren't rounded.
fn describe_action(action: &RawValue) -> String {
    let name = |name: &str| {
        let mut chars = name.chars();
        let identifier = chars
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
            && !matches!(name, "true" | "false" | "null");
        if identifier {
            name.to_string()
        } else {
            quoted(name)
        }
    };
    if let Ok(variant) = serde_json::from_str::<String>(action.get()) {
        return name(&variant);
    }
    // Only an object reads as members.
    if let Ok(Members(members)) = serde_json::from_str(action.get())
        && let [(variant, fields)] = members.as_slice()
    {
        let fields = compact_json(fields.get());
        return if fields == "{}" {
            name(variant)
        } else {
            format!("{} {}", name(variant), printable(&fields))
        };
    }
    printable(&compact_json(action.get()))
}

/// The members of a JSON object in order, duplicate keys included.
struct Members<'a>(Vec<(String, &'a RawValue)>);

impl<'de: 'a, 'a> Deserialize<'de> for Members<'a> {
    fn deserialize<D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Self, D::Error> {
        struct MembersVisitor;
        impl<'de> Visitor<'de> for MembersVisitor {
            type Value = Members<'de>;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("an object")
            }
            fn visit_map<A: MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<Self::Value, A::Error> {
                let mut members = Vec::new();
                while let Some(member) = map.next_entry()? {
                    members.push(member);
                }
                Ok(Members(members))
            }
        }
        deserializer.deserialize_map(MembersVisitor)
    }
}

fn format_bytes(bytes: usize) -> String {
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut size = bytes as f64;
    for unit in ["KB", "MB"] {
        size /= 1024.0;
        if size < 1024.0 {
            return format!("{size:.1} {unit}");
        }
    }
    format!("{:.1} GB", size / 1024.0)
}

impl TraceArgs {
    #[cfg(test)]
    fn new(trace_path: PathBuf) -> Self {
        TraceArgs {
            trace_path,
            state: None,
            json: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bombadil_trace::{TraceDriver, tag, write_envelope, write_record};
    use serde_json::Value;

    /// Writes a trace of 4 states, ended with an end record if `finished`.
    fn write_trace(directory: &Path, finished: bool) {
        let mut file = Vec::new();
        write_envelope(&mut file, TraceDriver::Terminal).unwrap();
        for (tag, payload) in [
            (tag::STATE_WITH_PAYLOAD, &br#"{"timestamp":1000000,"action":null,"violations":[]}"#[..]),
            (tag::STATE_PAYLOAD, &[0; 2048][..]),
            (
                tag::STATE,
                br#"{"timestamp":1500000,"action":{"TypeText":{"text":"hi"}},"violations":[{"name":"exitSuccess"}]}"#,
            ),
            (tag::STATE, br#"{"timestamp":2000000,"action":{"ScrollUp":{}},"violations":[]}"#),
            (tag::STATE, br#"{"timestamp":2500000,"action":"Back","violations":[]}"#),
        ] {
            write_record(&mut file, tag, &[payload]).unwrap();
        }
        if finished {
            write_record(&mut file, tag::END, &[]).unwrap();
        }
        std::fs::write(directory.join(TRACE_FILE_NAME), file).unwrap();
    }

    /// Writes a finished trace of states with these metadata.
    fn write_states(directory: &Path, states: &[&[u8]]) {
        let mut file = Vec::new();
        write_envelope(&mut file, TraceDriver::Terminal).unwrap();
        for metadata in states {
            write_record(&mut file, tag::STATE, &[metadata]).unwrap();
        }
        write_record(&mut file, tag::END, &[]).unwrap();
        std::fs::write(directory.join(TRACE_FILE_NAME), file).unwrap();
    }

    fn output(args: &TraceArgs) -> String {
        let mut out = Vec::new();
        show(args, &mut out).unwrap();
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn shows_states() {
        let directory = tempfile::tempdir().unwrap();
        write_trace(directory.path(), true);
        assert_eq!(
            output(&TraceArgs::new(directory.path().into())),
            "terminal trace\n\
             #0        +0.000s  (initial state)  (2.0 KB payload)\n\
             #1        +0.500s  TypeText {\"text\":\"hi\"}\n\
             \x20      violated: \"exitSuccess\"\n\
             #2        +1.000s  ScrollUp\n\
             #3        +1.500s  Back\n"
        );

        // Names that could be read as several, actions that could be
        // read as other actions, a clock that went backwards and a state
        // without a timestamp.
        write_states(
            directory.path(),
            &[
                br#"{"timestamp":2000000,"action":"Foo {\"x\":1}","violations":[{"name":"a, b"},{"name":"c\"d"}]}"#,
                br#"{"timestamp":1000000,"action":{"Foo":{"x":1}}}"#,
                br#"{"action":"(initial state)"}"#,
                br#"{"timestamp":2000000,"action":{"(initial state)":{}}}"#,
                br#"{"action":"true"}"#,
                br#"{"action":"_x9"}"#,
                br#"{"action":"9x"}"#,
                br#"{"action":"false"}"#,
                br#"{"action":"null"}"#,
                br#"{"action":{"A":1,"B":2}}"#,
                // Numbers as written, not as parsed.
                br#"{"action": {"Foo": {"n": 12345678901234567890123, "f": 1.50}}}"#,
                br#"{"action":{"Foo":{"x":1},"Foo":{"y":2}}}"#,
                br#"{"action":{"A":{},"A":{}}}"#,
            ],
        );
        assert_eq!(
            output(&TraceArgs::new(directory.path().into())),
            "terminal trace\n\
             #0        +0.000s  \"Foo {\\\"x\\\":1}\"\n\
             \x20      violated: \"a, b\", \"c\\\"d\"\n\
             #1        -1.000s  Foo {\"x\":1}\n\
             #2              ?  \"(initial state)\"\n\
             #3        +0.000s  \"(initial state)\"\n\
             #4              ?  \"true\"\n\
             #5              ?  _x9\n\
             #6              ?  \"9x\"\n\
             #7              ?  \"false\"\n\
             #8              ?  \"null\"\n\
             #9              ?  {\"A\":1,\"B\":2}\n\
             #10             ?  Foo {\"n\":12345678901234567890123,\"f\":1.50}\n\
             #11             ?  {\"Foo\":{\"x\":1},\"Foo\":{\"y\":2}}\n\
             #12             ?  {\"A\":{},\"A\":{}}\n"
        );
    }

    #[test]
    fn marks_unexpected_metadata() {
        let directory = tempfile::tempdir().unwrap();
        write_states(
            directory.path(),
            &[
                br#"{"timestamp":1000000,"action":null}"#,
                br#"["Back"]"#,
                br#"{"timestamp":1500000,"violations":[{"name":"a"},{"name":1}]}"#,
                br#"{"violations":{"name":"a"}}"#,
                br#"{"timestamp":"2000000","action":"Back"}"#,
                b"{\"timestamp\":1e400,\n\"x\":\"\\u001b\"}",
                br#"[1000000,null,[]]"#,
                br#"[5]"#,
                br#"{"violations":[["a"]]}"#,
                br#"{"timestamp":null}"#,
                br#"{"violations":null}"#,
            ],
        );
        assert_eq!(
            output(&TraceArgs::new(directory.path().into())),
            "terminal trace\n\
             #0        +0.000s  (initial state)\n\
             #1              ?  (unexpected metadata) [\"Back\"]\n\
             #2        +0.500s  (unexpected metadata) \
             {\"timestamp\":1500000,\"violations\":[{\"name\":\"a\"},{\"name\":1}]}\n\
             #3              ?  (unexpected metadata) {\"violations\":{\"name\":\"a\"}}\n\
             #4              ?  (unexpected metadata) \
             {\"timestamp\":\"2000000\",\"action\":\"Back\"}\n\
             #5              ?  (unexpected metadata) \
             {\"timestamp\":1e400,\\n\"x\":\"\\\\u001b\"}\n\
             #6              ?  (unexpected metadata) [1000000,null,[]]\n\
             #7              ?  (unexpected metadata) [5]\n\
             #8              ?  (unexpected metadata) {\"violations\":[[\"a\"]]}\n\
             #9              ?  (unexpected metadata) {\"timestamp\":null}\n\
             #10             ?  (unexpected metadata) {\"violations\":null}\n"
        );
    }

    /// `serde_json::Value` reads an object whose first key is this token
    /// as the JSON in its value; the text shows what is written instead.
    #[test]
    fn shows_the_raw_value_token_as_written() {
        let directory = tempfile::tempdir().unwrap();
        write_states(
            directory.path(),
            &[
                br#"{"$serde_json::private::RawValue":"{\"action\":\"Back\"}"}"#,
                br#"{"action":{"$serde_json::private::RawValue":"{\"Foo\":{\"x\":1}}"}}"#,
                br#"{"\u0024serde_json::private::RawValue":"{\"action\":null,\"timestamp\":7}"}"#,
                br#"{"violations":[{"$serde_json::private::RawValue":"{\"name\":\"fake\"}"}]}"#,
                br#"{"violations":[{"\u0024serde_json::private::RawValue":"{\"name\":\"fake\"}","name":"real"}]}"#,
                br#"{"action":{"Foo":{"$serde_json::private::RawValue":"\"x\""}}}"#,
                br#"{"action":{"$serde_json::private::RawValue":"\"Back\"","Foo":{}}}"#,
                br#"{"action":"Foo_bar"}"#,
            ],
        );
        assert_eq!(
            output(&TraceArgs::new(directory.path().into())),
            [
                "terminal trace",
                r#"#0              ?  (unexpected metadata) {"$serde_json::private::RawValue":"{\\"action\\":\\"Back\\"}"}"#,
                r#"#1              ?  "$serde_json::private::RawValue" "{\\"Foo\\":{\\"x\\":1}}""#,
                r#"#2              ?  (unexpected metadata) {"\\u0024serde_json::private::RawValue":"{\\"action\\":null,\\"timestamp\\":7}"}"#,
                r#"#3              ?  (unexpected metadata) {"violations":[{"$serde_json::private::RawValue":"{\\"name\\":\\"fake\\"}"}]}"#,
                "#4              ?  (initial state)",
                r#"       violated: "real""#,
                r#"#5              ?  Foo {"$serde_json::private::RawValue":"\\"x\\""}"#,
                r#"#6              ?  {"$serde_json::private::RawValue":"\\"Back\\"","Foo":{}}"#,
                "#7              ?  Foo_bar",
                "",
            ]
            .join("\n")
        );
    }

    #[test]
    fn passes_metadata_through_as_json() {
        let directory = tempfile::tempdir().unwrap();
        write_states(
            directory.path(),
            &[
                br#"{"big":18446744073709551616,"small":1e300,"x":[1.50]}"#,
                b"{ \"a\" :\n[1,\t\"b c\\\"\\\\\"]\r\n}",
            ],
        );
        let json = output(&TraceArgs {
            json: true,
            ..TraceArgs::new(directory.path().into())
        });
        assert_eq!(
            json,
            "{\"index\":0,\"metadata\":\
             {\"big\":18446744073709551616,\"small\":1e300,\"x\":[1.50]},\
             \"payload_bytes\":null}\n\
             {\"index\":1,\"metadata\":{\"a\":[1,\"b c\\\"\\\\\"]},\
             \"payload_bytes\":null}\n"
        );
        for line in json.lines() {
            serde_json::from_str::<Value>(line).unwrap();
        }
    }

    #[test]
    fn reads_no_further_than_the_range() {
        let directory = tempfile::tempdir().unwrap();
        write_states(
            directory.path(),
            &[br#"{"action":null}"#, br#"{"action":"Back"}"#, b"{"],
        );
        for range in ["1", "..2"] {
            let args = TraceArgs {
                state: Some(range.parse().unwrap()),
                ..TraceArgs::new(directory.path().into())
            };
            assert!(output(&args).ends_with("  Back\n"), "{range}");
        }
        // The broken state is read past the range.
        let args = TraceArgs {
            state: Some("1..".parse().unwrap()),
            ..TraceArgs::new(directory.path().into())
        };
        assert!(show(&args, &mut Vec::new()).is_err());
    }

    /// Fails every write with an error of this kind.
    struct Failing(io::ErrorKind);

    impl Write for Failing {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(self.0.into())
        }

        fn flush(&mut self) -> io::Result<()> {
            Err(self.0.into())
        }
    }

    #[test]
    fn stops_quietly_on_a_broken_pipe() {
        let directory = tempfile::tempdir().unwrap();
        write_trace(directory.path(), true);
        let args = TraceArgs::new(directory.path().into());
        let mut broken = Failing(io::ErrorKind::BrokenPipe);
        ignore_broken_pipe(show(&args, &mut broken)).unwrap();
        let mut full = Failing(io::ErrorKind::StorageFull);
        let error = ignore_broken_pipe(show(&args, &mut full)).unwrap_err();
        assert_eq!(
            error.downcast_ref::<io::Error>().map(io::Error::kind),
            Some(io::ErrorKind::StorageFull)
        );
    }

    #[test]
    fn shows_a_range_as_json() {
        let directory = tempfile::tempdir().unwrap();
        write_trace(directory.path(), true);
        let args = TraceArgs {
            state: Some("1..".parse().unwrap()),
            json: true,
            ..TraceArgs::new(directory.path().join(TRACE_FILE_NAME))
        };
        let lines: Vec<Value> = output(&args)
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0]["index"], 1);
        assert_eq!(lines[0]["metadata"]["action"]["TypeText"]["text"], "hi");
        assert_eq!(lines[2]["payload_bytes"], Value::Null);
        let first: Value = serde_json::from_str(
            output(&TraceArgs {
                state: Some("0".parse().unwrap()),
                json: true,
                ..TraceArgs::new(directory.path().into())
            })
            .trim_end(),
        )
        .unwrap();
        assert_eq!(first["payload_bytes"], 2048);

        // Times are from the first state of the trace with a timestamp,
        // even if not shown.
        let text = output(&TraceArgs {
            json: false,
            ..args
        });
        assert!(
            text.starts_with(
                "terminal trace\n#1        +0.500s  TypeText {\"text\":\"hi\"}\n"
            ),
            "{text}"
        );
    }

    #[test]
    fn points_json_traces_elsewhere() {
        let directory = tempfile::tempdir().unwrap();
        let json_trace = directory.path().join("trace.jsonl");
        std::fs::write(&json_trace, "{}\n").unwrap();
        let error = |path: &Path| {
            show(&TraceArgs::new(path.into()), &mut Vec::new()).unwrap_err()
        };
        // The run directory.
        assert_eq!(
            error(directory.path()).to_string(),
            format!(
                "{} is a JSON lines trace, which can be read directly, e.g. \
                 with jq or a text editor",
                json_trace.display()
            )
        );
        // A file of any other name than `trace.bin`.
        let other = directory.path().join("other.jsonl");
        std::fs::write(&other, "{}\n").unwrap();
        for path in [&json_trace, &other] {
            assert_eq!(
                error(path).to_string(),
                format!(
                    "{} looks like a JSON lines trace, which can be read \
                     directly, e.g. with jq or a text editor",
                    path.display()
                )
            );
        }
        // A damaged `trace.bin`, or a file not starting with `{`, is no
        // JSON trace.
        let damaged = directory.path().join(TRACE_FILE_NAME);
        std::fs::write(&damaged, "{}\n").unwrap();
        let text = directory.path().join("text");
        std::fs::write(&text, "[]\n").unwrap();
        for path in [&damaged, &text] {
            let error = error(path);
            assert_eq!(
                error.to_string(),
                format!("failed to read {}", path.display())
            );
            assert_eq!(error.root_cause().to_string(), "not a bombadil trace");
        }

        // A run directory with both traces: the binary one is read.
        write_trace(directory.path(), true);
        let shown = output(&TraceArgs::new(directory.path().into()));
        assert!(shown.ends_with("#3        +1.500s  Back\n"), "{shown}");
    }

    #[cfg(unix)]
    #[test]
    fn reads_a_pipe_once() {
        use std::os::fd::AsRawFd;
        let (reader, mut writer) = io::pipe().unwrap();
        // More than the trace reader buffers, so `{` is left to read.
        writer.write_all(&[b'{'; 10_000]).unwrap();
        drop(writer);
        let path = PathBuf::from(format!("/dev/fd/{}", reader.as_raw_fd()));
        let error =
            show(&TraceArgs::new(path.clone()), &mut Vec::new()).unwrap_err();
        assert_eq!(
            error.to_string(),
            format!("failed to read {}", path.display())
        );
    }

    #[test]
    fn refuses_states_of_an_empty_trace() {
        let directory = tempfile::tempdir().unwrap();
        write_states(directory.path(), &[]);
        let args = TraceArgs::new(directory.path().into());
        assert_eq!(output(&args), "terminal trace\n");
        for range in ["0", "..2", "0.."] {
            let args = TraceArgs {
                state: Some(range.parse().unwrap()),
                ..TraceArgs::new(directory.path().into())
            };
            let error = show(&args, &mut Vec::new()).unwrap_err();
            assert_eq!(
                error.to_string(),
                "no state 0 in a trace of 0 states",
                "{range}"
            );
        }
    }

    #[test]
    fn parses_state_ranges() {
        let range =
            |text: &str| text.parse::<StateRange>().map(|range| range.0);
        assert_eq!(range("4"), Ok(4..5));
        assert_eq!(range("4..6"), Ok(4..6));
        assert_eq!(range("4.."), Ok(4..usize::MAX));
        assert_eq!(range("..6"), Ok(0..6));
        for invalid in ["x", "18446744073709551615", "6..4", "4..4", "1..=2"] {
            assert!(range(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn refuses_states_past_the_end() {
        let directory = tempfile::tempdir().unwrap();
        write_trace(directory.path(), true);
        let args = TraceArgs {
            state: Some("4".parse().unwrap()),
            ..TraceArgs::new(directory.path().into())
        };
        let error = show(&args, &mut Vec::new()).unwrap_err();
        assert_eq!(error.to_string(), "no state 4 in a trace of 4 states");

        write_states(directory.path(), &[br#"{"action":null}"#]);
        let args = TraceArgs {
            state: Some("1".parse().unwrap()),
            ..TraceArgs::new(directory.path().into())
        };
        let error = show(&args, &mut Vec::new()).unwrap_err();
        assert_eq!(error.to_string(), "no state 1 in a trace of 1 state");
    }

    #[test]
    fn escapes_control_characters() {
        let directory = tempfile::tempdir().unwrap();
        // Raw (not `\u`-escaped), as serde_json writes non-ASCII.
        let raw = "{\"raw\":\"a\u{9b}\u{7f}\u{202e}\u{2028}\u{e0041}b\"}";
        write_states(
            directory.path(),
            &[
                br#"{"action":{"Evil\u001b[31m\u007f\u0080\u009b\u009f\u202e\u2028\udb40\udc41\\u{1b}":{}},"violations":[{"name":"Bad\u001b[31m\u009b\u202e\u2029"}]}"#,
                raw.as_bytes(),
                "{\"action\":{\"Type\":{\"t\":\"a\u{9b}31m\u{202e}b\u{7f}\"}}}"
                    .as_bytes(),
                "{\"action\":[\"\u{9b}\u{202e}\"]}".as_bytes(),
            ],
        );
        let action_name = "Evil\x1b[31m\u{7f}\u{80}\u{9b}\u{9f}\u{202e}\u{2028}\u{e0041}\\u{1b}";
        let violation_name = "Bad\x1b[31m\u{9b}\u{202e}\u{2029}";
        let evil = [
            '\x1b',
            '\u{7f}',
            '\u{80}',
            '\u{9b}',
            '\u{9f}',
            '\u{202e}',
            '\u{2028}',
            '\u{2029}',
            '\u{e0041}',
        ];

        let args = TraceArgs::new(directory.path().into());
        let text = output(&args);
        assert!(
            text.contains(
                r"Evil\u{1b}[31m\u{7f}\u{80}\u{9b}\u{9f}\u{202e}\u{2028}\u{e0041}\\u{1b}"
            ),
            "{text:?}"
        );
        assert!(
            text.contains(r#"violated: "Bad\u{1b}[31m\u{9b}\u{202e}\u{2029}""#),
            "{text:?}"
        );
        assert!(
            text.ends_with(
                "#2              ?  \
                 Type {\"t\":\"a\\u{9b}31m\\u{202e}b\\u{7f}\"}\n\
                 #3              ?  [\"\\u{9b}\\u{202e}\"]\n"
            ),
            "{text:?}"
        );
        assert!(!text.contains(evil), "{text:?}");

        let json = output(&TraceArgs { json: true, ..args });
        assert!(
            json.contains(
                r"Evil\u001b[31m\u007f\u0080\u009b\u009f\u202e\u2028\udb40\udc41\\u{1b}"
            ),
            "{json:?}"
        );
        assert!(
            json.contains(r"Bad\u001b[31m\u009b\u202e\u2029"),
            "{json:?}"
        );
        assert!(!json.contains(evil), "{json:?}");
        let lines = json
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(lines.len(), 4, "{json:?}");
        let action = lines[0]["metadata"]["action"].as_object().unwrap();
        assert_eq!(action.keys().collect::<Vec<_>>(), [action_name]);
        assert_eq!(
            lines[0]["metadata"]["violations"][0]["name"],
            violation_name
        );
        assert_eq!(
            lines[1]["metadata"],
            serde_json::from_str::<Value>(raw).unwrap()
        );
    }

    #[test]
    fn escapes_format_characters() {
        for c in [
            '\u{ad}',
            '\u{61c}',
            '\u{200b}',
            '\u{200f}',
            '\u{2028}',
            '\u{2029}',
            '\u{202a}',
            '\u{2060}',
            '\u{2066}',
            '\u{2069}',
            '\u{feff}',
            '\u{fff9}',
            '\u{e0041}',
        ] {
            assert_eq!(
                printable(&format!("a{c}b")),
                format!("a{}b", c.escape_unicode())
            );
        }
        assert_eq!(printable("café ✓"), "café ✓");
    }

    #[test]
    fn formats_sizes() {
        assert_eq!(format_bytes(1023), "1023 B");
        assert_eq!(format_bytes(2048), "2.0 KB");
        assert_eq!(format_bytes(100_000_000), "95.4 MB");
        assert_eq!(format_bytes(3 << 30), "3.0 GB");
    }

    #[test]
    fn shows_a_truncated_trace_up_to_the_cut() {
        let directory = tempfile::tempdir().unwrap();
        write_trace(directory.path(), false);
        let path = directory.path().join(TRACE_FILE_NAME);
        let mut file = std::fs::read(&path).unwrap();
        file.truncate(file.len() - 1);
        std::fs::write(&path, file).unwrap();
        let output = output(&TraceArgs::new(directory.path().into()));
        assert!(
            output.contains("#2 ") && !output.contains("#3 "),
            "{output}"
        );
    }
}
