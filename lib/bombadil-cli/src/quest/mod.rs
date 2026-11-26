//! The Quest service: it learns from uploaded runs and serves the action
//! weights that steer later ones towards a goal.

use std::{
    fmt::Write as _,
    fs::File,
    io::{self, BufWriter, IsTerminal as _},
    path::{Path, PathBuf},
    str::FromStr,
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::Duration,
};

use anyhow::{Context, Result, anyhow, bail};
use clap::Args;
use reqwest::{
    StatusCode,
    blocking::{Client, RequestBuilder, Response, multipart},
    header::{HeaderMap, HeaderValue},
};
use serde::Deserialize;
use tempfile::TempDir;
use url::Url;

use crate::trace;

#[derive(Args)]
pub struct QuestOptions {
    /// The app of the Quest service to work with: `--goal` takes its
    /// weights from it and `--quest-upload` adds runs to it. Implies
    /// `--coverage-components`.
    #[arg(long, value_name = "ORG/APP", requires = "quest_url")]
    pub quest_app: Option<App>,

    /// Upload the recorded runs to the app when testing ends, with the
    /// tables of coverage blocks and action templates, and have Quest train
    /// on them. Quest creates the app if it does not exist. The tables go
    /// to temporary files unless their options name others. Without this,
    /// the app is only read.
    #[arg(long, requires = "quest_app")]
    pub quest_upload: bool,

    /// Base URL of the Quest service.
    #[arg(long, value_name = "URL", requires = "quest_app")]
    pub quest_url: Option<Url>,

    /// Token for a Quest service behind the exe.dev proxy.
    #[arg(
        long,
        value_name = "TOKEN",
        env = "BOMBADIL_QUEST_AUTH_TOKEN",
        hide_env_values = true,
        requires = "quest_app"
    )]
    pub quest_auth_token: Option<String>,
}

/// An app of a Quest organisation, written `org/app`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct App {
    org: String,
    name: String,
}

impl FromStr for App {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        // What Quest accepts. Anything else must not reach a URL path.
        let is_slug = |slug: &str| {
            (1..=64).contains(&slug.len())
                && slug.bytes().all(|byte| {
                    byte.is_ascii_lowercase()
                        || byte.is_ascii_digit()
                        || byte == b'-'
                })
        };
        match text.split_once('/') {
            Some((org, name)) if is_slug(org) && is_slug(name) => Ok(App {
                org: org.to_string(),
                name: name.to_string(),
            }),
            _ => Err(format!(
                "`{text}` is not ORG/APP, each 1 to 64 lowercase letters, digits and hyphens"
            )),
        }
    }
}

const TIMEOUT: Duration = Duration::from_secs(30);
const TIMEOUT_UPLOAD: Duration = Duration::from_secs(15 * 60);
const TRAIN_POLL_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Deserialize)]
struct ErrorBody {
    error: String,
}

#[derive(Deserialize)]
struct Accepted {
    /// The path of the job to poll.
    job: String,
}

#[derive(Deserialize)]
struct Job {
    status: String,
    error: Option<String>,
    result: Option<JobResult>,
}

#[derive(Default, Deserialize)]
struct JobResult {
    version: Option<i64>,
    /// What Quest remarks on the model, such as a part it could not build.
    #[serde(default)]
    notes: Vec<String>,
}

/// What Quest answered to an upload that it took.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Uploaded {
    New,
    Known,
}

enum Trained {
    Model(JobResult),
    Failed(String),
    Declined(String),
    Interrupted,
}

struct Quest {
    client: Client,
    base_url: Url,
    app: App,
}

impl Quest {
    fn new(base_url: &Url, app: App, token: Option<&str>) -> Result<Self> {
        let mut headers = HeaderMap::new();
        if let Some(token) = token {
            let mut value = HeaderValue::from_str(&format!("Bearer {token}"))
                .context("the Quest token is not a header value")?;
            value.set_sensitive(true);
            headers.insert("X-Exedev-Authorization", value);
        }
        let client = Client::builder()
            .default_headers(headers)
            .timeout(TIMEOUT)
            .build()?;
        // A base without a trailing slash would lose its last segment
        // when joined.
        let mut base_url = base_url.clone();
        if !base_url.path().ends_with('/') {
            base_url.set_path(&format!("{}/", base_url.path()));
        }
        Ok(Quest {
            client,
            base_url,
            app,
        })
    }

    fn org_url(&self, rest: &str) -> Result<Url> {
        let path = format!("api/v1/orgs/{}/{rest}", self.app.org);
        Ok(self.base_url.join(&path)?)
    }

    fn app_url(&self, rest: &str) -> Result<Url> {
        self.org_url(&format!("apps/{}/{rest}", self.app.name))
    }

    fn send(&self, request: RequestBuilder) -> Result<Response> {
        request.send().with_context(|| {
            format!("no answer from Quest at {}", self.base_url)
        })
    }

    fn create_app(&self) -> Result<()> {
        let request = self
            .client
            .post(self.org_url("apps")?)
            .json(&serde_json::json!({ "name": self.app.name }));
        let response = self.send(request)?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(refusal(response))
        }
    }

    /// The weights file for `goals`, as Quest serves it.
    fn weights(&self, goals: &[String]) -> Result<String> {
        let mut url = self.app_url("weights")?;
        url.query_pairs_mut()
            .extend_pairs(goals.iter().map(|goal| ("goal", goal)));
        let response = self.send(self.client.get(url))?;
        if response.status().is_success() {
            Ok(response.text()?)
        } else {
            Err(refusal(response))
        }
    }

    fn upload(
        &self,
        trace: &Path,
        tables: &Tables,
        metadata: &str,
    ) -> Result<Uploaded> {
        let mut form = multipart::Form::new()
            .file("trace", trace)?
            .text("metadata", metadata.to_string());
        for (name, table) in
            [("blocks", &tables.blocks), ("templates", &tables.templates)]
        {
            form = form.file(name, table).with_context(|| {
                format!("reading the table {}", table.display())
            })?;
        }
        let request = self
            .client
            .post(self.app_url("runs")?)
            .timeout(TIMEOUT_UPLOAD)
            .multipart(form);
        let response = self.send(request)?;
        match response.status() {
            StatusCode::OK => Ok(Uploaded::Known),
            StatusCode::ACCEPTED => Ok(Uploaded::New),
            _ => Err(refusal(response)),
        }
    }

    /// Queues a train job and waits for it, until `interrupted`.
    fn train(&self, interrupted: &AtomicBool) -> Result<Trained> {
        let response = self.send(self.client.post(self.app_url("train")?))?;
        if response.status() == StatusCode::CONFLICT {
            return Ok(Trained::Declined(refusal(response).to_string()));
        }
        if !response.status().is_success() {
            return Err(refusal(response));
        }
        let accepted: Accepted = response.json()?;
        let job_url =
            self.base_url.join(accepted.job.trim_start_matches('/'))?;
        while !interrupted.load(Ordering::SeqCst) {
            let response = self.send(self.client.get(job_url.clone()))?;
            if !response.status().is_success() {
                return Err(refusal(response));
            }
            let job: Job = response.json()?;
            match job.status.as_str() {
                "done" => {
                    return Ok(Trained::Model(job.result.unwrap_or_default()));
                }
                "failed" => {
                    return Ok(Trained::Failed(
                        job.error.unwrap_or_else(|| "no reason given".into()),
                    ));
                }
                _ => thread::sleep(TRAIN_POLL_INTERVAL),
            }
        }
        Ok(Trained::Interrupted)
    }
}

/// Why Quest refused a request: its own words when it gave any.
fn refusal(response: Response) -> anyhow::Error {
    let status = response.status();
    let reason = response
        .text()
        .ok()
        .and_then(|text| serde_json::from_str::<ErrorBody>(&text).ok());
    match reason {
        Some(ErrorBody { error }) => anyhow!("{error} ({status})"),
        None if matches!(
            status,
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
        ) =>
        {
            anyhow!("{status}: is --quest-auth-token missing or expired?")
        }
        None => anyhow!("{status}"),
    }
}

/// The tables uploaded with every run.
struct Tables {
    blocks: PathBuf,
    templates: PathBuf,
    /// Holds the tables the command was not asked to write itself.
    _directory: Option<TempDir>,
}

/// What a weights file says of itself, for the metadata of the runs it
/// steers: Quest then reports how they did against it.
fn steering(weights: &str) -> Result<serde_json::Value> {
    #[derive(Deserialize)]
    struct Weights {
        goal: String,
        params: Params,
    }
    #[derive(Deserialize)]
    struct Params {
        model_version: i64,
        query: serde_json::Value,
    }
    let Weights { goal, params } = serde_json::from_str(weights)
        .context("the weights Quest served do not say what they steer to")?;
    Ok(serde_json::json!({
        "goal": goal,
        "model_version": params.model_version,
        "query": params.query,
    }))
}

/// A command's dealings with Quest: weights before its runs, upload and
/// training after them.
pub struct Session {
    quest: Quest,
    weights: Option<String>,
    /// `None` when the runs are not uploaded.
    upload: Option<Upload>,
}

/// What every uploaded run is sent with.
struct Upload {
    tables: Tables,
    metadata: String,
}

impl Session {
    /// `None` when the command neither steers nor uploads. Asks Quest for
    /// the weights of `goals`, if any. Quest's models are keyed by
    /// per-function coverage, so this turns on coverage components, of
    /// every script unless the command names some. With `--quest-upload` it
    /// also names a temporary file for each table the command was not asked
    /// to write.
    pub fn start(
        options: &QuestOptions,
        goals: &[String],
        coverage_components: &mut Option<Vec<String>>,
        blocks_path: &mut Option<PathBuf>,
        templates_path: &mut Option<PathBuf>,
    ) -> Result<Option<Self>> {
        let (Some(app), Some(base_url)) =
            (&options.quest_app, &options.quest_url)
        else {
            return Ok(None);
        };
        if goals.is_empty() && !options.quest_upload {
            return Ok(None);
        }
        let quest = Quest::new(
            base_url,
            app.clone(),
            options.quest_auth_token.as_deref(),
        )?;
        if options.quest_upload {
            quest.create_app().with_context(|| {
                format!("Quest app {}/{}", app.org, app.name)
            })?;
        }

        let weights = if goals.is_empty() {
            None
        } else {
            Some(quest.weights(goals).with_context(|| {
                format!("Quest serves no weights for {}", goals.join(", "))
            })?)
        };
        let mut metadata = match &weights {
            Some(weights) => steering(weights)?,
            None => serde_json::json!({ "goal": null }),
        };
        metadata["coverage_components"] =
            serde_json::json!(coverage_components.get_or_insert_default());
        if !options.quest_upload {
            return Ok(Some(Session {
                quest,
                weights,
                upload: None,
            }));
        }

        let directory = if blocks_path.is_none() || templates_path.is_none() {
            Some(
                tempfile::Builder::new()
                    .prefix("bombadil-quest-")
                    .tempdir()?,
            )
        } else {
            None
        };
        let table = |path: &mut Option<PathBuf>, name: &str| {
            path.get_or_insert_with(|| {
                directory
                    .as_ref()
                    .expect("a table without a path has the directory")
                    .path()
                    .join(name)
            })
            .clone()
        };
        let tables = Tables {
            blocks: table(blocks_path, "blocks.jsonl"),
            templates: table(templates_path, "templates.jsonl"),
            _directory: directory,
        };
        Ok(Some(Session {
            quest,
            weights,
            upload: Some(Upload {
                tables,
                metadata: metadata.to_string(),
            }),
        }))
    }

    /// The weights file Quest served for the goals, to steer the runs by.
    pub fn weights(&self) -> Option<&str> {
        self.weights.as_deref()
    }

    /// With `--quest-upload`, uploads the runs and has Quest train. Says
    /// what happened: the command's result is that of its tests, whatever
    /// Quest's.
    /// `interrupted` is the command's SIGINT flag, which also stops this.
    pub fn finish(
        self,
        traces: impl IntoIterator<Item = PathBuf>,
        interrupted: &AtomicBool,
    ) {
        let Some(upload) = &self.upload else {
            return;
        };
        // The campaign may have been stopped by the signal.
        interrupted.store(false, Ordering::SeqCst);
        let App { org, name } = &self.quest.app;
        match self.upload_and_train(upload, traces, interrupted) {
            Ok(summary) => println!("Quest ({org}/{name}): {summary}\n"),
            Err(error) => {
                eprintln!("Quest ({org}/{name}) failed: {error:#}\n")
            }
        }
    }

    fn upload_and_train(
        &self,
        upload: &Upload,
        traces: impl IntoIterator<Item = PathBuf>,
        interrupted: &AtomicBool,
    ) -> Result<String> {
        let export = tempfile::Builder::new()
            .prefix("bombadil-quest-")
            .suffix(".jsonl.zst")
            .tempfile()?;
        let (mut new_count, mut known_count) = (0u64, 0u64);
        for trace in traces {
            if interrupted.load(Ordering::SeqCst) {
                bail!(
                    "interrupted after uploading {} runs",
                    new_count + known_count
                );
            }
            export_trace(&trace, export.path())
                .with_context(|| format!("exporting {}", trace.display()))?;
            let uploaded = self
                .quest
                .upload(export.path(), &upload.tables, &upload.metadata)
                .with_context(|| format!("uploading {}", trace.display()))?;
            match uploaded {
                Uploaded::New => new_count += 1,
                Uploaded::Known => known_count += 1,
            }
        }
        let uploaded = match (new_count, known_count) {
            (0, 0) => bail!("there are no runs to upload"),
            (1, 0) => "uploaded 1 run".to_string(),
            (new, 0) => format!("uploaded {new} runs"),
            (new, known) => {
                format!("uploaded {new} runs, {known} more were there already")
            }
        };
        println!("Quest: {uploaded}, training...");
        Ok(match self.quest.train(interrupted)? {
            Trained::Model(JobResult { version, notes }) => {
                let mut summary = match version {
                    Some(version) => {
                        format!("{uploaded} and trained model {version}")
                    }
                    None => format!("{uploaded} and trained"),
                };
                // Yellow, when a person is reading.
                let (start, end) = if io::stdout().is_terminal()
                    && std::env::var_os("NO_COLOR").is_none()
                {
                    ("\x1b[33m", "\x1b[0m")
                } else {
                    ("", "")
                };
                for note in notes {
                    let note = trace::printable(&note);
                    write!(summary, "\n  {start}note: {note}{end}")?;
                }
                summary
            }
            Trained::Failed(reason) => {
                bail!("{uploaded}, but training failed: {reason}")
            }
            Trained::Declined(reason) => {
                format!("{uploaded}, not trained: {reason}")
            }
            Trained::Interrupted => {
                format!("{uploaded}, training goes on in Quest")
            }
        })
    }
}

/// Writes the trace as Quest reads it: one JSON object per state, without
/// the snapshots the specification excludes, compressed.
fn export_trace(trace: &Path, export: &Path) -> Result<()> {
    let file = BufWriter::new(File::create(export)?);
    let mut encoder = zstd::stream::write::Encoder::new(file, 0)?;
    trace::write_json_without_excluded(trace, &mut encoder)?;
    let mut file = encoder.finish()?;
    io::Write::flush(&mut file)?;
    Ok(())
}
