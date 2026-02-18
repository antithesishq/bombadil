use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use chromiumoxide::Page;
use chromiumoxide::cdp::browser_protocol::{input, page};
use serde::Serialize;
use serde::{Deserialize, Deserializer};
use tokio::time::sleep;

use crate::browser::keys::key_name;
use crate::geometry::Point;

fn deserialize_u8_from_number<'de, D>(deserializer: D) -> Result<u8, D::Error>
where
    D: Deserializer<'de>,
{
    use serde::de::Error;
    let value = serde_json::Value::deserialize(deserializer)?;
    match value {
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_u64() {
                if i <= u8::MAX as u64 {
                    Ok(i as u8)
                } else {
                    Err(Error::custom(format!(
                        "expected u8, got out-of-range integer: {}",
                        i
                    )))
                }
            } else if let Some(f) = n.as_f64() {
                if f.fract() == 0.0 && f >= 0.0 && f <= u8::MAX as f64 {
                    Ok(f as u8)
                } else {
                    Err(Error::custom(format!(
                        "expected u8, got non-integer or out-of-range float: {}",
                        f
                    )))
                }
            } else {
                Err(Error::custom("expected u8, got invalid number"))
            }
        }
        _ => Err(Error::custom("expected a number")),
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum BrowserAction {
    Back,
    Forward,
    Click {
        name: String,
        content: Option<String>,
        point: Point,
    },
    TypeText {
        text: String,
        #[serde(rename = "delayMillis")]
        delay_millis: f64,
    },
    PressKey {
        #[serde(deserialize_with = "deserialize_u8_from_number")]
        code: u8,
    },
    ScrollUp {
        origin: Point,
        distance: f64,
    },
    ScrollDown {
        origin: Point,
        distance: f64,
    },
    Reload,
}

impl BrowserAction {
    pub async fn apply(&self, page: &Page) -> Result<()> {
        match self {
            BrowserAction::Back => {
                let history =
                    page.execute(page::GetNavigationHistoryParams {}).await?;
                if history.current_index == 0 {
                    bail!("can't go back from first navigation entry");
                }
                let last: page::NavigationEntry = history.entries
                    [(history.current_index - 1) as usize]
                    .clone();
                page.execute(
                    page::NavigateToHistoryEntryParams::builder()
                        .entry_id(last.id)
                        .build()
                        .map_err(|err| anyhow!(err))?,
                )
                .await?;
            }
            BrowserAction::Forward => {
                let history =
                    page.execute(page::GetNavigationHistoryParams {}).await?;
                let next_index = (history.current_index + 1) as usize;
                if next_index >= history.entries.len() {
                    bail!("can't go forward from last navigation entry");
                }
                let next: page::NavigationEntry =
                    history.entries[next_index].clone();
                page.execute(
                    page::NavigateToHistoryEntryParams::builder()
                        .entry_id(next.id)
                        .build()
                        .map_err(|err| anyhow!(err))?,
                )
                .await?;
            }
            BrowserAction::Reload => {
                page.reload().await?;
            }
            BrowserAction::ScrollUp { origin, distance } => {
                page.execute(
                    input::SynthesizeScrollGestureParams::builder()
                        .x(origin.x)
                        .y(origin.y)
                        .y_distance(*distance)
                        .speed((distance.abs() * 10.0) as i64)
                        .build()
                        .map_err(|err| anyhow!(err))?,
                )
                .await?;
            }
            BrowserAction::ScrollDown { origin, distance } => {
                page.execute(
                    input::SynthesizeScrollGestureParams::builder()
                        .x(origin.x)
                        .y(origin.y)
                        .y_distance(-distance)
                        .speed((distance.abs() * 10.0) as i64)
                        .build()
                        .map_err(|err| anyhow!(err))?,
                )
                .await?;
            }
            BrowserAction::Click { point, .. } => {
                page.click((*point).into()).await?;
            }
            BrowserAction::TypeText { text, delay_millis } => {
                let delay = Duration::from_millis(*delay_millis as u64);
                for char in text.chars() {
                    sleep(delay).await;
                    page.execute(input::InsertTextParams::new(char)).await?;
                }
            }
            BrowserAction::PressKey { code } => {
                let build_params = |event_type| {
                    if let Some(name) = key_name(*code) {
                        input::DispatchKeyEventParams::builder()
                            .r#type(event_type)
                            .native_virtual_key_code(*code as i64)
                            .windows_virtual_key_code(*code as i64)
                            .code(name)
                            .key(name)
                            .unmodified_text("\r")
                            .text("\r")
                            .build()
                            .map_err(|err| anyhow!(err))
                    } else {
                        bail!("unknown key with code: {:?}", code)
                    }
                };
                page.execute(build_params(
                    input::DispatchKeyEventType::RawKeyDown,
                )?)
                .await?;
                page.execute(build_params(input::DispatchKeyEventType::Char)?)
                    .await?;
                page.execute(build_params(input::DispatchKeyEventType::KeyUp)?)
                    .await?;
            }
        };
        Ok(())
    }
}
