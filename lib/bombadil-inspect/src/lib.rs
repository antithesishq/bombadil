use std::rc::Rc;

use bombadil_schema::{TraceEntry, WsTraceEntryMessage};
use futures::StreamExt;
use gloo_console::{error, log};
use gloo_net::http::Request;
use gloo_net::websocket::futures::WebSocket;
use wasm_bindgen_futures::spawn_local;
use yew::prelude::*;

use crate::actions::ActionsList;
use crate::screenshot::Screenshot;
use crate::state_details::StateDetails;
use crate::timeline::Timeline;

mod actions;
mod container_size;
mod duration;
mod list_autoscroll;
mod render;
mod screenshot;
mod state_details;
mod time;
mod timeline;

#[function_component(App)]
fn app() -> Html {
    let selected_index = use_state_eq(|| 1usize);
    let trace = use_state(|| None::<Rc<[Rc<TraceEntry>]>>);

    let is_following_list = use_state(|| true);

    let search = web_sys::window().unwrap().location().search().unwrap();
    let params = web_sys::UrlSearchParams::new_with_str(&search).unwrap();

    {
        let trace = trace.clone();
        use_effect_with((), move |_| {
            if let Some(port) = params.get("streaming-port") {
                let ws = WebSocket::open(&format!("ws://127.0.0.1:{port}/"))
                    .unwrap();
                let (_write, mut read) = ws.split();

                spawn_local(async move {
                    while let Some(msg) = read.next().await {
                        let Ok(gloo_net::websocket::Message::Text(text)) = msg
                        else {
                            continue;
                        };
                        match serde_json::from_str::<WsTraceEntryMessage>(&text)
                        {
                            Ok(WsTraceEntryMessage::Entry(entry)) => {
                                let mut entries = trace
                                    .as_ref()
                                    .map(|t| t.to_vec())
                                    .unwrap_or_default();
                                entries.push(Rc::new(entry));
                                trace.set(Some(Rc::from(entries)));
                            }
                            Ok(WsTraceEntryMessage::AllEntries(all)) => {
                                let entries: Vec<_> =
                                    all.into_iter().map(Rc::new).collect();
                                trace.set(Some(Rc::from(entries)))
                            }
                            Err(e) => log!(format!("{e}")),
                        }
                    }
                })
            } else {
                spawn_local(async move {
                    let response = match Request::get("/api/trace").send().await
                    {
                        Ok(r) => r,
                        Err(e) => {
                            return error!("Failed to fetch:", e.to_string());
                        }
                    };
                    match response.json::<Vec<TraceEntry>>().await {
                        Ok(entries) => {
                            log!("Loaded trace entries:", entries.len());
                            let entries: Vec<_> =
                                entries.into_iter().map(Rc::new).collect();
                            trace.set(Some(Rc::from(entries)));
                        }
                        Err(e) => {
                            error!("Failed to parse response:", e.to_string())
                        }
                    }
                });
            }
            || ()
        });
    }

    // When following, derive the selected index from the trace length so that
    // a single `trace.set()` in the WS handler is enough — no second state
    // update, no double-render flicker.
    let trace_len = trace.as_ref().map(|t| t.len()).unwrap_or(0);

    let effective_index = if *is_following_list {
        trace_len.saturating_sub(1)
    } else {
        *selected_index
    };

    let on_select = {
        let selected_index = selected_index.clone();
        let is_following_list = is_following_list.clone();
        Callback::from(move |index: usize| {
            selected_index.set(index);

            is_following_list.set(index == trace_len - 1);
        })
    };

    // TODO: this should be part of test metadata
    let test_start =
        trace.as_ref().and_then(|t| t.first()).map(|e| e.timestamp);
    let before_entry = trace
        .as_ref()
        .and_then(|t| t.get(effective_index.saturating_sub(1)))
        .cloned();
    let after_entry =
        trace.as_ref().and_then(|t| t.get(effective_index)).cloned();
    let action = after_entry
        .as_ref()
        .and_then(|e| e.action.clone())
        .map(Rc::new);

    html! {
        <main class="grid">

            <svg width="0" height="0" aria-hidden="true" focusable="false">
              <defs>
                <pattern id="dither" width="2" height="2" patternUnits="userSpaceOnUse">
                        <circle cx="1" cy="1" r="1" opacity="0.3" />
                </pattern>
                <pattern id="violation" width="1" height="2" patternUnits="userSpaceOnUse">
                    <rect width="1" height="1" opacity="0.3" />
                </pattern>
              </defs>
            </svg>

            <header class="pane">
                <h1>{"Bombadil Inspect"}</h1>
                <span class="status"></span>
            </header>

            <div class="pane actions">
                <h2>{"Actions"}</h2>
                <div class="content">
                {
                    if let Some(trace) = trace.as_ref() && !trace.is_empty() {
                        html!(
                            <ActionsList
                                trace={trace.clone()}
                                selected_index={effective_index}
                                on_select={on_select.clone()}
                                is_following={is_following_list.clone()}
                                />
                            )
                    } else { Html::default() }
                }
                </div>
                {if !*is_following_list {
                    let ifl = is_following_list.clone();
                    // TODO: Figure out why all the styles are overridden when using a button :/
                    html!(<p onclick={Callback::from(move |_| ifl.set(true))} class="follow-list">{"↓ FOLLOW LIST ↓"}</p>)
                } else { Html::default() }}
            </div>

            <div class="pane state-screenshot before">
                <h2>{"State before"}</h2>
                {if let Some(ref entry) = before_entry {
                    html!(<Screenshot entry={entry.clone()} action={action.clone()} />)
                } else {Html::default()}}
            </div>

            <div class="pane state-screenshot after">
                <h2>{"State after"}</h2>
                {if let Some(ref entry) = after_entry {
                    html!(<Screenshot entry={entry.clone()} />)
                } else {Html::default()}}
            </div>

            <div class="pane state-details before">
                <div class="content">
                    {if let (Some(entry), Some(test_start)) = (&before_entry, test_start) {
                        html!(<StateDetails entry={entry.clone()} {test_start} />)
                    } else { Html::default() }}
                </div>
            </div>

            <div class="pane state-details after">
                <div class="content">
                    {if let (Some(entry), Some(test_start)) = (&after_entry, test_start) {
                        html!(<StateDetails entry={entry.clone()} {test_start} />)
                    } else {Html::default()}}
                </div>
            </div>

            <footer class="pane">
                {if let (Some(trace), Some(test_start)) = (trace.as_ref(), test_start) {
                    html!(<Timeline entries={Rc::clone(trace)} {test_start} selected_index={effective_index} on_select={on_select.clone()} />)
                } else {Html::default()}}
            </footer>
        </main>
    }
}

#[wasm_bindgen::prelude::wasm_bindgen(start)]
pub fn run_app() {
    yew::Renderer::<App>::new().render();
}
