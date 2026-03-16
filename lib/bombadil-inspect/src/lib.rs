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

#[function_component(App)]
fn app() -> Html {
    let selected_index = use_state_eq(|| 0usize);
    let trace = use_state(|| None::<Vec<TraceEntry>>);
    {
        let trace = trace.clone();
        use_effect_with((), move |_| {
            spawn_local(async move {
                match Request::get("/api/trace").send().await {
                    Ok(response) => {
                        match response.json::<Vec<TraceEntry>>().await {
                            Ok(entries) => {
                                log!("Loaded trace entries:", entries.len());
                                trace.set(Some(entries));
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
                    html!(<Screenshot entry={Rc::new(entry.clone())} />)
                } else {Html::default()}}
            </div>
            <div class="pane state-after">
                <h2>{"State after"}</h2>
                {if let Some(ref trace) = *trace && let Some(entry) = trace.get(*selected_index) {
                    html!(<Screenshot entry={Rc::new(entry.clone())} />)
                } else {Html::default()}}
            </div>
                <footer class="pane">
                    <svg viewBox="0 0 624 76" xmlns="http://www.w3.org/2000/svg" >
                        <defs>
                            <DitherPattern />
                        </defs>

                        // Timeline
                        <g transform="translate(12, 0)">
                            <g transform="translate(0, 0)">
                                <polyline
                                class="border"
                                points="
                                0,60
                                600,60
                                "
                                />
                                <polyline
                                class="border"
                                points="
                                0,0
                                0,64
                                "
                                />
                                <polyline
                                class="border"
                                points="
                                150,0
                                150,62
                                "
                                />
                                <polyline
                                class="border"
                                points="
                                300,0
                                300,64
                                "
                                />
                                <polyline
                                class="border"
                                points="
                                450,0
                                450,62
                                "
                                />
                                <polyline
                                class="border"
                                points="
                                600,0
                                600,64
                                "
                                />
                            </g>
                            <g transform="translate(0, 68)" class="time">
                                <text x="0" y="0">{"00:00"}</text>
                                <text x="300" y="0">{"02:43"}</text>
                                <text x="600" y="0">{"05:26"}</text>
                            </g>
                            <g class="events" transform="translate(0, 60)">
                                <circle cx="50" cy="0" />
                                <circle cx="180" cy="0" />
                                <circle cx="520" cy="0" />
                            </g>
                        </g>

                        // Heap
                        <g transform="translate(6, 15)">
                            <g transform="rotate(270 0 0)">
                                <text class="label">{"Heap"}</text>
                            </g>
                        </g>
                        <g transform="translate(12, 0)">
                            <polyline
                            class="border"
                            points="
                            0,30
                            600,30
                            "
                            />
                            <polyline
                            fill="none"
                            stroke-width=".5"
                            points="
                            0,28
                            40,28
                            40,26
                            90,26
                            90,23
                            120,23
                            120,22
                            180,22
                            180,19
                            200,19
                            200,17
                            260,17
                            260,14
                            280,14
                            280,11
                            330,11
                            330,8
                            350,8
                            350,9
                            400,9
                            400,7
                            430,7
                            430,5
                            460,5
                            460,4
                            510,4
                            510,5
                            540,5
                            540,3
                            600,3
                            "
                            />
                        </g>

                        // CPU
                        <g transform="translate(6, 45)">
                            <g transform="rotate(270 0 0)">
                                <text class="label">{"CPU"}</text>
                            </g>
                        </g>
                        <g transform="translate(12, 30)">
                            <path
                            fill="none"
                            stroke-width=".5"
                            d="M 0,16
                            L 15,14 25,15 40,12
                            C 60,6 70,4 100,4
                            L 115,5 125,3 135,5
                            C 155,9 165,13 185,17
                            L 195,19 205,17 215,20
                            C 235,25 255,27 280,26
                            L 290,25 300,27 310,25
                            C 330,20 340,15 360,11
                            L 370,9 380,10 390,8
                            C 410,4 430,3 455,5
                            L 465,6 475,4 485,5
                            C 500,10 510,15 530,19
                            L 540,21 548,19 555,21
                            C 570,25 580,26 600,24"
                            />
                        </g>

                        <g transform="translate(112, 0)">
                            <rect class="cursor" x="0" y="0" width="12" height="114" fill="url(#dither)" />
                        </g>
                    </svg>
            </footer>
        </main>
    }
}

#[function_component(DitherPattern)]
fn dither_pattern() -> Html {
    html!(
        <pattern id="dither" width="1" height="1" patternUnits="userSpaceOnUse">
            <circle cx="1" cy="1" r="0.5" fill="currentColor" opacity="0.5" />
        </pattern>
    )
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
                <time title={format!("{:?}", duration_since_start)}>{format_duration(duration_since_start)}</time>
                <div class="action-name">{action_name}</div>
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

#[derive(PartialEq, Properties)]
struct ScreenshotProps {
    pub entry: Rc<TraceEntry>,
}

#[component]
fn Screenshot(props: &ScreenshotProps) -> Html {
    html!(
        <div class="screenshot">
            <img src={props.entry.screenshot.clone()} />
        </div>
    )
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
