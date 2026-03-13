use bombadil_inspect_api::HelloResponse;
use gloo_net::http::Request;
use yew::prelude::*;

#[function_component(App)]
fn app() -> Html {
    let message = use_state(|| "Loading...".to_string());

    {
        let message = message.clone();
        use_effect_with((), move |_| {
            wasm_bindgen_futures::spawn_local(async move {
                match Request::get("/api/hello").send().await {
                    Ok(response) => {
                        match response.json::<HelloResponse>().await {
                            Ok(data) => message.set(data.message),
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
        ("Forward", None),
        ("Forward", None),
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
                <nav>
                    <h2>{"Status: "}</h2>
                    <span>{(*message).clone()}</span>
                </nav>
            </header>
            <div class="pane history">
                <h2>{"History"}</h2>
                <ol>
                {
                    actions.iter().enumerate().map(|(i, (action, details))| {
                        let li_class = if i == 0 { "current" }  else { "" };
                        match details {
                            Some(details) => html!{
                                <li class={li_class}>
                                    <details open={i == 0}>
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
            <div class="pane state-before">
                <h2>{"State before"}</h2>
                <span>{"<TODO: screenshot>"}</span>
            </div>
            <div class="pane state-after">
                <h2>{"State after"}</h2>
                <span>{"<TODO: screenshot>"}</span>
            </div>
            <div class="pane state-space">
                <h2>{"State space"}</h2>
            </div>
            <div class="pane heap">
                <h2>{"Heap"}</h2>

                <svg viewBox="0 0 600 300" xmlns="http://www.w3.org/2000/svg" style="width:100%;height:auto;">
                <polyline
                fill="none"
                stroke="#000"
                stroke-width="2"
                points="
                     0,280
                     40,280
                     40,260
                     90,260
                     90,230
                     120,230
                     120,220
                     180,220
                     180,190
                     200,190
                     200,170
                     260,170
                     260,140
                     280,140
                     280,110
                     330,110
                     330,80
                     350,80
                     350,90
                     400,90
                     400,70
                     430,70
                     430,50
                     460,50
                     460,40
                     510,40
                     510,50
                     540,50
                     540,30
                     600,30
                     "
                />
                </svg>
            </div>
            <div class="pane cpu">
                <h2>{"CPU"}</h2>
                <svg viewBox="0 0 600 300" xmlns="http://www.w3.org/2000/svg" style="width:100%;height:auto;">
                <path
                fill="none"
                stroke="#000"
                stroke-width="2"
                d="M 0,160
                L 15,145 25,155 40,120
                C 60,60 70,40 100,45
                L 115,55 125,35 135,50
                C 155,90 165,130 185,170
                L 195,190 205,175 215,200
                C 235,250 255,270 280,265
                L 290,255 300,270 310,250
                C 330,200 340,150 360,110
                L 370,95 380,105 390,80
                C 410,40 430,35 455,50
                L 465,60 475,42 485,55
                C 500,100 510,150 530,190
                L 540,210 548,195 555,215
                C 570,250 580,260 600,245"
                />
                </svg>
            </div>
            <div class="pane statistics">
                <table>
                    <thead>
                        <tr>
                            <th><h2>{"Statistics"}</h2></th>
                            <td>{"New"}</td>
                            <td>{"Total"}</td>
                        </tr>
                    </thead>
                {
                    statistics.iter().map(|(label, new, total)| {
                        html!{
                            <tr>
                                <th>{label}</th>
                                <td>{new}</td>
                                <td>{total}</td>
                            </tr>
                        }
                    }).collect::<Html>()
                }
                </table>
            </div>
            <div class="debug-grid" />
        </main>
    }
}

#[wasm_bindgen::prelude::wasm_bindgen(start)]
pub fn run_app() {
    yew::Renderer::<App>::new().render();
}
