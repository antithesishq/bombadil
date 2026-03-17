use std::rc::Rc;
use std::time::Duration;
use std::time::SystemTime;

use bombadil_browser_keys::key_name;
use bombadil_inspect_api::Point;
use bombadil_inspect_api::TraceEntry;
use gloo_console::{error, log};
use gloo_net::http::Request;
use wasm_bindgen_futures::spawn_local;
use yew::component;
use yew::prelude::*;

use crate::screenshot::Screenshot;
use crate::timeline::Timeline;

mod container_size;
mod screenshot;
mod timeline;

#[function_component(App)]
fn app() -> Html {
    let selected_index = use_state_eq(|| 1usize);
    let trace = use_state(|| None::<Rc<[TraceEntry]>>);
    {
        let trace = trace.clone();
        use_effect_with((), move |_| {
            spawn_local(async move {
                match Request::get("/api/trace").send().await {
                    Ok(response) => {
                        match response.json::<Vec<TraceEntry>>().await {
                            Ok(entries) => {
                                log!("Loaded trace entries:", entries.len());
                                trace.set(Some(Rc::from(entries)));
                            }
                            Err(error) => {
                                error!(
                                    "Failed to parse response: ",
                                    error.to_string()
                                )
                            }
                        }
                    }
                    Err(error) => error!("Failed to fetch:", error.to_string()),
                }
            });
            || ()
        });
    }

    html! {
        <main class="grid">
            <header class="pane">
                <h1>{"Bombadil Inspect"}</h1>
                <span class="status">{"● Live"}</span>
            </header>
            <div class="pane history">
                <h2>{"History"}</h2>
                <div class="content">
                    <ol>
                    {
                        if let Some(trace) = trace.as_ref() && !trace.is_empty() {
                            let test_start = trace.first().expect("no first trace entry").timestamp;
                            trace.iter().enumerate().map(|(i, entry)| {
                                let selected_index = selected_index.clone();
                                html!(
                                    <HistoryEntry
                                        entry={Rc::new(entry.clone())}
                                        is_current={i == *selected_index}
                                            test_start={test_start}
                                            index={i}
                                            on_select={Callback::from(move |index| {
                                                selected_index.set(index)
                                            })} />
                                )
                            }).collect::<Html>()
                        } else {
                            html!()
                        }
                    }
                    </ol>
                </div>
            </div>
            <div class="pane state-before">
                <h2>{"State before"}</h2>
                {if let Some(ref trace) = *trace && let Some(entry) = trace.get(selected_index.saturating_sub(1)) {
                    let action = trace.get(*selected_index).and_then(|e| e.action.clone()).map(Rc::new);
                    html!(<Screenshot entry={Rc::new(entry.clone())} action={action} />)
                } else {Html::default()}}
            </div>
            <div class="pane state-after">
                <h2>{"State after"}</h2>
                {if let Some(ref trace) = *trace && let Some(entry) = trace.get(*selected_index) {
                    html!(<Screenshot entry={Rc::new(entry.clone())} />)
                } else {Html::default()}}
            </div>
                <footer class="pane">
                {if let Some(ref trace) = *trace {
                    html!(<Timeline entries={trace.clone()} />)
                } else {Html::default()}}
                </footer>
        </main>
    }
}

#[derive(PartialEq, Properties)]
struct HistoryEntryProps {
    pub test_start: SystemTime,
    pub entry: Rc<TraceEntry>,
    pub index: usize,
    pub is_current: bool,
    pub on_select: Callback<usize>,
}

#[component]
fn HistoryEntry(props: &HistoryEntryProps) -> Html {
    let (action_name, details): (&str, Option<Vec<(&str, String)>>) =
        match &props.entry.action {
            Some(action) => match action {
                bombadil_inspect_api::BrowserAction::Back => ("Back", None),
                bombadil_inspect_api::BrowserAction::Forward => {
                    ("Forward", None)
                }
                bombadil_inspect_api::BrowserAction::Click {
                    point, ..
                } => ("Click", Some(vec![("Position", format_point(point))])),
                bombadil_inspect_api::BrowserAction::DoubleClick {
                    point,
                    delay_millis,
                    ..
                } => (
                    "Double-click",
                    Some(vec![
                        ("Position", format_point(point)),
                        ("Delay", format!("{}ms", delay_millis)),
                    ]),
                ),
                bombadil_inspect_api::BrowserAction::TypeText {
                    text,
                    delay_millis,
                } => (
                    "Type",
                    Some(vec![
                        ("Text", text.clone()),
                        ("Delay", format!("{}ms", delay_millis)),
                    ]),
                ),
                bombadil_inspect_api::BrowserAction::PressKey {
                    code, ..
                } => (
                    "Press key",
                    Some(vec![(
                        "Key",
                        key_name(*code).unwrap_or("Unknown").to_string(),
                    )]),
                ),
                bombadil_inspect_api::BrowserAction::ScrollUp {
                    origin,
                    distance,
                } => (
                    "Scroll up",
                    Some(vec![
                        ("Origin", format_point(origin)),
                        ("Distance", format!("{}px", distance)),
                    ]),
                ),
                bombadil_inspect_api::BrowserAction::ScrollDown {
                    origin,
                    distance,
                } => (
                    "Scroll down",
                    Some(vec![
                        ("Origin", format_point(origin)),
                        ("Distance", format!("{}px", distance)),
                    ]),
                ),
                bombadil_inspect_api::BrowserAction::Reload => ("Reload", None),
                bombadil_inspect_api::BrowserAction::Wait => ("Wait", None),
            },
            None => return html! {},
        };
    let li_class = if props.is_current { "current" } else { "" };
    let duration_since_start = props
        .entry
        .timestamp
        .duration_since(props.test_start)
        .unwrap_or_default();

    let index: usize = props.index;
    let on_select = props.on_select.clone();
    let on_click = move |_| on_select.emit(index);

    html! {
        <li class={li_class} role="button" onclick={on_click}>
            <header>
                <div class="action-name">{action_name}</div>
                <time title={format!("{:?}", duration_since_start)}>{format_duration(duration_since_start)}</time>
            </header>
            {if let Some(details) = details && props.is_current {
                html!(
                    <table class="details">
                    {details.iter().map(|(name, value)| {
                        html!(
                            <tr>
                                <th>{name}</th>
                                <td>{value}</td>
                            </tr>
                        )
                    }).collect::<Html>()}
                    </table>

                )
            } else { Html::default() }}
        </li>
    }
}

fn format_point(point: &Point) -> String {
    format!("{:.1}, {:.1}", point.x, point.y)
}

fn format_duration(duration: Duration) -> String {
    let total_secs = duration.as_secs();
    let hours = total_secs / 3600;
    let minutes = (total_secs % 3600) / 60;
    let seconds = total_secs % 60;
    if hours > 0 {
        format!("{:02}:{:02}:{:02}", hours, minutes, seconds)
    } else {
        format!("{:02}:{:02}", minutes, seconds)
    }
}

#[wasm_bindgen::prelude::wasm_bindgen(start)]
pub fn run_app() {
    yew::Renderer::<App>::new().render();
}
