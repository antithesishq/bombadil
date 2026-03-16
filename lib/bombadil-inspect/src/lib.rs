use bombadil_inspect_api::TraceEntry;
use gloo_net::http::Request;
use yew::prelude::*;

#[function_component(App)]
fn app() -> Html {
    let trace = use_state(|| None::<Vec<TraceEntry>>);
    let message = use_state(|| "Loading...".to_string());

    {
        let trace = trace.clone();
        let message = message.clone();
        use_effect_with((), move |_| {
            wasm_bindgen_futures::spawn_local(async move {
                match Request::get("/api/trace").send().await {
                    Ok(response) => {
                        match response.json::<Vec<TraceEntry>>().await {
                            Ok(entries) => {
                                message.set(format!(
                                    "Loaded {} trace entries",
                                    entries.len()
                                ));
                                trace.set(Some(entries));
                            }
                            Err(_) => message
                                .set("Failed to parse response".to_string()),
                        }
                    }
                    Err(_) => message.set("Failed to fetch".to_string()),
                }
            });
            || ()
        });
    }

    let actions = [
        ("Click", Some("x: 600, y: 312")),
        ("Double-click", Some("x: 600, y: 312")),
        ("Back", None),
        ("Click", Some("x: 600, y: 312")),
        ("Double-click", Some("x: 600, y: 312")),
        ("Back", None),
        ("Forward", None),
        ("Forward", None),
        ("Scroll down", Some("840px")),
        ("Click", Some("x: 600, y: 312")),
        ("Reload", None),
        ("Forward", None),
        ("Forward", None),
        ("Scroll down", Some("840px")),
        ("Click", Some("x: 600, y: 312")),
        ("Click", Some("x: 600, y: 312")),
        ("Double-click", Some("x: 600, y: 312")),
        ("Back", None),
        ("Forward", None),
        ("Forward", None),
        ("Scroll down", Some("840px")),
        ("Click", Some("x: 600, y: 312")),
        ("Reload", None),
        ("Reload", None),
        ("Click", Some("x: 600, y: 312")),
        ("Double-click", Some("x: 600, y: 312")),
        ("Back", None),
        ("Forward", None),
        ("Forward", None),
        ("Click", Some("x: 600, y: 312")),
        ("Double-click", Some("x: 600, y: 312")),
        ("Back", None),
        ("Forward", None),
        ("Forward", None),
        ("Scroll down", Some("840px")),
        ("Click", Some("x: 600, y: 312")),
        ("Reload", None),
        ("Scroll down", Some("840px")),
        ("Click", Some("x: 600, y: 312")),
        ("Reload", None),
    ];

    let statistics = [
        ("States seen", 201, 1321),
        ("Edges covered", 165, 893),
        ("Violations", 0, 1),
    ];

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
                        actions.iter().enumerate().map(|(i, (action, details))| {
                            let li_class = if i == 9 { "current" }  else { "" };
                            match details {
                                Some(details) => html!{
                                    <li class={li_class}>
                                        <details open={i == 9}>
                                            <summary>{action}</summary>
                                            {details}
                                        </details>
                                    </li>
                                },
                                None => html!{<li class={li_class}>{action}</li>},
                            }
                        }).collect::<Html>()
                    }
                    </ol>
                </div>
            </div>
            <div class="pane state-before">
                <h2>{"State before"}</h2>
                <div class="todo">{"TODO: screenshot"}</div>
            </div>
            <div class="pane state-after">
                <h2>{"State after"}</h2>
                <div class="todo">{"TODO: screenshot"}</div>
            </div>
                <footer class="pane">
                    <svg viewBox="0 0 624 76" xmlns="http://www.w3.org/2000/svg" >
                        <defs>
                          <pattern id="dither" width="1" height="1" patternUnits="userSpaceOnUse">
                            <circle cx="0.5" cy="0.5" r="0.5" fill="currentColor" opacity="0.25" />
                          </pattern>
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

#[wasm_bindgen::prelude::wasm_bindgen(start)]
pub fn run_app() {
    yew::Renderer::<App>::new().render();
}
