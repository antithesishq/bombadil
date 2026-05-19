use std::collections::HashMap;
use std::process::Stdio;

use anyhow::{Context, Result, anyhow, bail};
use bombadil::specification::domain::Snapshot;
use bombadil::tree::Tree;
use bombadil_browser::browser::{actions::BrowserAction, state::BrowserState};
use serde_json as json;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

/// State carried by the [`TestMode::ModelDriven`](crate::strategy::TestMode::ModelDriven)
/// mode. Holds the lazily-spawned Claude session and the last observed browser state so
/// that the user prompt and conversation history live in the model instead of being
/// re-sent each turn.
pub struct ModelDrivenState {
    pub user_prompt: String,
    pub claude_model: String,
    pub last_state: Option<StateSummary>,
    /// Snapshots from the round before the one in `last_state`. Used by
    /// `build_prompt` to render only the snapshots whose values changed.
    pub previous_snapshots: HashMap<String, json::Value>,
    /// Carried into the next consultation when the previous rollout was rejected so the
    /// model can adjust. Driver-level apply failures are reported here when we observe
    /// them; today the runner bails on those, so this only covers rollouts invalidated
    /// by the new state's available action set.
    pub pending_feedback: Option<String>,
    /// Lazily spawned on the first consultation; reused for every subsequent turn.
    pub client: Option<ModelClient>,
}

impl ModelDrivenState {
    pub fn new(user_prompt: String, claude_model: String) -> Self {
        Self {
            user_prompt,
            claude_model,
            last_state: None,
            previous_snapshots: HashMap::new(),
            pending_feedback: None,
            client: None,
        }
    }

    pub fn record_state(
        &mut self,
        state: &BrowserState,
        snapshots: &[Snapshot],
    ) {
        if let Some(previous) = self.last_state.take() {
            self.previous_snapshots = previous.snapshots.into_iter().collect();
        }
        self.last_state = Some(StateSummary::from_browser(state, snapshots));
    }
}

pub struct StateSummary {
    pub url: String,
    pub title: String,
    pub snapshots: Vec<(String, json::Value)>,
}

impl StateSummary {
    fn from_browser(state: &BrowserState, snapshots: &[Snapshot]) -> Self {
        Self {
            url: state.url.to_string(),
            title: state.title.clone(),
            snapshots: snapshots
                .iter()
                .map(|snapshot| {
                    (
                        snapshot
                            .name
                            .clone()
                            .unwrap_or_else(|| format!("#{}", snapshot.index)),
                        snapshot.value.clone(),
                    )
                })
                .collect(),
        }
    }

    fn render(
        &self,
        previous_snapshots: &HashMap<String, json::Value>,
    ) -> String {
        let mut out = String::new();
        out.push_str(&format!("URL: {}\n", self.url));
        out.push_str(&format!("Title: {}\n", self.title));
        let changed: Vec<_> = self
            .snapshots
            .iter()
            .filter(|(name, value)| previous_snapshots.get(name) != Some(value))
            .collect();
        if !changed.is_empty() {
            out.push_str("Snapshots (changed since last turn):\n");
            for (name, value) in changed {
                out.push_str(&format!("  {name}: {value}\n"));
            }
        }
        out
    }
}

/// Ask the model for a fresh rollout for the current state. The caller is
/// responsible for reconciling the first action against the live action list,
/// queueing the rest, and handling the empty-rollout (`Ok(None)`) handover
/// signal. Returns `Ok(None)` when the model returned an empty array.
pub async fn consult_model(
    state: &mut ModelDrivenState,
    tree: &Tree<BrowserAction>,
) -> Result<Option<Vec<BrowserAction>>> {
    let available_actions = tree.values();
    if available_actions.is_empty() {
        bail!("no actions available to offer the model");
    }

    if state.client.is_none() {
        let system_prompt = build_system_prompt(&state.user_prompt);
        state.client = Some(
            ModelClient::launch(&state.claude_model, &system_prompt)
                .await
                .context("launching claude streaming client")?,
        );
    }
    let prompt = build_prompt(state, &available_actions);

    let result_text = state
        .client
        .as_mut()
        .expect("client just initialized")
        .send(&prompt)
        .await
        .context("sending prompt to claude streaming client")?;

    let rollout = parse_rollout(&result_text, &available_actions).with_context(|| {
        format!(
            "parsing model response as JSON array of actions or indices: {:?}",
            result_text
        )
    })?;

    state.pending_feedback = None;
    if rollout.is_empty() {
        log::info!(
            "model returned an empty rollout; handing over to random walk"
        );
        return Ok(None);
    }
    Ok(Some(rollout))
}

fn build_system_prompt(user_prompt: &str) -> String {
    let mut prompt = String::from(
        "You are driving a property-based test of a web application via the \
         Bombadil testing tool. On each turn you will be shown the current browser \
         state and the list of actions available to take next. Your job is to \
         produce an action rollout that explores the application in line with the \
         goals below.\n\n",
    );
    prompt.push_str("--- User goals ---\n");
    prompt.push_str(user_prompt.trim());
    prompt.push_str("\n\n");
    prompt.push_str(
        "On every turn, respond with ONLY a JSON array (NO prose, no \
         markdown fences, no comments). Each element may be either:\n\
         - an INTEGER, treated as an index into the 'Available actions' \
           list below (cheap and easy; perfect for the first step).\n\
         - a full BrowserAction OBJECT (needed for speculative actions \
           whose exact shape doesn't appear in the current list, e.g. \
           typing text that hasn't been suggested yet).\n\n\
         You can mix both forms freely in one array.\n\n\
         When you have accomplished the goals above and have nothing \
         meaningful left to drive, return an EMPTY array (`[]`). That \
         hands the rest of the test over to random exploration - the run \
         keeps going (so the existing properties keep getting checked) \
         but you are not consulted again. Use this whenever further \
         planning would be unproductive; do not return `[]` just because \
         you are unsure.\n\n\
         Action shapes use the serde representation of `BrowserAction`. \
         The 'Available actions' section below shows real examples for the \
         current state. Allowed shapes:\n\n\
         {\"Click\":{\"name\":\"<elem>\",\"content\":\"<text or null>\",\
         \"point\":{\"x\":<num>,\"y\":<num>}}}\n\
         {\"DoubleClick\":{\"name\":\"<elem>\",\"content\":\"<text or null>\",\
         \"point\":{\"x\":<num>,\"y\":<num>},\"delay_millis\":<num>}}\n\
         {\"TypeText\":{\"text\":\"<your text>\",\"delay_millis\":<num>}}\n\
         {\"PressKey\":{\"code\":<integer keycode>}}\n\
         {\"ScrollUp\":{\"origin\":{\"x\":<num>,\"y\":<num>},\"distance\":<num>}}\n\
         {\"ScrollDown\":{\"origin\":{\"x\":<num>,\"y\":<num>},\"distance\":<num>}}\n\
         \"Back\" / \"Forward\" / \"Reload\" / \"Wait\"\n\n\
         How later actions are reconciled. After each action executes, \
         the action list is regenerated. Each queued action then gets \
         matched against the new list:\n\
         - Click / DoubleClick: matched by exact (name, content). The \
           point you provide is a fuzzy hint - reconciliation accepts \
           anything within ~500px of the actual element, and overrides \
           the point with live coordinates. So for speculative clicks on \
           elements you expect to appear, copy a plausible point from a \
           similar element in the current list (or just reuse any \
           nearby coordinate). Wrong (name, content) - or an element \
           that never materializes - causes the rollout to be discarded \
           from that step onward; the runner then re-consults you for \
           free with the new state, so optimistic planning is safe.\n\
         - TypeText / PressKey: invent text and key codes freely. They \
           reconcile as long as the action type is still legal in the new \
           state (i.e. some input is focused / the page accepts keyboard).\n\
         - Scroll / Back / Forward / Reload / Wait: trivial type match.\n\n\
         IMPORTANT: each consultation costs several seconds of wall-clock \
         latency. Default to LONG rollouts (10-30 actions) and only return \
         a short rollout when the next step's choice genuinely depends on \
         observing the previous outcome. For predictable UI flows - \
         typing several words separated by Enter, filling a known form, \
         repeating an idempotent action - chain as many steps as you \
         reasonably can. The runner re-consults you for free when the \
         world drifts from your prediction, so over-planning is safer \
         than under-planning.",
    );
    prompt
}

fn build_prompt(
    state: &ModelDrivenState,
    available_actions: &[BrowserAction],
) -> String {
    let mut prompt = String::new();

    if let Some(feedback) = &state.pending_feedback {
        prompt.push_str("--- Previous rollout was rejected ---\n");
        prompt.push_str(feedback);
        prompt.push_str("\n\n");
    }

    prompt.push_str("--- Current state ---\n");
    if let Some(summary) = &state.last_state {
        prompt.push_str(&summary.render(&state.previous_snapshots));
    } else {
        prompt.push_str("(no state observed yet)\n");
    }
    prompt.push('\n');

    prompt.push_str("--- Available actions ---\n");
    for (index, action) in available_actions.iter().enumerate() {
        let action_json = json::to_string(action).unwrap_or_else(|_| {
            format!("<unserializable action: {:?}>", action)
        });
        prompt.push_str(&format!("{index}: {action_json}\n"));
    }
    prompt.push('\n');

    prompt.push_str("Return one or more action indices as a JSON array (e.g. [2] or [2, 0, 5]).");
    prompt
}

/// One long-running `claude -p` subprocess in streaming-JSON mode. We send the
/// per-turn prompt as a user message on stdin and read events from stdout until
/// the `result` event for that turn arrives. This avoids re-spawning the CLI
/// (and re-resuming the session from disk) on every consultation, which was the
/// dominant source of latency in the one-shot implementation.
pub struct ModelClient {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl ModelClient {
    pub async fn launch(model: &str, system_prompt: &str) -> Result<Self> {
        let mut command = Command::new("claude");
        command
            .arg("-p")
            .arg("--model")
            .arg(model)
            .arg("--system-prompt")
            .arg(system_prompt)
            .arg("--effort")
            .arg("low")
            .arg("--input-format")
            .arg("stream-json")
            .arg("--output-format")
            .arg("stream-json")
            .arg("--verbose")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true);

        log::debug!("launching persistent claude CLI (model={model})");
        let mut child = command
            .spawn()
            .context("failed to spawn `claude` (is the CLI on PATH?)")?;
        let stdin = child
            .stdin
            .take()
            .context("claude child has no stdin handle")?;
        let stdout = child
            .stdout
            .take()
            .context("claude child has no stdout handle")?;
        let stdout = BufReader::new(stdout);

        // Don't wait for any pre-input event. In stream-json mode some
        // versions of the CLI don't emit a system/init event until they
        // see input, so blocking the launch on `read_line` deadlocks the
        // process (we have nothing to send yet, claude has nothing to say).
        // The session-init event, if any, gets consumed and logged by the
        // first `send` call along with everything else up to the `result`.
        Ok(Self {
            child,
            stdin,
            stdout,
        })
    }

    pub async fn send(&mut self, prompt: &str) -> Result<String> {
        let message = json::json!({
            "type": "user",
            "message": {
                "role": "user",
                "content": [{
                    "type": "text",
                    "text": prompt,
                }],
            }
        });
        let line = json::to_string(&message)?;
        log::debug!("sending claude prompt ({} chars)", prompt.len());
        self.stdin
            .write_all(line.as_bytes())
            .await
            .context("writing message line to claude stdin")?;
        self.stdin
            .write_all(b"\n")
            .await
            .context("writing newline to claude stdin")?;
        self.stdin.flush().await.context("flushing claude stdin")?;

        let mut buffer = String::new();
        loop {
            buffer.clear();
            let bytes = self
                .stdout
                .read_line(&mut buffer)
                .await
                .context("reading claude event")?;
            if bytes == 0 {
                let status = self.child.try_wait().ok().flatten();
                bail!(
                    "claude stdout closed unexpectedly (exit status: {:?})",
                    status
                );
            }
            let trimmed = buffer.trim();
            if trimmed.is_empty() {
                continue;
            }
            let event: json::Value =
                json::from_str(trimmed).with_context(|| {
                    format!("parsing claude event line: {trimmed}")
                })?;
            let event_type =
                event.get("type").and_then(|v| v.as_str()).unwrap_or("");
            if event_type == "result" {
                if event
                    .get("is_error")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
                {
                    bail!(
                        "claude returned an error result: {}",
                        event
                            .get("result")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                    );
                }
                return event
                    .get("result")
                    .and_then(|v| v.as_str())
                    .map(String::from)
                    .ok_or_else(|| {
                        anyhow!("claude result event has no 'result' field")
                    });
            }
            let event_subtype =
                event.get("subtype").and_then(|v| v.as_str()).unwrap_or("");
            log::debug!(
                "claude event: type={event_type} subtype={event_subtype}"
            );
            log::trace!("claude event raw: {trimmed}");
        }
    }
}

/// Pull a rollout from the model's free-form text. Each element may be either a
/// full `BrowserAction` object or an integer index into `available_actions`
/// (handy for picking among already-listed actions without re-typing them).
/// Tolerates surrounding prose / markdown fences by scanning for the first
/// top-level `[ ... ]` substring that parses as `Vec<json::Value>`.
fn parse_rollout(
    text: &str,
    available_actions: &[BrowserAction],
) -> Result<Vec<BrowserAction>> {
    let trimmed = text.trim();
    if let Ok(values) = json::from_str::<Vec<json::Value>>(trimmed) {
        return resolve_rollout(values, available_actions);
    }

    let bytes = trimmed.as_bytes();
    let mut start = None;
    let mut depth = 0u32;
    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b'[' => {
                if depth == 0 {
                    start = Some(i);
                }
                depth += 1;
            }
            b']' => {
                if depth == 0 {
                    continue;
                }
                depth -= 1;
                if depth == 0
                    && let Some(s) = start.take()
                {
                    let candidate = &trimmed[s..=i];
                    if let Ok(values) =
                        json::from_str::<Vec<json::Value>>(candidate)
                    {
                        return resolve_rollout(values, available_actions);
                    }
                }
            }
            _ => {}
        }
    }

    bail!("could not extract a JSON array from the model's response")
}

fn resolve_rollout(
    values: Vec<json::Value>,
    available_actions: &[BrowserAction],
) -> Result<Vec<BrowserAction>> {
    values
        .into_iter()
        .map(|value| match value {
            json::Value::Number(number) if number.is_u64() => {
                let index = number.as_u64().unwrap() as usize;
                available_actions.get(index).cloned().ok_or_else(|| {
                    anyhow!(
                        "model returned action index {} but only {} actions are listed",
                        index,
                        available_actions.len()
                    )
                })
            }
            other => json::from_value::<BrowserAction>(other.clone()).with_context(|| {
                format!(
                    "rollout element is neither an index nor a BrowserAction object: {}",
                    other
                )
            }),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_available() -> Vec<BrowserAction> {
        vec![
            BrowserAction::Reload,
            BrowserAction::Wait,
            BrowserAction::Back,
            BrowserAction::Forward,
            BrowserAction::PressKey { code: 13 },
            BrowserAction::TypeText {
                text: "x".into(),
                delay_millis: 0,
            },
        ]
    }

    #[test]
    fn parse_rollout_plain_array_of_objects() {
        let text = r#"[{"TypeText":{"text":"hi","delay_millis":50}},"Reload"]"#;
        let rollout = parse_rollout(text, &sample_available()).unwrap();
        assert!(
            matches!(rollout[0], BrowserAction::TypeText { .. })
                && matches!(rollout[1], BrowserAction::Reload)
        );
    }

    #[test]
    fn parse_rollout_array_of_indices() {
        let rollout = parse_rollout("[5, 5, 5]", &sample_available()).unwrap();
        assert_eq!(rollout.len(), 3);
        assert!(matches!(rollout[0], BrowserAction::TypeText { .. }));
    }

    #[test]
    fn parse_rollout_with_markdown_fence_and_indices() {
        let text = "```json\n[5, 5, 5]\n```";
        let rollout = parse_rollout(text, &sample_available()).unwrap();
        assert_eq!(rollout.len(), 3);
    }

    #[test]
    fn parse_rollout_mixed_indices_and_objects() {
        let text = r#"[0, {"PressKey":{"code":27}}, 1]"#;
        let rollout = parse_rollout(text, &sample_available()).unwrap();
        assert!(matches!(rollout[0], BrowserAction::Reload));
        assert!(matches!(rollout[1], BrowserAction::PressKey { code: 27 }));
        assert!(matches!(rollout[2], BrowserAction::Wait));
    }

    #[test]
    fn parse_rollout_with_prose() {
        let text = r#"Sure! Here you go: [{"PressKey":{"code":13}}]
Thanks."#;
        let rollout = parse_rollout(text, &sample_available()).unwrap();
        assert!(matches!(rollout[0], BrowserAction::PressKey { code: 13 }));
    }

    #[test]
    fn parse_rollout_rejects_non_array() {
        assert!(parse_rollout("no actions", &sample_available()).is_err());
    }

    #[test]
    fn parse_rollout_rejects_out_of_range_index() {
        assert!(parse_rollout("[99]", &sample_available()).is_err());
    }
}
