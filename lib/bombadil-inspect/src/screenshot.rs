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
        (
            Some((container_width, container_height)),
            Some((natural_width, natural_height)),
        ) => {
            let transform = ContainTransform::new(
                container_width,
                container_height,
                natural_width,
                natural_height,
            );
            let width = natural_width * transform.scale;
            let height = natural_height * transform.scale;
            let style = format!("width: {width}px; height: {height}px;");
            let overlay = props
                .action
                .as_deref()
                .and_then(action_point)
                .map(|point| {
                    // TODO: this needs to be part of test metadata
                    let device_scale_factor = 2.0;
                    let x = point.x * device_scale_factor * transform.scale;
                    let y = point.y * device_scale_factor * transform.scale;
                    let radius = 20.0_f64;
                    let diameter = 2.0 * radius;
                    html!(
                        <svg class="annotation">
                            <path
                                fill-rule="evenodd"
                                d={format!(
                                    "M0,0H{width}V{height}H0Z \
                                     M{},{y} \
                                     a{radius},{radius} 0 1,0 {diameter},0 \
                                     a{radius},{radius} 0 1,0 -{diameter},0Z",
                                    x - radius,
                                )}
                                fill="var(--color-overlay)"
                                opacity="0.5"
                            />
                            <circle
                                cx={x.to_string()}
                                cy={y.to_string()}
                                r={radius.to_string()}
                                fill="none"
                                stroke="var(--color-selected)"
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
                    alt="Screenshot"
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
