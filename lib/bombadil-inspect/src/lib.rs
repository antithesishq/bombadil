use std::rc::Rc;

use bombadil_schema::TraceEntry;
use gloo_console::{error, log};
use gloo_net::http::Request;
use wasm_bindgen_futures::spawn_local;
use yew::prelude::*;

use crate::actions::ActionsList;
use crate::screenshot::Screenshot;
use crate::state_details::StateDetails;
use crate::timeline::Timeline;

mod actions;
mod container_size;
mod duration;
mod render;
mod screenshot;
mod state_details;
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

            <svg width="0" height="0" aria-hidden="true" focusable="false">
              <defs>
                <pattern id="dither" width="1.5" height="1.5" patternUnits="userSpaceOnUse">
                        <circle cx="1" cy="1" r=".5" opacity="0.4" />
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
                        let selected_index = selected_index.clone();
                        html!(
                            <ActionsList
                                trace={trace.clone()}
                                selected_index={*selected_index}
                                on_select={Callback::from(move |index| { selected_index.set(index) })}
                                />
                            )
                    } else { Html::default() }
                }
                </div>
            </div>

            <div class="pane state-screenshot before">
                <h2>{"State before"}</h2>
                {if let Some(ref trace) = *trace && let Some(entry) = trace.get(selected_index.saturating_sub(1)) {
                    let action = trace.get(*selected_index).and_then(|e| e.action.clone()).map(Rc::new);
                    html!(<Screenshot entry={Rc::new(entry.clone())} action={action} />)
                } else {Html::default()}}
            </div>

            <div class="pane state-screenshot after">
                <h2>{"State after"}</h2>
                {if let Some(ref trace) = *trace && let Some(entry) = trace.get(*selected_index) {
                    html!(<Screenshot entry={Rc::new(entry.clone())} />)
                } else {Html::default()}}
            </div>

            <div class="pane state-details before">
                <div class="content">
                    {if let Some(ref trace) = *trace && let Some(entry) = trace.get(selected_index.saturating_sub(1)) {
                        // TODO: this should be part of test metadata
                        let test_start = trace.first().expect("no first trace entry").timestamp;
                        html!(<StateDetails entry={Rc::new(entry.clone())} test_start={test_start} />)
                    } else { Html::default() }}
                </div>
            </div>

            <div class="pane state-details after">
                <div class="content">
                    {if let Some(ref trace) = *trace && let Some(entry) = trace.get(*selected_index) {
                        // TODO: this should be part of test metadata
                        let test_start = trace.first().expect("no first trace entry").timestamp;
                        html!(<StateDetails entry={Rc::new(entry.clone())} test_start={test_start} />)
                    } else {Html::default()}}
                </div>
            </div>

            <footer class="pane">
                {if let Some(ref trace) = *trace {
                    // TODO: this should be part of test metadata
                    let test_start = trace.first().expect("no first trace entry").timestamp;
                    let on_select = {
                        let selected_index = selected_index.clone();
                        Callback::from(move |index| selected_index.set(index))
                    };
                    html!(<Timeline entries={trace.clone()} test_start={test_start} selected_index={*selected_index} on_select={on_select} />)
                } else {Html::default()}}
            </footer>
        </main>
    }
}

#[wasm_bindgen::prelude::wasm_bindgen(start)]
pub fn run_app() {
    yew::Renderer::<App>::new().render();
}
