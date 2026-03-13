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

    html! {
        <div>
            <h1>{"Bombadil Inspect"}</h1>
            <p>{"It says: "}{(*message).clone()}</p>
        </div>
    }
}

#[wasm_bindgen::prelude::wasm_bindgen(start)]
pub fn run_app() {
    yew::Renderer::<App>::new().render();
}
