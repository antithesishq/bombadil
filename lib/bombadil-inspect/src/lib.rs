use std::rc::Rc;

use bombadil_inspect_api::TraceEntry;
use gloo_console::{error, log};
use gloo_net::http::Request;
use wasm_bindgen_futures::spawn_local;
use yew::prelude::*;

use crate::actions::ActionsList;
use crate::screenshot::Screenshot;
use crate::timeline::Timeline;

mod actions;
mod container_size;
mod duration;
mod screenshot;
mod svg;
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
                <span class="status"></span>
            </header>
            <div class="pane history">
                <h2>{"History"}</h2>
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
