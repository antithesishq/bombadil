use std::{
    fs::File,
    io::BufReader,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::SystemTime,
};

use anyhow::{Context, Result, anyhow};
use axum::{
    Router,
    body::Bytes,
    extract::{Path, State},
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use bombadil_schema::browser::BrowserTraceEntry;
use bombadil_trace::{StateReader, TraceDriver, read_payload_from};
use include_dir::{Dir, include_dir};
use serde_json::value::RawValue;

use crate::{
    browser_trace::{self, BrowserTraceFile},
    output_path,
    trace::Members,
};

static INSPECT_ASSETS: Dir =
    include_dir!("$CARGO_MANIFEST_DIR/../../target/inspect");

/// Response header naming the error that ended reading the trace early,
/// when `/api/trace` serves only the states before it.
const TRACE_ERROR_HEADER: &str = "x-bombadil-trace-error";

/// Response header set to `1` when a binary trace has no end record (its
/// run was killed, or is still writing it).
const TRACE_UNFINISHED_HEADER: &str = "x-bombadil-trace-unfinished";

/// Error messages logged and sent are cut to this many characters, as
/// they can quote strings of any size from the trace.
const ERROR_MESSAGE_CHARS_MAX: usize = 200;

/// `message`, cut to [`ERROR_MESSAGE_CHARS_MAX`] characters and `…` if
/// longer.
fn truncated(message: String) -> String {
    match message.char_indices().nth(ERROR_MESSAGE_CHARS_MAX) {
        Some((end, _)) => format!("{}…", &message[..end]),
        None => message,
    }
}

/// `message`, as shown in a header or a log: escaped, as it can hold
/// text from the trace (control characters, escape sequences), and a
/// header value may only be visible ASCII.
fn escaped(message: &str) -> String {
    message.escape_default().to_string()
}

/// The largest response body for the trace that the page can read:
/// above V8's maximum string length, the page can't load it.
const PAGE_BODY_BYTES_MAX: usize = (1 << 29) - 24;

/// A JSON array of served entries, no larger than `bytes_max`.
struct Body {
    bytes: Vec<u8>,
    states: usize,
    bytes_max: usize,
}

impl Body {
    fn new(bytes_max: usize) -> Self {
        Body {
            bytes: Vec::from(*b"["),
            states: 0,
            bytes_max,
        }
    }

    /// Adds `entry`, or fails with the error to report, the body being
    /// full.
    fn push(&mut self, entry: &str) -> Result<(), String> {
        let separator = usize::from(self.states > 0);
        // With the closing bracket.
        if self.bytes.len() + separator + entry.len() + 1 > self.bytes_max {
            return Err(format!(
                "trace too large for the page; showing the first {} states",
                self.states
            ));
        }
        if separator > 0 {
            self.bytes.push(b',');
        }
        self.bytes.extend_from_slice(entry.as_bytes());
        self.states += 1;
        Ok(())
    }

    fn finish(mut self) -> Bytes {
        self.bytes.push(b']');
        Bytes::from(self.bytes)
    }
}

/// A state's screenshot in a binary trace.
#[derive(Clone, Debug)]
struct ScreenshotLocation {
    /// Offset of the screenshot in the trace file.
    offset: u64,
    /// Its format, as recorded in the trace, and one bombadil writes.
    format: String,
    media_type: &'static str,
}

/// The trace file a scan read, to tell whether the file at its path
/// changed since: any change (replaced, rewritten, or appended to)
/// changes its identity, length or modification time, and
/// [`appended_to`](Self::appended_to) tells appending apart.
#[derive(Clone, Copy, Debug, PartialEq)]
struct FileStamp {
    /// The file's device and inode (Unix only).
    identity: Option<(u64, u64)>,
    length: u64,
    modified: Option<SystemTime>,
}

impl FileStamp {
    fn of(metadata: &std::fs::Metadata) -> Self {
        #[cfg(unix)]
        let identity = {
            use std::os::unix::fs::MetadataExt;
            Some((metadata.dev(), metadata.ino()))
        };
        #[cfg(not(unix))]
        let identity = None;
        FileStamp {
            identity,
            length: metadata.len(),
            modified: metadata.modified().ok(),
        }
    }

    /// Whether `now` is the file this stamp was taken of, maybe appended
    /// to since, as a trace still being written is, so the offsets read
    /// from it still hold.
    ///
    /// On Unix, that is the same device and inode, and at least the
    /// same length. A scan keeps the file it read open, so its inode
    /// can't be freed and reused by another file meanwhile (as file
    /// systems do once a file is deleted): another file at the path,
    /// whether renamed over it, as bombadil replaces a trace, or created
    /// after deleting it, has another identity. A shorter file was cut.
    /// Not detected: the scanned file rewritten in place to at least its
    /// length, which bombadil doesn't do.
    ///
    /// Elsewhere, without an identity, a file at least as long and
    /// modified no earlier is taken as appended to, so a trace replaced
    /// by a longer one isn't detected for screenshots, though the trace
    /// is read again on any change. Either way, screenshots are read
    /// from the scanned file, through its handle.
    fn appended_to(&self, now: &FileStamp) -> bool {
        now.length >= self.length
            && match (self.identity, now.identity) {
                (Some(_), _) | (_, Some(_)) => self.identity == now.identity,
                (None, None) => now.modified >= self.modified,
            }
    }
}

/// What a scan of a binary trace found.
struct Scan {
    /// Numbers the scans, from 1. Screenshot URLs name the scan they come
    /// from, so a URL from a scan of a trace since replaced or cut is not
    /// found, rather than served from the new trace.
    generation: u64,
    /// The first scan whose URLs this one serves: its own generation,
    /// or, if the file was only appended to since the scan before (which
    /// succeeded), that scan's `valid_from`. A state's payload stays where
    /// it was as states are appended, so URLs of those earlier scans
    /// still name the same screenshots, and a reload by one client
    /// doesn't break another's.
    valid_from: u64,
    stamp: FileStamp,
    /// The file scanned, kept open so its screenshots are read from it,
    /// and so its inode isn't reused (see [`FileStamp::appended_to`]).
    /// `None` if it couldn't be opened.
    ///
    /// So inspect holds the trace open while it runs. On Windows, on file
    /// systems without POSIX-semantics rename and delete (e.g. FAT/exFAT,
    /// some network shares), that can make an `--output-path-overwrite`
    /// rerun's rename over the trace, or deleting the run directory,
    /// fail until inspect stops.
    file: Option<File>,
    /// The error if the trace couldn't be read at all.
    result: Result<Scanned, String>,
}

struct Scanned {
    /// The entries read, as served (screenshots pointing at the
    /// screenshot endpoint), as a JSON array.
    body: Bytes,
    states: usize,
    /// Where each state's screenshot is in the file, if it has one that
    /// can be served.
    screenshots: Vec<Option<ScreenshotLocation>>,
    /// The error that ended reading before the end of the trace, if any.
    error: Option<String>,
    /// Whether the trace has no end record.
    unfinished: bool,
}

#[derive(Clone)]
struct AppState {
    trace: Arc<BrowserTraceFile>,
    /// For a binary trace: its latest scan. Held while scanning, so
    /// concurrent requests share one scan.
    scan: Arc<tokio::sync::Mutex<Option<Arc<Scan>>>>,
    /// How many times the trace was scanned.
    scans: Arc<AtomicU64>,
    /// The largest body served for the trace: [`PAGE_BODY_BYTES_MAX`],
    /// but for tests.
    body_bytes_max: usize,
}

impl AppState {
    fn new(trace: BrowserTraceFile) -> Self {
        AppState {
            trace: Arc::new(trace),
            scan: Arc::default(),
            scans: Arc::default(),
            body_bytes_max: PAGE_BODY_BYTES_MAX,
        }
    }
}

pub async fn serve(
    trace_path: PathBuf,
    port: u16,
    open_browser: bool,
) -> Result<()> {
    // Fail before serving rather than when the page asks for the trace.
    let trace = output_path::resolve_browser_trace_file(&trace_path)?;
    let state = AppState::new(trace);

    let app = Router::new()
        .route("/api/trace", get(trace_handler))
        // Legacy traces' screenshot files.
        .route("/api/screenshots/{filename}", get(screenshot_handler))
        // Binary traces' screenshots, by the scan they were read in.
        .route(
            "/api/screenshots/{generation}/{filename}",
            get(binary_screenshot_handler),
        )
        .route("/", get(serve_index))
        .fallback(serve_assets)
        .with_state(state);

    let address = format!("0.0.0.0:{}", port);
    let listener = tokio::net::TcpListener::bind(&address).await?;
    let url = format!("http://{}", address);

    println!("Bombadil Inspect available at {}", url);

    if open_browser && let Err(error) = open::that(&url) {
        log::warn!("Failed to open browser: {}", error);
    }

    axum::serve(listener, app).await?;
    Ok(())
}

/// The trace as served by `/api/trace`.
struct Served {
    /// The entries, as a JSON array.
    body: Bytes,
    /// The error that ended reading them early, if any.
    error: Option<String>,
    unfinished: bool,
}

/// Serves the trace's entries as a JSON array. If a bad record ends the
/// trace early, the entries before it are served, and the error is
/// logged and named in the [`TRACE_ERROR_HEADER`] header.
async fn trace_handler(State(state): State<AppState>) -> Response {
    // TODO: this should be streamed over a websocket or SSE rather than
    // loaded into memory and sent as JSON. OK for now, but worth
    // revisiting when we do "live inspect mode".
    match trace_body(&state).await {
        Ok(served) => {
            let mut response =
                ([(header::CONTENT_TYPE, "application/json")], served.body)
                    .into_response();
            // Escaped, as a header value may only be visible ASCII.
            if let Some(error) = served.error
                && let Ok(value) = HeaderValue::from_str(&escaped(&error))
            {
                response.headers_mut().insert(TRACE_ERROR_HEADER, value);
            }
            if served.unfinished {
                response.headers_mut().insert(
                    TRACE_UNFINISHED_HEADER,
                    HeaderValue::from_static("1"),
                );
            }
            response
        }
        Err(error) => {
            log::error!(
                "Failed to read trace: {}",
                escaped(&truncated(format!("{error:#}")))
            );
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// The trace's entries, as served, logging the error that ended reading
/// them early, if any.
async fn trace_body(state: &AppState) -> Result<Served> {
    let (served, states, file) = match state.trace.as_ref() {
        BrowserTraceFile::Json(file) => {
            let (entries, error) = tokio::task::spawn_blocking({
                let file = file.clone();
                move || browser_trace::read_json_prefix(&file)
            })
            .await??;
            let mut error = error.map(|error| truncated(format!("{error:#}")));
            let mut body = Body::new(state.body_bytes_max);
            for (index, entry) in entries.into_iter().enumerate() {
                // The bytes served, checked as the page reads them.
                let entry = serde_json::to_string(&json_entry_served(entry))?;
                if let Err(state_error) = check_served(&entry) {
                    error = Some(truncated(format!(
                        "invalid state {index}: {state_error}"
                    )));
                    break;
                }
                if let Err(full) = body.push(&entry) {
                    error = Some(full);
                    break;
                }
            }
            let states = body.states;
            let served = Served {
                body: body.finish(),
                error,
                unfinished: false,
            };
            (served, states, file)
        }
        BrowserTraceFile::Binary(file) => {
            let scan = scan(state, file, true).await?;
            let scanned =
                scan.result.as_ref().map_err(|error| anyhow!("{error}"))?;
            let served = Served {
                body: scanned.body.clone(),
                error: scanned.error.clone(),
                unfinished: scanned.unfinished,
            };
            (served, scanned.states, file)
        }
    };
    if let Some(error) = &served.error {
        log::error!(
            "Serving the {states} states of {} before an error: {}",
            file.display(),
            escaped(error)
        );
    }
    Ok(served)
}

/// Checks that the page can read `entry`, a served entry, as it reads it:
/// in the JSON array of the trace, which nests it one level deeper (and
/// nesting is limited), whatever else the array holds.
fn check_served(entry: &str) -> serde_json::Result<()> {
    serde_json::from_str::<[BrowserTraceEntry; 1]>(&format!("[{entry}]"))
        .map(drop)
}

/// A legacy entry, as served: its screenshot pointing at the screenshot
/// endpoint.
fn json_entry_served(mut entry: BrowserTraceEntry) -> BrowserTraceEntry {
    let filename = std::path::Path::new(&entry.state.screenshot)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("")
        .to_string();
    entry.state.screenshot = format!("/api/screenshots/{filename}");
    entry
}

/// The scan of the binary trace `file`: the latest one if the file is
/// unchanged since, or else a new one. One scan runs at a time, and
/// requests arriving during it get its result.
///
/// For a screenshot, the scan of a file appended to since is reused (see
/// [`FileStamp::appended_to`]), so screenshots of a trace still being
/// written keep being served under the URLs the page has, without a scan
/// per request. For the trace, any change is read again, so reloading
/// the page shows new states; if the file was only appended to, the new
/// scan keeps serving the URLs of the one before (see
/// [`Scan::valid_from`]), so a reload doesn't break the screenshots of
/// another page showing the trace. A truncated or replaced trace, or one
/// after a failed scan, starts over, and older URLs are not found.
///
/// For a screenshot, a failed scan is reused too, so a failure isn't
/// retried on every request. For the trace (`for_trace`), it is retried,
/// so reloading the page retries it.
async fn scan(
    state: &AppState,
    file: &std::path::Path,
    for_trace: bool,
) -> Result<Arc<Scan>> {
    let mut latest = state.scan.clone().lock_owned().await;
    let stamp = {
        let metadata = tokio::fs::metadata(file)
            .await
            .with_context(|| format!("failed to read {}", file.display()))?;
        FileStamp::of(&metadata)
    };
    if let Some(scan) = latest.as_ref()
        && if for_trace {
            scan.result.is_ok() && scan.stamp == stamp
        } else {
            scan.stamp.appended_to(&stamp)
        }
    {
        return Ok(scan.clone());
    }
    let generation = state.scans.fetch_add(1, Ordering::Relaxed) + 1;
    let body_bytes_max = state.body_bytes_max;
    let file = file.to_path_buf();
    let opened_stamp = stamp;
    // Scanned in a task of its own, which holds the lock until the scan
    // is done, so a request cancelled meanwhile doesn't let another scan
    // start alongside it.
    tokio::spawn(async move {
        let mut scan = tokio::task::spawn_blocking(move || {
            scan_trace(&file, opened_stamp, generation, body_bytes_max)
        })
        .await?;
        // Compared while the previous scan still holds its file open, so
        // its identity can't have been reused by another file.
        if let Some(previous) = latest.as_ref()
            && previous.result.is_ok()
            && scan.result.is_ok()
            && previous.stamp.appended_to(&scan.stamp)
        {
            scan.valid_from = previous.valid_from;
        }
        // A failure is logged by the handler of each request it fails.
        let scan = Arc::new(scan);
        *latest = Some(scan.clone());
        Ok::<_, anyhow::Error>(scan)
    })
    .await?
}

/// Reads the binary trace `file`: each state's entry, and where its
/// screenshot is. Payloads are checked but not kept; screenshots are read
/// when requested.
///
/// `path_stamp` is the stamp of the path when the scan started; a trace
/// that can't be opened gets it, so screenshot requests reuse that failed
/// scan until the path changes or `/api/trace` retries.
fn scan_trace(
    file: &std::path::Path,
    path_stamp: FileStamp,
    generation: u64,
    body_bytes_max: usize,
) -> Scan {
    let started = std::time::Instant::now();
    let opened = File::open(file)
        .and_then(|opened| Ok((opened.metadata()?, opened)))
        .with_context(|| format!("failed to open {}", file.display()));
    let (metadata, opened) = match opened {
        Ok(opened) => opened,
        Err(error) => {
            return Scan {
                generation,
                valid_from: generation,
                stamp: path_stamp,
                file: None,
                result: Err(truncated(format!("{error:#}"))),
            };
        }
    };
    // The stamp of the file read, even if the path is replaced meanwhile.
    let stamp = FileStamp::of(&metadata);
    let result = scan_states(file, &opened, generation, body_bytes_max)
        .map_err(|error| truncated(format!("{error:#}")));
    if let Ok(scanned) = &result {
        log::info!(
            "Read {} states of {} in {:.1?}",
            scanned.states,
            file.display(),
            started.elapsed()
        );
    }
    Scan {
        generation,
        valid_from: generation,
        stamp,
        file: Some(opened),
        result,
    }
}

/// The JSON object `object` with the value of its member `key` replaced
/// by `value` of it. The rest is as written, member by member.
fn with_member(
    object: &str,
    key: &str,
    value: impl FnOnce(&str) -> Result<String>,
) -> Result<String> {
    let Members(members) = serde_json::from_str(object)?;
    let mut value = Some(value);
    let mut spliced = String::with_capacity(object.len() + 32);
    spliced.push('{');
    for (index, (name, member)) in members.iter().enumerate() {
        if index > 0 {
            spliced.push(',');
        }
        spliced.push_str(&serde_json::to_string(name)?);
        spliced.push(':');
        match value.take_if(|_| name == key) {
            Some(value) => spliced.push_str(&value(member.get())?),
            None => spliced.push_str(member.get()),
        }
    }
    spliced.push('}');
    Ok(spliced)
}

/// A state's metadata as served: as recorded, with its screenshot
/// replaced by `url`.
fn served_entry(metadata: &str, url: &str) -> Result<String> {
    with_member(metadata, "state", |state| {
        with_member(state, "screenshot", |_| Ok(serde_json::to_string(url)?))
    })
}

fn scan_states(
    file: &std::path::Path,
    opened: &File,
    generation: u64,
    body_bytes_max: usize,
) -> Result<Scanned> {
    let mut states =
        StateReader::<_, Box<RawValue>>::new(BufReader::new(opened))
            .and_then(|states| states.expect_driver(TraceDriver::Browser))
            .with_context(|| format!("failed to read {}", file.display()))?
            .without_payloads();
    let mut body = Body::new(body_bytes_max);
    let mut screenshots = Vec::new();
    let mut error = None;
    let mut missing = 0;
    for trace_state in states.by_ref() {
        let trace_state = match trace_state {
            Ok(trace_state) => trace_state,
            Err(state_error) => {
                error = Some(truncated(format!("{state_error:#}")));
                break;
            }
        };
        let index = trace_state.index;
        let metadata = trace_state.metadata.get();
        // The page reads the whole trace as `BrowserTraceEntry`s, and shows
        // nothing if one fails to read, so a state it can't read ends the
        // trace as a bad record does. Only a check: the metadata is served
        // as recorded, but for the screenshot's URL, so whatever the page
        // reads, it reads from what was recorded.
        let parsed = match serde_json::from_str::<BrowserTraceEntry>(metadata) {
            Ok(parsed) => parsed,
            Err(state_error) => {
                error = Some(truncated(format!(
                    "invalid state {index}: {state_error}"
                )));
                break;
            }
        };
        let format = parsed.state.screenshot.clone();
        let location = trace_state.payload_offset.and_then(|offset| {
            browser_trace::screenshot_media_type(&format).map(|media_type| {
                ScreenshotLocation {
                    offset,
                    format: format.clone(),
                    media_type,
                }
            })
        });
        // Named by the scan and the state's index; a screenshot that
        // can't be served gets a name without a format, which is not
        // found.
        let url = match &location {
            Some(location) => format!(
                "/api/screenshots/{generation}/{index}.{}",
                location.format
            ),
            None => {
                missing += 1;
                format!("/api/screenshots/{generation}/{index}")
            }
        };
        let entry = match served_entry(metadata, &url) {
            Ok(entry) => entry,
            Err(state_error) => {
                error = Some(truncated(format!(
                    "invalid state {index}: {state_error:#}"
                )));
                break;
            }
        };
        // Both the served bytes and the entry as read from them are
        // checked, as each can nest deeper than the other: a value read
        // from the JSON in a string (the RawValue token, with
        // serde_json's `raw_value` feature, see `browser_trace`) is read
        // with a nesting limit of its own, so the page reads it, but
        // nests deeper than the bytes; a duplicate key keeps only its
        // last value, which may nest less than the bytes. Checking both
        // is a conservative bound, which may refuse some states the page
        // could read, but keeps the depth of what it parses bounded.
        let checked = serde_json::to_string(&parsed)
            .map_err(anyhow::Error::from)
            .and_then(|parsed| Ok(check_served(&parsed)?))
            .and_then(|()| Ok(check_served(&entry)?));
        if let Err(state_error) = checked {
            error = Some(truncated(format!(
                "invalid state {index}: {state_error}"
            )));
            break;
        }
        if let Err(full) = body.push(&entry) {
            error = Some(full);
            break;
        }
        screenshots.push(location);
    }
    if missing > 0 {
        log::warn!(
            "{missing} states of {} have no screenshot that can be shown \
             (none was recorded, or its format isn't png or jpeg)",
            file.display()
        );
    }
    // Only of a trace read to its end: one cut short by an error lacks
    // its end record anyway.
    let unfinished = error.is_none() && !states.finished();
    Ok(Scanned {
        body: body.finish(),
        states: screenshots.len(),
        screenshots,
        error,
        unfinished,
    })
}

impl Scanned {
    /// The location of the screenshot named `filename` (`<index>.<format>`,
    /// as named in the served trace).
    fn locate(&self, filename: &str) -> Option<&ScreenshotLocation> {
        let (index, format) = filename.split_once('.')?;
        let index = index.parse::<usize>().ok()?;
        self.screenshots
            .get(index)?
            .as_ref()
            // Only the exact name, so the response's type is the
            // screenshot's.
            .filter(|location| {
                location.format == format
                    && filename == format!("{index}.{format}")
            })
    }
}

/// The media type and bytes of the screenshot named `filename` by the
/// scan numbered `generation` of a binary trace, or `None` if there is
/// none by that name in the latest scan.
async fn binary_screenshot(
    state: &AppState,
    file: &std::path::Path,
    generation: &str,
    filename: &str,
) -> Result<Option<(&'static str, Vec<u8>)>> {
    let scan = scan(state, file, false).await?;
    let scanned = scan.result.as_ref().map_err(|error| anyhow!("{error}"))?;
    // A URL of an earlier scan, of a trace since replaced or cut, isn't
    // served; one of a trace only appended to since is.
    let Some(generation) = generation
        .parse::<u64>()
        .ok()
        .filter(|parsed| parsed.to_string() == generation)
    else {
        return Ok(None);
    };
    if !(scan.valid_from..=scan.generation).contains(&generation) {
        return Ok(None);
    }
    let Some(location) = scanned.locate(filename).cloned() else {
        return Ok(None);
    };
    // From the file scanned, through its handle, so from the trace the
    // URL's scan read even if the path names another trace by now.
    // Positional reads, so concurrent requests don't race on the handle.
    let path = file.to_path_buf();
    let data = tokio::task::spawn_blocking(move || -> Result<_> {
        let opened = scan.file.as_ref().context("the trace isn't open")?;
        read_payload_from(opened, location.offset)
            .with_context(|| format!("failed to read {}", path.display()))
    })
    .await??;
    Ok(Some((location.media_type, data)))
}

/// A legacy trace's screenshot file.
async fn screenshot_handler(
    State(state): State<AppState>,
    Path(filename): Path<String>,
) -> Response {
    let BrowserTraceFile::Json(file) = state.trace.as_ref() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let sanitized = std::path::Path::new(&filename)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("")
        .to_string();
    let directory = output_path::resolve_trace_directory(file);
    match tokio::fs::read(directory.join("screenshots").join(&sanitized)).await
    {
        Ok(data) => {
            let mime =
                mime_guess::from_path(&sanitized).first_or_octet_stream();
            ([(header::CONTENT_TYPE, mime.as_ref().to_string())], data)
                .into_response()
        }
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

/// A binary trace's screenshot.
async fn binary_screenshot_handler(
    State(state): State<AppState>,
    Path((generation, filename)): Path<(String, String)>,
) -> Response {
    let BrowserTraceFile::Binary(file) = state.trace.as_ref() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    match binary_screenshot(&state, file, &generation, &filename).await {
        Ok(Some((media_type, data))) => {
            ([(header::CONTENT_TYPE, media_type)], data).into_response()
        }
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(error) => {
            log::warn!(
                "Failed to read screenshot: {}",
                escaped(&truncated(format!("{error:#}")))
            );
            StatusCode::NOT_FOUND.into_response()
        }
    }
}

async fn serve_index() -> axum::response::Html<&'static str> {
    let html = INSPECT_ASSETS
        .get_file("index.html")
        .expect("index.html not found in embedded assets")
        .contents_utf8()
        .expect("index.html is not valid UTF-8");
    axum::response::Html(html)
}

async fn serve_assets(uri: axum::http::Uri) -> impl IntoResponse {
    let path = uri.path().trim_start_matches('/');
    if let Some(file) = INSPECT_ASSETS.get_file(path) {
        let mime = mime_guess::from_path(path).first_or_octet_stream();
        ([(header::CONTENT_TYPE, mime.as_ref())], file.contents())
            .into_response()
    } else {
        StatusCode::NOT_FOUND.into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bombadil_trace::{TRACE_FILE_NAME, tag, write_envelope, write_record};

    /// The metadata of a state whose screenshot has `format`.
    fn metadata(url: &str, format: &str) -> String {
        format!(
            r#"{{"timestamp":0,"action":null,"state":{{"url":"{url}","hash_previous":null,"hash_current":null,"screenshot":"{format}","resources":{{"js_heap_used":0,"js_heap_total":0,"dom_nodes":0,"documents":0,"js_event_listeners":0,"layout_objects":0,"timestamp":0.0,"thread_time":0.0,"task_duration":0.0,"script_duration":0.0}}}},"snapshots":[],"violations":[]}}"#
        )
    }

    /// A browser trace's bytes, of states with this metadata and payload
    /// (or none), finished.
    fn trace(states: &[(String, Option<&[u8]>)]) -> Vec<u8> {
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
        file
    }

    /// A trace of 200 states with 100 KB screenshots, slow enough to scan
    /// that requests overlap the scan.
    fn large_trace(path: &std::path::Path) -> Vec<u8> {
        let image = vec![7; 100_000];
        let states: Vec<_> = (0..200)
            .map(|_| (metadata("u", "jpeg"), Some(&image[..])))
            .collect();
        std::fs::write(path, trace(&states)).unwrap();
        image
    }

    fn binary_state(path: &std::path::Path) -> AppState {
        AppState::new(BrowserTraceFile::Binary(path.to_path_buf()))
    }

    fn block_on<F: std::future::Future>(future: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(future)
    }

    fn served(body: &Bytes) -> Vec<BrowserTraceEntry> {
        serde_json::from_slice(body).unwrap()
    }

    /// The screenshot URLs of the trace as served.
    async fn screenshot_urls(state: &AppState) -> Vec<String> {
        let served = trace_body(state).await.unwrap();
        self::served(&served.body)
            .into_iter()
            .map(|entry| entry.state.screenshot)
            .collect()
    }

    /// The screenshot at a served URL.
    async fn screenshot_at(
        state: &AppState,
        url: &str,
    ) -> Option<(&'static str, Vec<u8>)> {
        let BrowserTraceFile::Binary(file) = state.trace.as_ref() else {
            panic!("not a binary trace");
        };
        let (generation, filename) = url
            .strip_prefix("/api/screenshots/")
            .unwrap()
            .split_once('/')
            .unwrap();
        binary_screenshot(state, file, generation, filename)
            .await
            .unwrap()
    }

    #[test]
    fn locates_screenshots_by_their_exact_names() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(TRACE_FILE_NAME);
        std::fs::write(
            &path,
            trace(&[
                (metadata("u", "jpeg"), Some(b"image")),
                (metadata("u", "png"), Some(b"png image")),
            ]),
        )
        .unwrap();
        let state = binary_state(&path);
        block_on(async {
            // Before the trace was requested.
            assert_eq!(
                binary_screenshot(&state, &path, "1", "0.jpeg")
                    .await
                    .unwrap(),
                Some(("image/jpeg", b"image".to_vec()))
            );
            assert_eq!(
                screenshot_urls(&state).await,
                ["/api/screenshots/1/0.jpeg", "/api/screenshots/1/1.png"]
            );
            assert_eq!(
                screenshot_at(&state, "/api/screenshots/1/1.png").await,
                Some(("image/png", b"png image".to_vec()))
            );
            for (generation, name) in [
                ("1", "0.html"),
                ("1", "00.jpeg"),
                ("1", "+0.jpeg"),
                ("1", "0.jpeg.html"),
                ("1", "2.jpeg"),
                ("1", "0"),
                ("2", "0.jpeg"),
                ("01", "0.jpeg"),
            ] {
                assert_eq!(
                    binary_screenshot(&state, &path, generation, name)
                        .await
                        .unwrap(),
                    None,
                    "{generation}/{name}"
                );
            }
        });
    }

    /// A state without a payload, or whose format bombadil doesn't write,
    /// has a screenshot name that is not found.
    #[test]
    fn screenshots_that_cant_be_served_are_not_found() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(TRACE_FILE_NAME);
        std::fs::write(
            &path,
            trace(&[
                (metadata("u", "jpeg"), None),
                (metadata("u", "html"), Some(b"<script>")),
                (metadata("u", ""), Some(b"image")),
            ]),
        )
        .unwrap();
        let state = binary_state(&path);
        block_on(async {
            let urls = screenshot_urls(&state).await;
            assert_eq!(
                urls,
                [
                    "/api/screenshots/1/0",
                    "/api/screenshots/1/1",
                    "/api/screenshots/1/2"
                ]
            );
            for name in ["0", "0.jpeg", "1", "1.html", "2", "2."] {
                assert_eq!(
                    binary_screenshot(&state, &path, "1", name).await.unwrap(),
                    None,
                    "{name}"
                );
            }
        });
    }

    /// Concurrent first requests share one scan of the trace, and the
    /// trace request reuses it.
    #[test]
    fn scans_the_trace_once() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(TRACE_FILE_NAME);
        let image = large_trace(&path);
        let state = binary_state(&path);
        block_on(async {
            let requests: Vec<_> = (0..8)
                .map(|index| {
                    let state = state.clone();
                    let path = path.clone();
                    tokio::spawn(async move {
                        binary_screenshot(
                            &state,
                            &path,
                            "1",
                            &format!("{index}.jpeg"),
                        )
                        .await
                    })
                })
                .collect();
            for request in requests {
                let (_, data) = request.await.unwrap().unwrap().unwrap();
                assert_eq!(data, image);
            }
            assert_eq!(screenshot_urls(&state).await.len(), 200);
        });
        assert_eq!(state.scans.load(Ordering::Relaxed), 1);
    }

    /// A request cancelled during the scan it started (the client went
    /// away) leaves the scan running, and later requests wait for it
    /// rather than starting another.
    #[test]
    fn a_cancelled_request_leaves_its_scan_running() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(TRACE_FILE_NAME);
        let image = large_trace(&path);
        let state = binary_state(&path);
        block_on(async {
            let cancelled = tokio::spawn({
                let state = state.clone();
                let path = path.clone();
                async move { binary_screenshot(&state, &path, "1", "0.jpeg").await }
            });
            while state.scans.load(Ordering::Relaxed) == 0 {
                tokio::task::yield_now().await;
            }
            cancelled.abort();
            assert!(cancelled.await.unwrap_err().is_cancelled());
            let requests: Vec<_> = (0..8)
                .map(|index| {
                    let state = state.clone();
                    let path = path.clone();
                    tokio::spawn(async move {
                        binary_screenshot(
                            &state,
                            &path,
                            "1",
                            &format!("{index}.jpeg"),
                        )
                        .await
                    })
                })
                .collect();
            for request in requests {
                let (_, data) = request.await.unwrap().unwrap().unwrap();
                assert_eq!(data, image);
            }
        });
        assert_eq!(state.scans.load(Ordering::Relaxed), 1);
    }

    /// A bad record mid-trace: the states before it are served, and the
    /// error is reported.
    #[test]
    fn serves_the_states_before_a_bad_record() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(TRACE_FILE_NAME);
        let mut file = trace(&[
            (metadata("https://a/", "jpeg"), Some(b"a")),
            (metadata("https://b/", "jpeg"), Some(b"b")),
            (metadata("https://c/", "jpeg"), Some(b"c")),
        ]);
        std::fs::write(&path, &file).unwrap();
        // Corrupt the second state's payload.
        let offset = browser_trace::open_binary(&path)
            .unwrap()
            .nth(1)
            .unwrap()
            .unwrap()
            .payload_offset
            .unwrap();
        file[offset as usize] = b'x';
        std::fs::write(&path, file).unwrap();
        let state = binary_state(&path);
        block_on(async {
            let response = trace_handler(State(state.clone())).await;
            assert_eq!(response.status(), StatusCode::OK);
            let error = response.headers()[TRACE_ERROR_HEADER]
                .to_str()
                .unwrap()
                .to_string();
            assert!(error.contains("trace record 3"), "{error}");
            let served = trace_body(&state).await.unwrap();
            let entries = self::served(&served.body);
            assert_eq!(entries.len(), 1);
            assert_eq!(entries[0].state.url, "https://a/");
            assert!(served.error.unwrap().contains("checksum"));
            assert_eq!(
                screenshot_at(&state, &entries[0].state.screenshot).await,
                Some(("image/jpeg", b"a".to_vec()))
            );
        });

        // A legacy trace with a bad line too.
        let json = directory.path().join("trace.jsonl");
        std::fs::write(
            &json,
            format!(
                "{}\n{{\n{}\n",
                metadata("https://a/", "a.png"),
                metadata("https://c/", "c.png")
            ),
        )
        .unwrap();
        let state = AppState::new(BrowserTraceFile::Json(json.clone()));
        let served = block_on(trace_body(&state)).unwrap();
        let entries = self::served(&served.body);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].state.screenshot, "/api/screenshots/a.png");
        assert_eq!(
            served.error.unwrap(),
            format!(
                "failed to parse {}:2: EOF while parsing an object at line 1 \
                 column 1",
                json.display()
            )
        );
    }

    /// A state that isn't a browser entry ends the trace like a bad
    /// record, and an error quoting a long string from the trace is cut
    /// short, and sent escaped as the header value only allows visible
    /// ASCII.
    #[test]
    fn cuts_long_errors_short() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(TRACE_FILE_NAME);
        let long = "é".repeat(100_000);
        let bad = metadata("u", "jpeg").replacen(
            r#""timestamp":0,"#,
            &format!(r#""timestamp":"{long}","#),
            1,
        );
        std::fs::write(
            &path,
            trace(&[(metadata("u", "jpeg"), Some(b"a")), (bad, None)]),
        )
        .unwrap();
        let state = binary_state(&path);
        block_on(async {
            let served = trace_body(&state).await.unwrap();
            assert_eq!(self::served(&served.body).len(), 1);
            let error = served.error.unwrap();
            assert!(error.starts_with("invalid state 1: "), "{error}");
            assert!(error.ends_with("é…"), "{error}");
            assert_eq!(error.chars().count(), ERROR_MESSAGE_CHARS_MAX + 1);

            let response = trace_handler(State(state.clone())).await;
            let header =
                response.headers()[TRACE_ERROR_HEADER].to_str().unwrap();
            assert!(header.len() < 2_000, "{}", header.len());
            assert!(header.ends_with(r"\u{e9}\u{2026}"), "{header}");
        });
        assert_eq!(truncated("short".to_string()), "short");
    }

    /// The metadata is served as recorded (numbers as written, members
    /// the page doesn't read kept), but for the screenshot's URL.
    #[test]
    fn serves_metadata_as_recorded() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(TRACE_FILE_NAME);
        let snapshot = r#"{"index":0,"name":"x","value":{"n":1.50,"big":18446744073709551616},"time":0}"#;
        let recorded = metadata("u", "jpeg").replace(
            r#""snapshots":[]"#,
            &format!(r#""snapshots":[{snapshot}],"extra":{{"n":1.50}}"#),
        );
        std::fs::write(&path, trace(&[(recorded.clone(), Some(b"a"))]))
            .unwrap();
        let state = binary_state(&path);
        let served = block_on(trace_body(&state)).unwrap();
        let body = std::str::from_utf8(&served.body).unwrap();
        assert_eq!(
            body,
            format!(
                "[{}]",
                recorded.replace(
                    r#""screenshot":"jpeg""#,
                    r#""screenshot":"/api/screenshots/1/0.jpeg""#
                )
            )
        );
    }

    /// A state the page can't read as a `BrowserTraceEntry` ends the
    /// trace there, as a bad record does, since the page would otherwise
    /// show nothing: an unknown action (e.g. from a newer bombadil), a
    /// snapshot value a `serde_json::Value` can't read, a violation or a
    /// snapshot of another shape, or nesting at the limit. Nesting is
    /// bounded conservatively, as recorded and as read, so a state the
    /// page could read may be cut too.
    #[test]
    fn ends_the_trace_at_a_state_the_page_cant_read() {
        let good = metadata("https://a/", "jpeg");
        // A snapshot value nested as deep as a state read on its own allows,
        // one level too deep in the array the page reads.
        let deep = |good: &str| {
            let value = format!("{}{}", "[".repeat(124), "]".repeat(124));
            let deep = good.replace(
                r#""snapshots":[]"#,
                &format!(
                    r#""snapshots":[{{"index":0,"name":"x","value":{value},"time":0}}]"#
                ),
            );
            assert!(serde_json::from_str::<BrowserTraceEntry>(&deep).is_ok());
            deep
        };
        for (name, bad) in [
            (
                "unknown action",
                good.replace(r#""action":null"#, r#""action":{"Teleport":1}"#),
            ),
            (
                "RawValue token",
                good.replace(
                    r#""snapshots":[]"#,
                    r#""snapshots":[{"index":0,"name":"x","value":{"$serde_json::private::RawValue":5},"time":0}]"#,
                ),
            ),
            (
                "violation",
                good.replace(r#""violations":[]"#, r#""violations":[{"bogus":1}]"#),
            ),
            ("snapshot", good.replace(r#""snapshots":[]"#, r#""snapshots":[1]"#)),
            ("nesting", deep(&good)),
            // Nested shallowly as recorded, but deeply as read: the
            // RawValue token makes a `serde_json::Value` read its string
            // as JSON, with a nesting limit of its own. The page can read
            // this, but the check bounds the depth of what it reads.
            ("deep as read, through the RawValue token", {
                let inner = format!("{}{}", "[".repeat(100), "]".repeat(100));
                let value = format!(
                    r#"{}{{"$serde_json::private::RawValue":"{inner}"}}{}"#,
                    "[".repeat(100),
                    "]".repeat(100)
                );
                let bad = good.replace(
                    r#""snapshots":[]"#,
                    &format!(
                        r#""snapshots":[{{"index":0,"name":"x","value":{value},"time":0}}]"#
                    ),
                );
                assert!(check_served(&bad).is_ok());
                bad
            }),
            // Nested deeply as recorded, but shallowly as read: a
            // duplicate key keeps its last value only, so only the
            // check of the served bytes sees the depth.
            ("deep as recorded, through a duplicate key", {
                let value = format!(
                    r#"{{"a":{}{},"a":1}}"#,
                    "[".repeat(123),
                    "]".repeat(123)
                );
                let bad = good.replace(
                    r#""snapshots":[]"#,
                    &format!(
                        r#""snapshots":[{{"index":0,"name":"x","value":{value},"time":0}}]"#
                    ),
                );
                let parsed: BrowserTraceEntry =
                    serde_json::from_str(&bad).unwrap();
                assert!(
                    check_served(&serde_json::to_string(&parsed).unwrap())
                        .is_ok()
                );
                bad
            }),
        ] {
            assert_ne!(bad, good, "{name}");
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join(TRACE_FILE_NAME);
            std::fs::write(
                &path,
                trace(&[
                    (good.clone(), Some(b"a")),
                    (bad, Some(b"b")),
                    (good.clone(), Some(b"c")),
                ]),
            )
            .unwrap();
            let state = binary_state(&path);
            block_on(async {
                let response = trace_handler(State(state.clone())).await;
                assert_eq!(response.status(), StatusCode::OK, "{name}");
                let error = response.headers()[TRACE_ERROR_HEADER]
                    .to_str()
                    .unwrap()
                    .to_string();
                assert!(error.starts_with("invalid state 1: "), "{name}: {error}");
                assert!(
                    !response.headers().contains_key(TRACE_UNFINISHED_HEADER),
                    "{name}"
                );
                let served = trace_body(&state).await.unwrap();
                let entries = self::served(&served.body);
                assert_eq!(entries.len(), 1, "{name}");
                assert_eq!(entries[0].state.url, "https://a/", "{name}");
            });
        }

        // A legacy trace, re-serialised: the entry served is checked.
        let directory = tempfile::tempdir().unwrap();
        let json = directory.path().join("trace.jsonl");
        let good = metadata("https://a/", "a.png");
        std::fs::write(&json, format!("{good}\n{}\n{good}\n", deep(&good)))
            .unwrap();
        let state = AppState::new(BrowserTraceFile::Json(json));
        block_on(async {
            let response = trace_handler(State(state.clone())).await;
            let error = response.headers()[TRACE_ERROR_HEADER]
                .to_str()
                .unwrap()
                .to_string();
            assert!(error.starts_with("invalid state 1: "), "{error}");
            let served = trace_body(&state).await.unwrap();
            assert_eq!(self::served(&served.body).len(), 1);
        });
    }

    /// Snapshot indices are `u64`, so the page (wasm32, where `usize` is
    /// 32 bits) reads those above `u32::MAX` as the server does.
    #[test]
    fn serves_snapshot_indices_above_u32() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(TRACE_FILE_NAME);
        let state = metadata("u", "jpeg").replace(
            r#""snapshots":[]"#,
            r#""snapshots":[{"index":4294967296,"name":"x","value":1,"time":0}]"#,
        );
        std::fs::write(&path, trace(&[(state, Some(b"a"))])).unwrap();
        let served = block_on(trace_body(&binary_state(&path))).unwrap();
        assert_eq!(served.error, None);
        let index: u64 = self::served(&served.body)[0].snapshots[0].index;
        assert_eq!(index, 1 << 32);
    }

    /// A body the page couldn't load, above the limit, is cut short at
    /// the last state that fits, as at a bad record. Binary and legacy.
    #[test]
    fn cuts_a_trace_too_large_for_the_page() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(TRACE_FILE_NAME);
        let state = (metadata("u", "jpeg"), Some(&b"a"[..]));
        std::fs::write(&path, trace(&[state.clone(), state.clone(), state]))
            .unwrap();
        let json = directory.path().join("trace.jsonl");
        let line = metadata("u", "a.png");
        std::fs::write(&json, format!("{line}\n{line}\n{line}\n")).unwrap();
        for trace in [
            BrowserTraceFile::Binary(path.clone()),
            BrowserTraceFile::Json(json.clone()),
        ] {
            let trace = Arc::new(trace);
            let whole = AppState {
                trace: trace.clone(),
                ..AppState::new(BrowserTraceFile::Json(json.clone()))
            };
            let length = block_on(trace_body(&whole)).unwrap().body.len();
            for (bytes_max, states, error) in [
                (length, 3, None),
                (
                    length - 1,
                    2,
                    Some(
                        "trace too large for the page; showing the first 2 states",
                    ),
                ),
            ] {
                let state = AppState {
                    trace: trace.clone(),
                    body_bytes_max: bytes_max,
                    ..AppState::new(BrowserTraceFile::Json(json.clone()))
                };
                let response = block_on(trace_handler(State(state.clone())));
                assert_eq!(
                    response
                        .headers()
                        .get(TRACE_ERROR_HEADER)
                        .map(|value| value.to_str().unwrap()),
                    error,
                    "{trace:?}"
                );
                let served = block_on(trace_body(&state)).unwrap();
                assert!(served.body.len() <= bytes_max);
                assert_eq!(
                    self::served(&served.body).len(),
                    states,
                    "{trace:?}"
                );
            }
        }
    }

    /// A large state followed by a small one, with the limit at the
    /// large one: the trace ends there, without the small one after it.
    #[test]
    fn cuts_a_trace_too_large_at_the_first_state_that_doesnt_fit() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(TRACE_FILE_NAME);
        let large_url = format!("https://{}/", "x".repeat(1000));
        std::fs::write(
            &path,
            trace(&[
                (metadata("u", "jpeg"), Some(b"a")),
                (metadata(&large_url, "jpeg"), Some(b"b")),
                (metadata("u", "jpeg"), Some(b"c")),
            ]),
        )
        .unwrap();
        let json = directory.path().join("trace.jsonl");
        let line = metadata("u", "a.png");
        std::fs::write(
            &json,
            format!("{line}\n{}\n{line}\n", metadata(&large_url, "a.png")),
        )
        .unwrap();
        for trace in [
            BrowserTraceFile::Binary(path.clone()),
            BrowserTraceFile::Json(json.clone()),
        ] {
            let binary = matches!(trace, BrowserTraceFile::Binary(_));
            let trace = Arc::new(trace);
            let whole = AppState {
                trace: trace.clone(),
                ..AppState::new(BrowserTraceFile::Json(json.clone()))
            };
            // The whole body is `[s,l,s]`, the large entry `l` longer
            // than the small `s` by the longer URL.
            let length = block_on(trace_body(&whole)).unwrap().body.len();
            let longer = large_url.len() - 1;
            let small = (length - longer - 4) / 3;
            // Room for `[s,s]`, but not `[s,l`.
            let bytes_max = 2 * small + 3;
            let state = AppState {
                trace: trace.clone(),
                body_bytes_max: bytes_max,
                ..AppState::new(BrowserTraceFile::Json(json.clone()))
            };
            let response = block_on(trace_handler(State(state.clone())));
            assert_eq!(
                response.headers()[TRACE_ERROR_HEADER],
                "trace too large for the page; showing the first 1 states",
                "{trace:?}"
            );
            let served = block_on(trace_body(&state)).unwrap();
            let entries = self::served(&served.body);
            assert_eq!(entries.len(), 1, "{trace:?}");
            if binary {
                block_on(async {
                    for url in [
                        "/api/screenshots/1/1.jpeg",
                        "/api/screenshots/1/2.jpeg",
                    ] {
                        assert_eq!(screenshot_at(&state, url).await, None);
                    }
                    assert!(
                        screenshot_at(&state, &entries[0].state.screenshot)
                            .await
                            .is_some()
                    );
                });
            }
        }
    }

    /// Text from the trace is escaped where it is shown, in logs and
    /// headers.
    #[test]
    fn escapes_error_text() {
        assert_eq!(
            escaped("a\x1b[31m\u{202e}\"\\"),
            r#"a\u{1b}[31m\u{202e}\"\\"#
        );
    }

    /// A trace without an end record is served, marked unfinished.
    #[test]
    fn marks_unfinished_traces() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(TRACE_FILE_NAME);
        let mut file = trace(&[(metadata("u", "jpeg"), Some(b"a"))]);
        let finished = file.clone();
        // Without the end record, a header with no payload.
        file.truncate(file.len() - 14);
        std::fs::write(&path, &file).unwrap();
        let state = binary_state(&path);
        block_on(async {
            let response = trace_handler(State(state.clone())).await;
            assert_eq!(response.headers()[TRACE_UNFINISHED_HEADER], "1");
            assert!(!response.headers().contains_key(TRACE_ERROR_HEADER));
            std::fs::write(&path, &finished).unwrap();
            let response = trace_handler(State(state.clone())).await;
            assert!(!response.headers().contains_key(TRACE_UNFINISHED_HEADER));
        });
    }

    /// A scan is stamped with the file it read, not the path's stamp
    /// taken before, so a trace replaced in between isn't taken for the
    /// one stamped.
    #[test]
    fn stamps_a_scan_with_the_file_read() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(TRACE_FILE_NAME);
        let state = (metadata("u", "jpeg"), Some(&b"a"[..]));
        std::fs::write(&path, trace(std::slice::from_ref(&state))).unwrap();
        let a_stamp = FileStamp::of(&std::fs::metadata(&path).unwrap());
        let replacement = directory.path().join("replacement");
        std::fs::write(&replacement, trace(&[state.clone(), state])).unwrap();
        std::fs::rename(&replacement, &path).unwrap();
        let b_stamp = FileStamp::of(&std::fs::metadata(&path).unwrap());
        let scan = scan_trace(&path, a_stamp, 1, PAGE_BODY_BYTES_MAX);
        assert!(scan.result.is_ok());
        assert_eq!(scan.stamp, b_stamp);
        assert!(!a_stamp.appended_to(&scan.stamp));
    }

    /// A trace that can't be read is a server error, retried by the next
    /// trace request but not by every screenshot request.
    #[test]
    fn remembers_a_failure_for_screenshots() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(TRACE_FILE_NAME);
        std::fs::write(&path, b"not a trace").unwrap();
        let state = binary_state(&path);
        block_on(async {
            for _ in 0..3 {
                assert!(
                    binary_screenshot(&state, &path, "1", "0.jpeg")
                        .await
                        .is_err()
                );
            }
            assert_eq!(state.scans.load(Ordering::Relaxed), 1);
            let response = trace_handler(State(state.clone())).await;
            assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
            assert_eq!(state.scans.load(Ordering::Relaxed), 2);
        });
        // A trace that can't be opened is remembered just the same.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let path = directory.path().join("unreadable");
            std::fs::write(&path, b"not a trace").unwrap();
            std::fs::set_permissions(
                &path,
                std::fs::Permissions::from_mode(0o000),
            )
            .unwrap();
            // As root, permissions don't keep it from being opened.
            if File::open(&path).is_err() {
                let state = binary_state(&path);
                block_on(async {
                    for _ in 0..3 {
                        assert!(
                            binary_screenshot(&state, &path, "1", "0.jpeg")
                                .await
                                .is_err()
                        );
                    }
                    assert_eq!(state.scans.load(Ordering::Relaxed), 1);
                    let response = trace_handler(State(state.clone())).await;
                    assert_eq!(
                        response.status(),
                        StatusCode::INTERNAL_SERVER_ERROR
                    );
                    assert_eq!(state.scans.load(Ordering::Relaxed), 2);
                });
            }
        }
    }

    /// Reloading reads a trace that grew since, as one still being
    /// written does.
    #[test]
    fn reloading_reads_a_trace_that_grew() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(TRACE_FILE_NAME);
        let one = (metadata("u", "jpeg"), Some(&b"a"[..]));
        std::fs::write(&path, trace(std::slice::from_ref(&one))).unwrap();
        let state = binary_state(&path);
        block_on(async {
            assert_eq!(screenshot_urls(&state).await.len(), 1);
            // In place, so only its length and time change.
            std::fs::write(&path, trace(&[one.clone(), one])).unwrap();
            assert_eq!(
                screenshot_urls(&state).await,
                ["/api/screenshots/2/0.jpeg", "/api/screenshots/2/1.jpeg"]
            );
        });
    }

    /// A trace replaced while served (as by a run with
    /// `--output-path-overwrite`) is read again: the old trace's URLs are
    /// not found, rather than served from the new trace, and the new
    /// trace's URLs serve its screenshots.
    #[test]
    fn reads_a_replaced_trace_again() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(TRACE_FILE_NAME);
        std::fs::write(
            &path,
            trace(&[(metadata("u", "jpeg"), Some(b"old image"))]),
        )
        .unwrap();
        let state = binary_state(&path);
        block_on(async {
            let old = screenshot_urls(&state).await.remove(0);
            assert_eq!(
                screenshot_at(&state, &old).await,
                Some(("image/jpeg", b"old image".to_vec()))
            );
            // Replaced by renaming, as the trace writer does, with a trace
            // whose screenshot is at the same offset.
            let replacement = directory.path().join("replacement");
            std::fs::write(
                &replacement,
                trace(&[(metadata("u", "jpeg"), Some(b"new image"))]),
            )
            .unwrap();
            std::fs::rename(&replacement, &path).unwrap();
            assert_eq!(screenshot_at(&state, &old).await, None);
            let new = screenshot_urls(&state).await.remove(0);
            assert_ne!(new, old);
            assert_eq!(
                screenshot_at(&state, &new).await,
                Some(("image/jpeg", b"new image".to_vec()))
            );
        });
        assert_eq!(state.scans.load(Ordering::Relaxed), 2);
    }

    /// A state whose screenshot is shared with an earlier state's (see
    /// `bombadil_trace::tag::STATE_PAYLOAD_REF`) is served those bytes.
    #[test]
    fn serves_shared_screenshots() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(TRACE_FILE_NAME);
        let a = Arc::new(b"page a, longer than a reference".to_vec());
        let b = Arc::new(b"page b, longer than a reference".to_vec());
        let screenshots = [&a, &b, &a, &b, &a];
        let mut writer = bombadil_trace::TraceFileWriter::create(
            directory.path(),
            TraceDriver::Browser,
            false,
        )
        .unwrap();
        for screenshot in screenshots {
            let metadata: Box<RawValue> =
                RawValue::from_string(metadata("u", "png")).unwrap();
            writer
                .write_state(metadata, Some(screenshot.clone()))
                .unwrap();
        }
        writer.finish().unwrap();
        // Written once each.
        let from: Vec<_> = bombadil_trace::open_trace::<Box<RawValue>>(&path)
            .unwrap()
            .map(|state| state.unwrap().payload_from)
            .collect();
        assert_eq!(from, [Some(0), Some(1), Some(0), Some(1), Some(0)]);

        let app = binary_state(&path);
        block_on(async {
            let urls = screenshot_urls(&app).await;
            assert_eq!(urls.len(), screenshots.len());
            for (url, screenshot) in urls.iter().zip(screenshots) {
                assert_eq!(
                    screenshot_at(&app, url).await,
                    Some(("image/png", screenshot.to_vec())),
                    "{url}"
                );
            }
        });
    }

    /// A trace still being written (appended to in place) keeps serving
    /// screenshots under the URLs the page has, without a scan per
    /// request; reloading the trace reads its new states.
    #[test]
    fn serves_screenshots_of_a_trace_being_written() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(TRACE_FILE_NAME);
        // Without its end record, as while it is written.
        let unfinished = |states: &[(String, Option<&[u8]>)]| {
            let mut file = trace(states);
            file.truncate(file.len() - 14);
            file
        };
        let first = (metadata("u", "jpeg"), Some(&b"first"[..]));
        let second = (metadata("u", "jpeg"), Some(&b"second"[..]));
        std::fs::write(&path, unfinished(std::slice::from_ref(&first)))
            .unwrap();
        let app = binary_state(&path);
        block_on(async {
            let urls = screenshot_urls(&app).await;
            assert_eq!(urls, ["/api/screenshots/1/0.jpeg"]);
            // Appended to: the same file, longer.
            let mut file = std::fs::OpenOptions::new()
                .append(true)
                .open(&path)
                .unwrap();
            let grown = unfinished(&[first.clone(), second.clone()]);
            let length = std::fs::metadata(&path).unwrap().len() as usize;
            std::io::Write::write_all(&mut file, &grown[length..]).unwrap();
            drop(file);
            for _ in 0..3 {
                assert_eq!(
                    screenshot_at(&app, &urls[0]).await,
                    Some(("image/jpeg", b"first".to_vec()))
                );
            }
            assert_eq!(app.scans.load(Ordering::Relaxed), 1);
            assert_eq!(
                screenshot_urls(&app).await,
                ["/api/screenshots/2/0.jpeg", "/api/screenshots/2/1.jpeg"]
            );
            assert_eq!(
                screenshot_at(&app, "/api/screenshots/2/1.jpeg").await,
                Some(("image/jpeg", b"second".to_vec()))
            );
        });
        assert_eq!(app.scans.load(Ordering::Relaxed), 2);
    }

    /// A trace deleted and created again at its path, longer, while
    /// served: the old trace's URLs are not found, never served from the
    /// new trace, as the scan holds the old file open, so the new one
    /// can't have its inode.
    #[test]
    fn reads_a_recreated_trace_again() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(TRACE_FILE_NAME);
        let old = (metadata("u", "jpeg"), Some(&b"old image"[..]));
        std::fs::write(&path, trace(std::slice::from_ref(&old))).unwrap();
        let state = binary_state(&path);
        block_on(async {
            let urls = screenshot_urls(&state).await;
            std::fs::remove_file(&path).unwrap();
            let new = (metadata("u", "jpeg"), Some(&b"NEW image"[..]));
            std::fs::write(&path, trace(&[new.clone(), new])).unwrap();
            assert_eq!(screenshot_at(&state, &urls[0]).await, None);
            let urls = screenshot_urls(&state).await;
            assert_eq!(
                screenshot_at(&state, &urls[0]).await,
                Some(("image/jpeg", b"NEW image".to_vec()))
            );
        });
    }

    /// One page's reload of a trace being written doesn't break another
    /// page's screenshots: the URLs of a scan stay served while the trace
    /// is only appended to, even past the states that scan had; a
    /// replacement ends them.
    #[test]
    fn keeps_serving_earlier_urls_of_a_trace_being_written() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(TRACE_FILE_NAME);
        let unfinished = |states: &[(String, Option<&[u8]>)]| {
            let mut file = trace(states);
            file.truncate(file.len() - 14);
            file
        };
        let first = (metadata("u", "jpeg"), Some(&b"first"[..]));
        let second = (metadata("u", "jpeg"), Some(&b"second"[..]));
        std::fs::write(&path, unfinished(std::slice::from_ref(&first)))
            .unwrap();
        let app = binary_state(&path);
        block_on(async {
            // One page.
            let urls = screenshot_urls(&app).await;
            let grown = unfinished(&[first.clone(), second.clone()]);
            let length = std::fs::metadata(&path).unwrap().len() as usize;
            let mut file = std::fs::OpenOptions::new()
                .append(true)
                .open(&path)
                .unwrap();
            std::io::Write::write_all(&mut file, &grown[length..]).unwrap();
            drop(file);
            // Another page, or a reload.
            let reloaded = screenshot_urls(&app).await;
            assert_eq!(app.scans.load(Ordering::Relaxed), 2);
            assert_eq!(reloaded[0], "/api/screenshots/2/0.jpeg");
            assert_eq!(
                screenshot_at(&app, &urls[0]).await,
                Some(("image/jpeg", b"first".to_vec()))
            );
            // Past the states the first scan had: the same state.
            assert_eq!(
                screenshot_at(&app, "/api/screenshots/1/1.jpeg").await,
                Some(("image/jpeg", b"second".to_vec()))
            );
            // Appended to again and reloaded: the first scan's URLs are
            // still served, carried over two scans.
            let third = (metadata("u", "jpeg"), Some(&b"third"[..]));
            let grown_again =
                unfinished(&[first.clone(), second.clone(), third]);
            let mut file = std::fs::OpenOptions::new()
                .append(true)
                .open(&path)
                .unwrap();
            std::io::Write::write_all(&mut file, &grown_again[grown.len()..])
                .unwrap();
            drop(file);
            let reloaded_again = screenshot_urls(&app).await;
            assert_eq!(app.scans.load(Ordering::Relaxed), 3);
            assert_eq!(reloaded_again[2], "/api/screenshots/3/2.jpeg");
            assert_eq!(
                screenshot_at(&app, &urls[0]).await,
                Some(("image/jpeg", b"first".to_vec()))
            );
            // Replaced by renaming: neither scan's URLs are served.
            let replacement = directory.path().join("replacement");
            std::fs::write(&replacement, trace(&[first.clone(), second]))
                .unwrap();
            std::fs::rename(&replacement, &path).unwrap();
            let replaced = screenshot_urls(&app).await;
            for url in [&urls[0], &reloaded[0], &reloaded[1]] {
                assert_eq!(screenshot_at(&app, url).await, None, "{url}");
            }
            assert_eq!(
                screenshot_at(&app, &replaced[0]).await,
                Some(("image/jpeg", b"first".to_vec()))
            );
        });
    }

    /// A trace cut short in place (same file, shorter) is read again for
    /// a screenshot, rather than read at offsets past its end.
    #[test]
    fn reads_a_truncated_trace_again() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(TRACE_FILE_NAME);
        let state = (metadata("u", "jpeg"), Some(&b"image"[..]));
        std::fs::write(&path, trace(&[state.clone(), state.clone()])).unwrap();
        let app = binary_state(&path);
        block_on(async {
            assert!(
                screenshot_at(&app, "/api/screenshots/1/1.jpeg")
                    .await
                    .is_some()
            );
            std::fs::write(&path, trace(&[state])).unwrap();
            assert_eq!(
                screenshot_at(&app, "/api/screenshots/1/1.jpeg").await,
                None
            );
        });
        assert_eq!(app.scans.load(Ordering::Relaxed), 2);
    }
}
