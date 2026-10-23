//! `bombadil trace`: show the states of a binary trace.

use std::{
    fmt,
    fs::File,
    io::{self, BufReader, Read, Write},
    ops::Range,
    path::{Path, PathBuf},
    str::FromStr,
};

use anyhow::{Context, Result, anyhow, bail};
use bombadil_trace::{
    State, StateReader, TRACE_FILE_NAME, TraceDriver, read_payload_from,
};
use serde::{
    Deserialize, Deserializer,
    de::{IgnoredAny, MapAccess, Visitor},
};
use serde_json::value::RawValue;

use crate::browser_trace;

#[derive(clap::Args)]
pub struct TraceArgs {
    /// Trace file (`trace.bin`), or the run directory containing it
    trace_path: PathBuf,
    /// Only show this state, or a range of states such as `40..50` (from
    /// 40 up to but not including 50), `40..` or `..50`
    #[arg(long, value_name = "INDEX|START..END")]
    state: Option<StateRange>,
    /// Print one JSON object per state: its index, metadata and payload
    /// size, with --screen its screen (a string, or null without one), and
    /// with --screenshots the file its screenshot is in, if it has one
    #[arg(long)]
    json: bool,
    /// Show each state's screen (terminal traces)
    #[arg(long)]
    screen: bool,
    /// Show screens with their colors and styles, as ANSI escape sequences
    /// (implies --screen)
    #[arg(long)]
    color: bool,
    /// Write the screenshot of each state shown into this directory, as
    /// `<index>.<format>` (browser traces), or `<index>.bin` for a format
    /// other than png and jpeg. A file already there is left as it is if
    /// it holds the same screenshot, and refused otherwise
    #[arg(long, value_name = "DIRECTORY")]
    screenshots: Option<PathBuf>,
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
    let mut extracted = None;
    let result = ignore_broken_pipe(show_into(
        &args,
        &mut io::stdout().lock(),
        &mut extracted,
    ));
    if let Some(summary) = extraction_summary(&args, extracted.as_ref()) {
        eprintln!("{summary}");
    }
    result
}

/// What `--screenshots` did, once it started (even if showing stopped
/// early, as some may have been written), or `None` if it didn't.
fn extraction_summary(
    args: &TraceArgs,
    extracted: Option<&Extracted>,
) -> Option<String> {
    let (directory, extracted) = (args.screenshots.as_ref()?, extracted?);
    let written = match extracted.written {
        1 => "1 screenshot".to_string(),
        written => format!("{written} screenshots"),
    };
    let skipped = match extracted.skipped {
        1 => "1 was".to_string(),
        skipped => format!("{skipped} were"),
    };
    Some(format!(
        "{written} written to {}, {skipped} already there",
        printable(&directory.to_string_lossy()),
    ))
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

/// Writes the states to `out`, and with `--screenshots` their screenshots
/// to files.
/// Counts those in `extracted`, set once the trace is known to be one
/// they can be extracted from.
fn show_into(
    args: &TraceArgs,
    out: &mut impl Write,
    extracted: &mut Option<Extracted>,
) -> Result<()> {
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
    let show_screen = args.screen || args.color;
    // Opened once: the states are read through a clone of the handle,
    // and the screenshots by offset through the handle itself, so they
    // are from the file whose states were read, even if its path names
    // another trace since (as after an overwriting run).
    let trace_path = if path.is_dir() {
        path.join(TRACE_FILE_NAME)
    } else {
        path.to_path_buf()
    };
    let opened = File::open(&trace_path).with_context(|| {
        format!("failed to open trace file {}", trace_path.display())
    })?;
    let states = opened
        .try_clone()
        .map_err(anyhow::Error::from)
        .and_then(|clone| {
            StateReader::<_, Box<RawValue>>::new(BufReader::new(clone))
        })
        .with_context(|| format!("failed to read {}", trace_path.display()));
    let states = match states {
        Ok(states) => states,
        Err(_) if looks_like_json_trace(path) => bail!(
            "{} looks like a JSON lines trace, {READ_JSON_DIRECTLY}",
            path.display()
        ),
        Err(error) => return Err(error),
    };
    if show_screen && states.driver != TraceDriver::Terminal {
        bail!("--screen only applies to terminal traces");
    }
    if args.screenshots.is_some() && states.driver != TraceDriver::Browser {
        bail!("--screenshots only applies to browser traces");
    }
    let mut extracted = args
        .screenshots
        .as_ref()
        .map(|_| extracted.insert(Extracted::default()));
    // Screenshots are read again by offset, only for the states shown.
    let mut states = if show_screen {
        states
    } else {
        states.without_payloads()
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
        let screen = match (&state.payload, show_screen) {
            (Some(snapshot), true) => {
                let scroll_offset = read_scroll_offset(state.metadata.get());
                Some(if args.json && !args.color {
                    // Escaped as JSON below. With color, the styled text
                    // is filtered as in text mode, so a JSON reader
                    // printing it gets SGR sequences only.
                    render_screen(snapshot, scroll_offset, false)?
                } else {
                    printable_screen(snapshot, scroll_offset, args.color)?
                })
            }
            _ => None,
        };
        let screenshot = match (&args.screenshots, extracted.as_deref_mut()) {
            (Some(directory), Some(extracted)) => Some(extract_screenshot(
                (&trace_path, &opened),
                directory,
                &state,
                extracted,
            )?),
            _ => None,
        };
        if args.json {
            // The metadata as written, so its numbers aren't rounded.
            let mut line = format!(
                r#"{{"index":{},"metadata":{},"payload_bytes":{}"#,
                state.index,
                state.metadata.get(),
                state
                    .payload_length
                    .map_or("null".to_string(), |length| length.to_string())
            );
            if show_screen {
                // A string of the screen's lines, or null without one.
                let screen = screen.as_ref().map(|lines| lines.join("\n"));
                line.push_str(r#","screen":"#);
                line.push_str(&serde_json::to_string(&screen)?);
            }
            if let Some(Screenshot::Extracted { file, .. }) = &screenshot {
                line.push_str(r#","screenshot_file":"#);
                line.push_str(&serde_json::to_string(&file.to_string_lossy())?);
            }
            line.push('}');
            writeln!(out, "{}", escape_json_line(&line))?;
        } else {
            write_state(out, &state, time, start_time)?;
            match &screen {
                Some(screen) => {
                    for line in screen {
                        writeln!(out, "       | {line}")?;
                    }
                }
                None if show_screen => writeln!(
                    out,
                    "       (no screen: no terminal snapshot was recorded)"
                )?,
                None => {}
            }
            if let Some(screenshot) = &screenshot {
                writeln!(out, "       {screenshot}")?;
            }
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

/// [`show_into`], returning what `--screenshots` wrote.
#[cfg(test)]
fn show(args: &TraceArgs, out: &mut impl Write) -> Result<Extracted> {
    let mut extracted = None;
    show_into(args, out, &mut extracted)?;
    Ok(extracted.unwrap_or_default())
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

/// The `state` of a browser state's metadata, as far as it is shown.
#[derive(Default, Deserialize)]
struct BrowserState {
    url: Option<String>,
    /// The screenshot's format (its file extension).
    screenshot: Option<String>,
}

/// The `state` of a browser state's metadata `json`, with the members
/// that aren't strings left out.
fn read_browser_state(json: &str) -> BrowserState {
    #[derive(Deserialize)]
    struct Metadata<'a> {
        #[serde(borrow)]
        state: &'a RawValue,
    }
    #[derive(Deserialize)]
    struct Members<'a> {
        #[serde(borrow, default)]
        url: Option<&'a RawValue>,
        #[serde(borrow, default)]
        screenshot: Option<&'a RawValue>,
    }
    let string =
        |json: Option<&RawValue>| serde_json::from_str(json?.get()).ok();
    read_object::<Metadata>(json)
        .and_then(|metadata| read_object::<Members>(metadata.state.get()))
        .map_or_else(BrowserState::default, |members| BrowserState {
            url: string(members.url),
            screenshot: string(members.screenshot),
        })
}

/// How many screenshots `--screenshots` wrote, and how many were already
/// there.
#[derive(Debug, Default, PartialEq)]
struct Extracted {
    written: usize,
    skipped: usize,
}

/// What became of a state's screenshot with `--screenshots`.
#[derive(Debug, PartialEq)]
enum Screenshot {
    /// In `file`, written or already there.
    Extracted {
        file: PathBuf,
        written: bool,
        /// The recorded format, if it isn't png or jpeg (so the file is
        /// named `<index>.bin`); `None` inside if there is none.
        unknown_format: Option<Option<String>>,
    },
    /// The state has no screenshot.
    Missing,
}

impl fmt::Display for Screenshot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Screenshot::Extracted {
                file,
                written,
                unknown_format,
            } => {
                write!(
                    f,
                    "screenshot: {}",
                    printable(&file.to_string_lossy())
                )?;
                if !written {
                    write!(f, " (already there)")?;
                }
                match unknown_format {
                    Some(Some(format)) => {
                        write!(f, " (unknown format {})", quoted(format))
                    }
                    Some(None) => write!(f, " (no format recorded)"),
                    None => Ok(()),
                }
            }
            Screenshot::Missing => {
                write!(f, "(no screenshot: none was recorded)")
            }
        }
    }
}

/// Writes a state's screenshot, read from `trace` (open at `trace_path`),
/// into `directory` as `<index>.<format>`, or `<index>.bin` for a format
/// other than png and jpeg, since the format comes from the trace and
/// must not name another path or kind of file. A file already there is
/// left alone if it holds the same bytes, so extracting again succeeds,
/// and otherwise refused rather than replaced.
fn extract_screenshot(
    (trace_path, trace): (&Path, &File),
    directory: &Path,
    state: &State<Box<RawValue>>,
    extracted: &mut Extracted,
) -> Result<Screenshot> {
    let Some(offset) = state.payload_offset else {
        return Ok(Screenshot::Missing);
    };
    let format = read_browser_state(state.metadata.get()).screenshot;
    let (extension, unknown_format) = match format {
        Some(format)
            if browser_trace::screenshot_media_type(&format).is_some() =>
        {
            (format, None)
        }
        format => ("bin".to_string(), Some(format)),
    };
    let screenshot = read_payload_from(trace, offset)
        .with_context(|| format!("failed to read {}", trace_path.display()))?;
    std::fs::create_dir_all(directory)
        .with_context(|| format!("failed to create {}", directory.display()))?;
    let file = directory.join(format!("{}.{extension}", state.index));
    let differs = || {
        anyhow!(
            "{} already exists and differs from the screenshot of state {}; \
             remove it or choose another directory",
            file.display(),
            state.index
        )
    };
    let written = match holds(&file, &screenshot)? {
        Some(true) => false,
        Some(false) => return Err(differs()),
        None if create_file(&file, &screenshot)? => true,
        // Created meanwhile.
        None => match holds(&file, &screenshot)? {
            Some(true) => false,
            _ => return Err(differs()),
        },
    };
    if written {
        extracted.written += 1;
    } else {
        extracted.skipped += 1;
    }
    Ok(Screenshot::Extracted {
        file,
        written,
        unknown_format,
    })
}

/// Whether the file at `file` holds `data`, or `None` if there is no
/// file there. Anything there but a regular file (a symbolic link, a
/// directory, a FIFO…) is refused rather than followed or read. Its
/// length is compared first, then its bytes, a chunk at a time.
fn holds(file: &Path, data: &[u8]) -> Result<Option<bool>> {
    let metadata = match std::fs::symlink_metadata(file) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(None);
        }
        Err(error) => {
            return Err(error)
                .with_context(|| format!("failed to read {}", file.display()));
        }
    };
    if !metadata.is_file() {
        bail!("{} exists and isn't a regular file", file.display());
    }
    if metadata.len() != data.len() as u64 {
        return Ok(Some(false));
    }
    let read_error = || format!("failed to read {}", file.display());
    let mut existing = File::open(file).with_context(read_error)?;
    let mut chunk = vec![0; 64 * 1024];
    let mut rest = data;
    loop {
        let read = match existing.read(&mut chunk) {
            Ok(read) => read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {
                continue;
            }
            Err(error) => return Err(error).with_context(read_error),
        };
        if read == 0 {
            return Ok(Some(rest.is_empty()));
        }
        if read > rest.len() || chunk[..read] != rest[..read] {
            return Ok(Some(false));
        }
        rest = &rest[read..];
    }
}

// Numbers the temporary files of this process.
static TEMP_FILES_CREATED: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

/// Creates `file` holding `data`, unless there is a file there: returns
/// whether it did. `data` is written to a new temporary file next to it
/// first (`.<name>.<pid>.<n>.tmp`), then linked to `file`, so `file` is
/// never seen partly written. The temporary file is removed before this
/// returns, whether it succeeds or not, but a process killed meanwhile
/// leaves it behind. Without hard links, it is renamed to `file` after
/// checking `file` doesn't exist, which replaces a file created in
/// between.
fn create_file(file: &Path, data: &[u8]) -> Result<bool> {
    let write_error = || format!("failed to write {}", file.display());
    let directory = file.parent().unwrap_or(Path::new("."));
    let name = file.file_name().unwrap_or_default().to_string_lossy();
    let (temp_path, mut temp) = loop {
        let n = TEMP_FILES_CREATED
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path =
            directory.join(format!(".{name}.{}.{n}.tmp", std::process::id()));
        match File::create_new(&path) {
            Ok(temp) => break (path, temp),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error).with_context(write_error),
        }
    };
    let written = temp.write_all(data);
    drop(temp);
    let created =
        written.and_then(|()| match std::fs::hard_link(&temp_path, file) {
            Ok(()) => Ok(true),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                Ok(false)
            }
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::Unsupported
                        | io::ErrorKind::PermissionDenied
                ) =>
            {
                if std::fs::symlink_metadata(file).is_ok() {
                    Ok(false)
                } else {
                    std::fs::rename(&temp_path, file).map(|()| true)
                }
            }
            Err(error) => Err(error),
        });
    // Gone already if renamed.
    let _ = std::fs::remove_file(&temp_path);
    created.with_context(write_error)
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

    // Unexpected metadata is shown whole above.
    if metadata.is_some()
        && let Some(url) = read_browser_state(state.metadata.get()).url
    {
        writeln!(out, "       url: {}", quoted(&url))?;
    }
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
pub(crate) struct Members<'a>(pub(crate) Vec<(String, &'a RawValue)>);

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

/// The `state.scroll_offset` of a terminal state's metadata: where its
/// viewport was, in rows from the top of the scrollback. `None` if it
/// isn't there as a number, so the screen is shown at the bottom (the
/// active screen) rather than at the top of the scrollback.
fn read_scroll_offset(json: &str) -> Option<usize> {
    #[derive(Deserialize)]
    struct Metadata<'a> {
        #[serde(borrow)]
        state: &'a RawValue,
    }
    #[derive(Deserialize)]
    struct State {
        scroll_offset: usize,
    }
    let metadata: Metadata = read_object(json)?;
    read_object::<State>(metadata.state.get()).map(|state| state.scroll_offset)
}

/// The screen of a terminal snapshot as lines of text, [`printable`] as
/// it is the terminal's text. With `color`, the SGR sequences
/// (`ESC [ … m`) the renderer styles it with are kept, and nothing else.
fn printable_screen(
    snapshot: &[u8],
    scroll_offset: Option<usize>,
    color: bool,
) -> Result<Vec<String>> {
    let plain = render_screen(snapshot, scroll_offset, false)?;
    if !color {
        return Ok(plain.iter().map(|line| printable(line)).collect());
    }
    let styled = render_screen(snapshot, scroll_offset, true)?;
    if styled.len() != plain.len() {
        bail!("failed to render terminal snapshot: rows differ with color");
    }
    Ok(plain
        .iter()
        .zip(&styled)
        .map(|(plain, styled)| printable_styled(plain, styled))
        .collect())
}

/// A line of the screen as `styled` (rendered with SGR sequences), with
/// everything but those sequences [`printable`], and ending with a reset
/// (unless it does already) so its style doesn't carry over. If the
/// text itself (`plain`, the same line rendered without styles) has an
/// escape character, an SGR sequence in `styled` may be text rather than
/// styling, so the line is shown [`printable`] without styles instead.
fn printable_styled(plain: &str, styled: &str) -> String {
    if plain.contains('\x1b') {
        return printable(plain);
    }
    let mut line = String::with_capacity(styled.len());
    let mut styles = false;
    let mut rest = styled;
    while let Some(c) = rest.chars().next() {
        if let Some(sgr) = sgr_prefix(rest) {
            line.push_str(sgr);
            styles = true;
            rest = &rest[sgr.len()..];
        } else {
            line.push_str(&printable(&rest[..c.len_utf8()]));
            rest = &rest[c.len_utf8()..];
        }
    }
    if styles && !line.ends_with("\x1b[0m") && !line.ends_with("\x1b[m") {
        line.push_str("\x1b[0m");
    }
    line
}

/// The SGR sequence `text` starts with, if any: `ESC [`, then digits,
/// `;` and `:` only, then `m`.
fn sgr_prefix(text: &str) -> Option<&str> {
    let parameters = text.strip_prefix("\x1b[")?;
    let length = parameters
        .find(|c: char| !(c.is_ascii_digit() || c == ';' || c == ':'))?;
    (parameters[length..].starts_with('m')).then(|| &text[..length + 3])
}

#[cfg(feature = "terminal")]
fn render_screen(
    snapshot: &[u8],
    scroll_offset: Option<usize>,
    color: bool,
) -> Result<Vec<String>> {
    bombadil_terminal::trace::render_screen(snapshot, scroll_offset, color)
}

#[cfg(not(feature = "terminal"))]
fn render_screen(
    _snapshot: &[u8],
    _scroll_offset: Option<usize>,
    _color: bool,
) -> Result<Vec<String>> {
    bail!("this build of bombadil has no terminal support")
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
            screen: false,
            color: false,
            screenshots: None,
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
        ignore_broken_pipe(show(&args, &mut broken).map(drop)).unwrap();
        let mut full = Failing(io::ErrorKind::StorageFull);
        let error =
            ignore_broken_pipe(show(&args, &mut full).map(drop)).unwrap_err();
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

    /// Writes a finished browser trace of states with these metadata and
    /// payloads.
    fn write_browser_states(
        directory: &Path,
        states: &[(&str, Option<&[u8]>)],
    ) {
        let mut file = Vec::new();
        write_envelope(&mut file, TraceDriver::Browser).unwrap();
        for (metadata, payload) in states {
            match payload {
                Some(payload) => {
                    write_record(
                        &mut file,
                        tag::STATE_WITH_PAYLOAD,
                        &[metadata.as_bytes()],
                    )
                    .unwrap();
                    write_record(&mut file, tag::STATE_PAYLOAD, &[payload])
                        .unwrap();
                }
                None => {
                    write_record(&mut file, tag::STATE, &[metadata.as_bytes()])
                        .unwrap()
                }
            }
        }
        write_record(&mut file, tag::END, &[]).unwrap();
        std::fs::write(directory.join(TRACE_FILE_NAME), file).unwrap();
    }

    fn screenshot_args(directory: &Path, screenshots: &Path) -> TraceArgs {
        TraceArgs {
            screenshots: Some(screenshots.to_path_buf()),
            ..TraceArgs::new(directory.into())
        }
    }

    #[test]
    fn extracts_browser_screenshots() {
        let directory = tempfile::tempdir().unwrap();
        write_browser_states(
            directory.path(),
            &[
                (
                    r#"{"timestamp":0,"action":null,"state":{"url":"https://example.com/","screenshot":"jpeg"},"violations":[]}"#,
                    Some(b"image"),
                ),
                (
                    r#"{"timestamp":1,"action":"Back","state":{"url":"https://example.com/b","screenshot":"png"},"violations":[]}"#,
                    Some(b"png image"),
                ),
            ],
        );
        let screenshots = directory.path().join("screenshots");
        let args = screenshot_args(directory.path(), &screenshots);
        let mut out = Vec::new();
        let extracted = show(&args, &mut out).unwrap();
        assert_eq!(
            extracted,
            Extracted {
                written: 2,
                skipped: 0
            }
        );
        let first = screenshots.join("0.jpeg");
        assert_eq!(
            String::from_utf8(out).unwrap(),
            format!(
                "browser trace\n\
                 #0        +0.000s  (initial state)  (5 B payload)\n\
                 \x20      url: \"https://example.com/\"\n\
                 \x20      screenshot: {}\n\
                 #1        +0.000s  Back  (9 B payload)\n\
                 \x20      url: \"https://example.com/b\"\n\
                 \x20      screenshot: {}\n",
                first.display(),
                screenshots.join("1.png").display()
            )
        );
        assert_eq!(std::fs::read(&first).unwrap(), b"image");
        assert_eq!(
            std::fs::read(screenshots.join("1.png")).unwrap(),
            b"png image"
        );

        // Again: the same screenshots are left as they are.
        let mut out = Vec::new();
        let extracted = show(&args, &mut out).unwrap();
        assert_eq!(
            extracted,
            Extracted {
                written: 0,
                skipped: 2
            }
        );
        let out = String::from_utf8(out).unwrap();
        assert!(
            out.contains(&format!(
                "screenshot: {} (already there)\n",
                first.display()
            )),
            "{out}"
        );

        // Only the states shown.
        std::fs::remove_file(&first).unwrap();
        let extracted = show(
            &TraceArgs {
                state: Some("1".parse().unwrap()),
                ..args
            },
            &mut Vec::new(),
        )
        .unwrap();
        assert_eq!(
            extracted,
            Extracted {
                written: 0,
                skipped: 1
            }
        );
        assert!(!first.exists());

        // A different file is refused, and left as it was.
        std::fs::write(&first, "other").unwrap();
        let args = screenshot_args(directory.path(), &screenshots);
        let error = show(&args, &mut Vec::new()).unwrap_err();
        assert_eq!(
            error.to_string(),
            format!(
                "{} already exists and differs from the screenshot of state \
                 0; remove it or choose another directory",
                first.display()
            )
        );
        assert_eq!(std::fs::read(&first).unwrap(), b"other");

        // As JSON, with the file.
        std::fs::remove_file(&first).unwrap();
        let json = output(&TraceArgs { json: true, ..args });
        let lines: Vec<Value> = json
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(
            lines[0]["screenshot_file"],
            first.to_string_lossy().as_ref()
        );
        assert_eq!(
            lines[0]["metadata"]["state"]["url"],
            "https://example.com/"
        );
        // Again, with the files already there: still named.
        let json = output(&TraceArgs {
            json: true,
            ..screenshot_args(directory.path(), &screenshots)
        });
        let files: Vec<Value> = json
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .map(|line| line["screenshot_file"].clone())
            .collect();
        assert_eq!(
            files,
            [
                first.to_string_lossy().as_ref(),
                screenshots.join("1.png").to_string_lossy().as_ref()
            ]
        );

        let args = TraceArgs {
            screen: true,
            ..TraceArgs::new(directory.path().into())
        };
        let error = show(&args, &mut Vec::new()).unwrap_err();
        assert!(error.to_string().contains("terminal traces"), "{error}");
    }

    /// A file of a screenshot's name that isn't a regular file is refused
    /// rather than followed or read, as is a regular file of the same
    /// length with other bytes; no temporary file is left behind.
    #[test]
    fn refuses_screenshot_files_it_cant_compare() {
        let directory = tempfile::tempdir().unwrap();
        write_browser_states(
            directory.path(),
            &[(r#"{"state":{"screenshot":"jpeg"}}"#, Some(b"image"))],
        );
        let screenshots = directory.path().join("shots");
        std::fs::create_dir(&screenshots).unwrap();
        let file = screenshots.join("0.jpeg");
        let args = screenshot_args(directory.path(), &screenshots);
        let refused = |args: &TraceArgs| {
            show(args, &mut Vec::new()).unwrap_err().to_string()
        };

        // A link to a file with the same bytes.
        let same = directory.path().join("same");
        std::fs::write(&same, "image").unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&same, &file).unwrap();
            assert_eq!(
                refused(&args),
                format!("{} exists and isn't a regular file", file.display())
            );
            std::fs::remove_file(&file).unwrap();
        }
        std::fs::create_dir(&file).unwrap();
        assert_eq!(
            refused(&args),
            format!("{} exists and isn't a regular file", file.display())
        );
        std::fs::remove_dir(&file).unwrap();
        std::fs::write(&file, "imagf").unwrap();
        assert!(refused(&args).contains("differs"));
        std::fs::write(&file, "image").unwrap();
        assert_eq!(
            show(&args, &mut Vec::new()).unwrap(),
            Extracted {
                written: 0,
                skipped: 1
            }
        );
        let entries: Vec<_> = std::fs::read_dir(&screenshots)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(entries, ["0.jpeg"]);
    }

    /// A screenshot is written whole, through a temporary file that is
    /// removed.
    #[test]
    fn creates_files_whole() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("0.jpeg");
        assert!(create_file(&file, b"image").unwrap());
        assert!(!create_file(&file, b"other").unwrap());
        assert_eq!(std::fs::read(&file).unwrap(), b"image");
        let entries: Vec<_> = std::fs::read_dir(directory.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(entries, ["0.jpeg"]);
        // A failure to write it leaves nothing behind.
        let missing = directory.path().join("missing").join("0.jpeg");
        assert!(create_file(&missing, b"image").is_err());
    }

    /// Replaces the trace in `directory` by renaming another over it, as
    /// an overwriting run does, on the first write to it.
    struct ReplacesTrace<'a> {
        directory: &'a Path,
        replaced: bool,
    }

    impl Write for ReplacesTrace<'_> {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            if !self.replaced {
                self.replaced = true;
                let other = tempfile::tempdir().unwrap();
                write_browser_states(
                    other.path(),
                    &[(r#"{"state":{"screenshot":"jpeg"}}"#, Some(b"NEW!!"))],
                );
                std::fs::rename(
                    other.path().join(TRACE_FILE_NAME),
                    self.directory.join(TRACE_FILE_NAME),
                )?;
            }
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    /// Screenshots are extracted from the trace whose states are read,
    /// even if another is renamed over it meanwhile.
    #[test]
    fn extracts_screenshots_of_the_trace_read() {
        let directory = tempfile::tempdir().unwrap();
        write_browser_states(
            directory.path(),
            &[(r#"{"state":{"screenshot":"jpeg"}}"#, Some(b"image"))],
        );
        let screenshots = directory.path().join("shots");
        let args = screenshot_args(directory.path(), &screenshots);
        let mut out = ReplacesTrace {
            directory: directory.path(),
            replaced: false,
        };
        show(&args, &mut out).unwrap();
        assert!(out.replaced);
        assert_eq!(
            std::fs::read(screenshots.join("0.jpeg")).unwrap(),
            b"image"
        );
    }

    /// The summary of `--screenshots` is shown once extraction started,
    /// even if it then failed, but not if it never did.
    #[test]
    fn summarizes_extraction_once_started() {
        let directory = tempfile::tempdir().unwrap();
        let screenshots = directory.path().join("shots");
        let summary = |args: &TraceArgs| {
            let mut extracted = None;
            let result = show_into(args, &mut Vec::new(), &mut extracted);
            (result.is_ok(), extraction_summary(args, extracted.as_ref()))
        };
        // No trace, and a terminal trace: never started.
        let missing = directory.path().join("missing");
        assert_eq!(
            summary(&screenshot_args(&missing, &screenshots)),
            (false, None)
        );
        write_trace(directory.path(), true);
        assert_eq!(
            summary(&screenshot_args(directory.path(), &screenshots)),
            (false, None)
        );
        // Started, wrote nothing, then failed on a file in the way.
        write_browser_states(
            directory.path(),
            &[(r#"{"state":{"screenshot":"jpeg"}}"#, Some(b"image"))],
        );
        std::fs::create_dir_all(screenshots.join("0.jpeg")).unwrap();
        let expected = format!(
            "0 screenshots written to {}, 0 were already there",
            screenshots.display()
        );
        let args = screenshot_args(directory.path(), &screenshots);
        assert_eq!(summary(&args), (false, Some(expected)));
        // Counted in the singular and the plural.
        let one = Extracted {
            written: 1,
            skipped: 1,
        };
        assert_eq!(
            extraction_summary(&args, Some(&one)),
            Some(format!(
                "1 screenshot written to {}, 1 was already there",
                screenshots.display()
            ))
        );
        // Without --screenshots, none.
        assert_eq!(
            summary(&TraceArgs::new(directory.path().into())),
            (true, None)
        );
    }

    #[test]
    fn refuses_screenshots_of_other_traces() {
        let directory = tempfile::tempdir().unwrap();
        write_trace(directory.path(), true);
        let args =
            screenshot_args(directory.path(), &directory.path().join("s"));
        let error = show(&args, &mut Vec::new()).unwrap_err();
        assert_eq!(
            error.to_string(),
            "--screenshots only applies to browser traces"
        );
    }

    /// Only png and jpeg name a screenshot's file; other formats, which
    /// come from the trace, are written as `.bin`, with a note, as is a
    /// screenshot without a format. A state without a screenshot gets a
    /// note.
    #[test]
    fn names_screenshots_by_known_formats_only() {
        let directory = tempfile::tempdir().unwrap();
        write_browser_states(
            directory.path(),
            &[
                (
                    r#"{"state":{"screenshot":"d/../../escaped"}}"#,
                    Some(b"image"),
                ),
                (r#"{"state":{"screenshot":"html"}}"#, Some(b"<script>")),
                (r#"{"state":{"screenshot":"PNG"}}"#, Some(b"image")),
                (r#"{"state":{}}"#, Some(b"image")),
                (r#"{"state":{"screenshot":1}}"#, Some(b"image")),
                (r#"{"state":{"screenshot":"jpeg"}}"#, None),
            ],
        );
        let screenshots = directory.path().join("out").join("shots");
        std::fs::create_dir_all(screenshots.join("0.d")).unwrap();

        let out = output(&screenshot_args(directory.path(), &screenshots));
        let notes: Vec<&str> = out
            .lines()
            .filter(|line| line.starts_with("       "))
            .map(|line| {
                line.trim_start()
                    .split_once(".bin")
                    .map_or(line, |(_, note)| note)
            })
            .collect();
        assert_eq!(
            notes,
            [
                r#" (unknown format "d/../../escaped")"#,
                r#" (unknown format "html")"#,
                r#" (unknown format "PNG")"#,
                " (no format recorded)",
                " (no format recorded)",
                "       (no screenshot: none was recorded)",
            ],
            "{out}"
        );
        let mut files: Vec<String> = std::fs::read_dir(&screenshots)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        files.sort();
        assert_eq!(files, ["0.bin", "0.d", "1.bin", "2.bin", "3.bin", "4.bin"]);
        assert!(!directory.path().join("out").join("escaped").exists());
    }

    /// URLs are recorded from the system under test, so they are escaped
    /// and quoted, and read without `Value`.
    #[test]
    fn escapes_urls() {
        let directory = tempfile::tempdir().unwrap();
        write_browser_states(
            directory.path(),
            &[
                (
                    r#"{"action":null,"state":{"url":"https://x/\u001b[31m\u202e\"a\\b"}}"#,
                    None,
                ),
                (
                    r#"{"action":null,"state":{"$serde_json::private::RawValue":"{\"url\":\"fake\"}"}}"#,
                    None,
                ),
                (r#"{"action":null,"state":{"url":1}}"#, None),
                (r#"{"action":null,"state":"https://x/"}"#, None),
                // Unexpected metadata, shown whole: no separate URL.
                (r#"{"timestamp":"x","state":{"url":"https://x/"}}"#, None),
            ],
        );
        assert_eq!(
            output(&TraceArgs::new(directory.path().into())),
            "browser trace\n\
             #0              ?  (initial state)\n\
             \x20      url: \"https://x/\\u{1b}[31m\\u{202e}\\\"a\\\\b\"\n\
             #1              ?  (initial state)\n\
             #2              ?  (initial state)\n\
             #3              ?  (initial state)\n\
             #4              ?  (unexpected metadata) \
             {\"timestamp\":\"x\",\"state\":{\"url\":\"https://x/\"}}\n"
        );
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

    #[test]
    fn keeps_only_sgr_styling() {
        // SGR sequences are kept, anything else in the styled rendering
        // is escaped, and a reset ends the line.
        assert_eq!(
            printable_styled(
                "ok",
                "\x1b[0m\x1b[38;5;1;48:2::1:2:3mok\x1b]8;;x\x07\x1b[2J\x1b[m"
            ),
            "\x1b[0m\x1b[38;5;1;48:2::1:2:3mok\\u{1b}]8;;x\\u{7}\\u{1b}[2J\x1b[m"
        );
        assert_eq!(printable_styled("ok", "\x1b[1mok"), "\x1b[1mok\x1b[0m");
        assert_eq!(printable_styled("a\u{202e}b", "a\u{202e}b"), "a\\u{202e}b");
        // Text with an escape character of its own is shown without
        // styles, as its SGR-like text can't be told from styling.
        assert_eq!(
            printable_styled("\x1b[31mx", "\x1b[0m\x1b[31mx\x1b[0m"),
            "\\u{1b}[31mx"
        );
        assert_eq!(sgr_prefix("\x1b[1;2mx"), Some("\x1b[1;2m"));
        assert_eq!(sgr_prefix("\x1b[mx"), Some("\x1b[m"));
        for not_sgr in [
            "\x1b[1;2",
            "\x1b[?25h",
            "\x1b[1 m",
            "\x1b]0m",
            "x",
            // Private-parameter `m` sequences: not SGR.
            "\x1b[>4;2m",
            "\x1b[?1m",
        ] {
            assert_eq!(sgr_prefix(not_sgr), None, "{not_sgr:?}");
        }
    }

    #[cfg(feature = "terminal")]
    mod screens {
        use super::*;

        /// A 12×2 terminal's snapshot after `input`, scrolled to the top.
        fn snapshot(input: &[u8]) -> Vec<u8> {
            let mut terminal = libghostty_vt::Terminal::new(12, 2).unwrap();
            terminal.vt_write(input);
            terminal
                .scroll_viewport(libghostty_vt::terminal::ScrollViewport::Top);
            let mut snapshot = Vec::new();
            terminal.encode_snapshot(&mut snapshot).unwrap();
            snapshot
        }

        /// Writes a finished terminal trace of states with these metadata
        /// and snapshots.
        fn write_screens(directory: &Path, states: &[(&str, Option<&[u8]>)]) {
            let mut file = Vec::new();
            write_envelope(&mut file, TraceDriver::Terminal).unwrap();
            for (metadata, snapshot) in states {
                match snapshot {
                    Some(snapshot) => {
                        write_record(
                            &mut file,
                            tag::STATE_WITH_PAYLOAD,
                            &[metadata.as_bytes()],
                        )
                        .unwrap();
                        write_record(
                            &mut file,
                            tag::STATE_PAYLOAD,
                            &[snapshot],
                        )
                        .unwrap();
                    }
                    None => write_record(
                        &mut file,
                        tag::STATE,
                        &[metadata.as_bytes()],
                    )
                    .unwrap(),
                }
            }
            write_record(&mut file, tag::END, &[]).unwrap();
            std::fs::write(directory.join(TRACE_FILE_NAME), file).unwrap();
        }

        fn screen_args(directory: &Path) -> TraceArgs {
            TraceArgs {
                screen: true,
                ..TraceArgs::new(directory.into())
            }
        }

        #[test]
        fn shows_screens_full_height_at_the_scroll_offset() {
            let directory = tempfile::tempdir().unwrap();
            // Three rows of scrollback above the two of the screen.
            let snapshot = snapshot(b"one\r\ntwo\r\nthree\r\nfour\r\nfive");
            let blank = self::snapshot(b"");
            write_screens(
                directory.path(),
                &[
                    (
                        r#"{"action":null,"state":{"scroll_offset":0}}"#,
                        Some(&snapshot),
                    ),
                    (
                        r#"{"action":null,"state":{"scroll_offset":2}}"#,
                        Some(&snapshot),
                    ),
                    // Missing or not a number: the bottom.
                    (r#"{"action":null}"#, Some(&snapshot)),
                    (
                        r#"{"action":null,"state":{"scroll_offset":"0"}}"#,
                        Some(&snapshot),
                    ),
                    (
                        r#"{"action":null,"state":{"scroll_offset":0}}"#,
                        Some(&blank),
                    ),
                    (r#"{"action":null}"#, None),
                ],
            );
            let shown = output(&screen_args(directory.path()));
            let screens: Vec<&str> = shown
                .lines()
                .filter(|line| !line.starts_with('#'))
                .collect();
            assert_eq!(
                screens,
                [
                    "terminal trace",
                    "       | one",
                    "       | two",
                    "       | three",
                    "       | four",
                    "       | four",
                    "       | five",
                    "       | four",
                    "       | five",
                    "       | ",
                    "       | ",
                    "       (no screen: no terminal snapshot was recorded)",
                ],
                "{shown}"
            );
        }

        #[test]
        fn escapes_screens() {
            let directory = tempfile::tempdir().unwrap();
            // Format characters and backslashes in the cells, and a link,
            // a title and a C1 control that rendering must not pass on.
            let snapshot = snapshot(
                "\x1b]0;title\x07\x1b[31ma\u{202e}b\\c\x1b[0m\r\n\
                 \x1b]8;;http://x\x1b\\d\u{9b}2Je\u{200b}\x1b]8;;\x1b\\"
                    .as_bytes(),
            );
            write_screens(
                directory.path(),
                &[(
                    r#"{"action":null,"state":{"scroll_offset":0}}"#,
                    Some(&snapshot),
                )],
            );
            let plain = output(&screen_args(directory.path()));
            assert!(
                plain.ends_with(
                    "       | a\\u{202e}b\\\\c\n       | d2Je\\u{200b}\n"
                ),
                "{plain:?}"
            );

            let colored = output(&TraceArgs {
                color: true,
                ..TraceArgs::new(directory.path().into())
            });
            let lines: Vec<&str> = colored.lines().collect();
            assert_eq!(lines.len(), 4, "{colored:?}");
            // The styling, and the same escaped text without it.
            assert!(lines[2].contains("\x1b[38;5;1m"), "{colored:?}");
            assert!(lines[2].ends_with("\x1b[0m"), "{colored:?}");
            for (line, text) in
                lines[2..].iter().zip(["a\\u{202e}b\\\\c", "d2Je\\u{200b}"])
            {
                let mut without_sgr = line.to_string();
                while let Some(start) = without_sgr.find('\x1b') {
                    let sgr = sgr_prefix(&without_sgr[start..])
                        .expect("only SGR escapes")
                        .len();
                    without_sgr.replace_range(start..start + sgr, "");
                }
                assert_eq!(without_sgr, format!("       | {text}"));
            }
            for evil in ['\x07', '\u{9b}', '\u{202e}', '\u{200b}'] {
                assert!(!colored.contains(evil), "{colored:?}");
            }
            assert!(!colored.contains("\x1b]"), "{colored:?}");

            // The JSON screen with --color is the same: SGR only.
            let json = output(&TraceArgs {
                json: true,
                color: true,
                ..TraceArgs::new(directory.path().into())
            });
            let json: Value = serde_json::from_str(json.trim_end()).unwrap();
            let screen: Vec<&str> =
                json["screen"].as_str().unwrap().split('\n').collect();
            let shown: Vec<&str> = lines[2..]
                .iter()
                .map(|line| line.strip_prefix("       | ").unwrap())
                .collect();
            assert_eq!(screen, shown);
        }

        #[test]
        fn adds_screens_to_json() {
            let directory = tempfile::tempdir().unwrap();
            let snapshot =
                snapshot("\x1b[31mred\u{202e}\x1b[0m\r\nx".as_bytes());
            write_screens(
                directory.path(),
                &[
                    (
                        r#"{"action":null,"state":{"scroll_offset":0}}"#,
                        Some(&snapshot),
                    ),
                    (r#"{"action":null}"#, None),
                ],
            );
            for color in [false, true] {
                let json = output(&TraceArgs {
                    json: true,
                    color,
                    ..screen_args(directory.path())
                });
                assert!(!json.contains(['\x1b', '\u{202e}']), "{json:?}");
                let lines: Vec<Value> = json
                    .lines()
                    .map(|line| serde_json::from_str(line).unwrap())
                    .collect();
                assert_eq!(lines.len(), 2);
                let screen = lines[0]["screen"].as_str().unwrap();
                if color {
                    assert!(screen.contains("\x1b[38;5;1mred"), "{screen:?}");
                } else {
                    assert_eq!(screen, "red\u{202e}\nx");
                }
                assert_eq!(lines[1]["screen"], Value::Null);
                assert_eq!(lines[1]["metadata"]["action"], Value::Null);
            }
            // Without --screen, no screen member.
            let json = output(&TraceArgs {
                json: true,
                ..TraceArgs::new(directory.path().into())
            });
            assert!(!json.contains("screen"), "{json}");
        }

        #[test]
        fn refuses_screens_of_other_traces() {
            let directory = tempfile::tempdir().unwrap();
            let mut file = Vec::new();
            write_envelope(&mut file, TraceDriver::Browser).unwrap();
            write_record(&mut file, tag::END, &[]).unwrap();
            std::fs::write(directory.path().join(TRACE_FILE_NAME), file)
                .unwrap();
            let error = show(&screen_args(directory.path()), &mut Vec::new())
                .unwrap_err();
            assert_eq!(
                error.to_string(),
                "--screen only applies to terminal traces"
            );
        }
    }
}
