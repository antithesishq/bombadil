use std::rc::Rc;

use bombadil_inspect_api::BrowserAction;
use bombadil_inspect_api::Point;
use bombadil_inspect_api::TraceEntry;
use wasm_bindgen::JsCast;
use yew::component;
use yew::prelude::*;

use crate::container_size::use_container_size;

#[derive(PartialEq, Properties)]
pub struct ScreenshotProps {
    pub entry: Rc<TraceEntry>,
    #[prop_or_default]
    pub action: Option<Rc<BrowserAction>>,
}

#[component]
pub fn Screenshot(props: &ScreenshotProps) -> Html {
    let natural_size = use_state(|| None::<(f64, f64)>);
    let (container_ref, container_size) = use_container_size();

    let on_load = {
        let natural_size = natural_size.clone();
        Callback::from(move |event: web_sys::Event| {
            if let Some(img) = event
                .target()
                .and_then(|t| t.dyn_into::<web_sys::HtmlImageElement>().ok())
            {
                natural_size.set(Some((
                    img.natural_width() as f64,
                    img.natural_height() as f64,
                )));
            }
        })
    };

    let (inner_style, overlay) = match (container_size, *natural_size) {
        (Some((cw, ch)), Some((nw, nh))) => {
            let transform = ContainTransform::new(cw, ch, nw, nh);
            let w = nw * transform.scale;
            let h = nh * transform.scale;
            let style = format!("width: {w}px; height: {h}px;");
            let overlay = props
                .action
                .as_deref()
                .and_then(action_point)
                .map(|point| {
                    let dpr = 2.0;
                    let x = point.x * dpr * transform.scale;
                    let y = point.y * dpr * transform.scale;
                    let r = 20.0_f64;
                    let d2r = 2.0 * r;
                    html!(
                        <svg class="annotation">
                            <path
                                fill-rule="evenodd"
                                d={format!(
                                    "M0,0H{w}V{h}H0Z \
                                     M{},{y} \
                                     a{r},{r} 0 1,0 {d2r},0 \
                                     a{r},{r} 0 1,0 -{d2r},0Z",
                                    x - r,
                                )}
                                fill="black"
                                opacity="0.25"
                            />
                            <circle
                                cx={x.to_string()}
                                cy={y.to_string()}
                                r={r.to_string()}
                                fill="none"
                                stroke="var(--color-fg)"
                                stroke-width="3"
                            />
                        </svg>
                    )
                })
                .unwrap_or_default();
            (style, overlay)
        }
        _ => (String::new(), Html::default()),
    };

    html!(
        <div class="screenshot" ref={container_ref}>
            <div class="img-container" style={inner_style}>
                <img
                    src={props.entry.screenshot.clone()}
                    onload={on_load}
                />
                {overlay}
            </div>
        </div>
    )
}

struct ContainTransform {
    scale: f64,
}

impl ContainTransform {
    fn new(
        container_width: f64,
        container_height: f64,
        natural_width: f64,
        natural_height: f64,
    ) -> Self {
        let scale = (container_width / natural_width)
            .min(container_height / natural_height);
        ContainTransform { scale }
    }
}

fn action_point(action: &BrowserAction) -> Option<&Point> {
    match action {
        BrowserAction::Click { point, .. }
        | BrowserAction::DoubleClick { point, .. } => Some(point),
        BrowserAction::ScrollUp { origin, .. }
        | BrowserAction::ScrollDown { origin, .. } => Some(origin),
        _ => None,
    }
}
