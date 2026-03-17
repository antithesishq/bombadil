use std::rc::Rc;
use std::time::SystemTime;

use bombadil_inspect_api::TraceEntry;
use gloo_console::log;
use yew::component;
use yew::prelude::*;

use crate::container_size::use_container_size;

#[derive(PartialEq, Properties)]
pub struct TimelineProps {
    pub entries: Rc<[TraceEntry]>,
    pub test_start: SystemTime,
}

#[component]
pub fn Timeline(props: &TimelineProps) -> Html {
    let (container_ref, container_size) = use_container_size();

    let series_heap: Series = {
        let mut series_heap = Vec::with_capacity(props.entries.len());
        for entry in props.entries.iter() {
            let pair = (
                entry
                    .timestamp
                    .duration_since(props.test_start)
                    .expect("couldn't calculate offset time")
                    .as_millis() as f64,
                entry.resources.js_heap_used as f64,
            );
            log!(format!("{:?}", pair));
            series_heap.push(pair);
        }
        series_heap.into()
    };

    let print_y = Callback::from(move |y: f64| y.to_string());

    let component_inner = if let Some((width, height)) = container_size {
        html!(
            <svg class="timeline" viewBox={format!("0 0 {width} {height}")} xmlns="http://www.w3.org/2000/svg" >
                <defs>
                    <DitherPattern />
                </defs>

                // Timeline
                // <g transform="translate(12, 0)">
                //     <g transform="translate(0, 0)">
                //         <polyline
                //         class="border"
                //         points="
                //     0,60
                //     600,60
                //     "
                //         />
                //         <polyline
                //         class="border"
                //         points="
                //     0,0
                //     0,64
                //     "
                //         />
                //         <polyline
                //         class="border"
                //         points="
                //     150,0
                //     150,62
                //     "
                //         />
                //         <polyline
                //         class="border"
                //         points="
                //     300,0
                //     300,64
                //     "
                //         />
                //         <polyline
                //         class="border"
                //         points="
                //     450,0
                //     450,62
                //     "
                //         />
                //         <polyline
                //         class="border"
                //         points="
                //     600,0
                //     600,64
                //     "
                //         />
                //     </g>
                //     <g transform="translate(0, 68)" class="time">
                //         <text x="0" y="0">{"00:00"}</text>
                //         <text x="300" y="0">{"02:43"}</text>
                //         <text x="600" y="0">{"05:26"}</text>
                //     </g>
                //     <g class="events" transform="translate(0, 60)">
                //         <circle cx="50" cy="0" />
                //         <circle cx="180" cy="0" />
                //         <circle cx="520" cy="0" />
                //     </g>
                // </g>

                // Heap
                <g transform="translate(6, 0)">
                    <LineChart
                        name="Heap"
                        width={width}
                        height={height / 3.0}
                        series={series_heap}
                        print_y={print_y} />
                </g>

                // CPU
                // <g transform="translate(6, 45)">
                //     <g transform="rotate(270 0 0)">
                //         <text class="label">{"CPU"}</text>
                //     </g>
                // </g>
                // <g transform="translate(12, 30)">
                //     <path
                //     fill="none"
                //     stroke-width=".5"
                //     d="M 0,16
                // L 15,14 25,15 40,12
                // C 60,6 70,4 100,4
                // L 115,5 125,3 135,5
                // C 155,9 165,13 185,17
                // L 195,19 205,17 215,20
                // C 235,25 255,27 280,26
                // L 290,25 300,27 310,25
                // C 330,20 340,15 360,11
                // L 370,9 380,10 390,8
                // C 410,4 430,3 455,5
                // L 465,6 475,4 485,5
                // C 500,10 510,15 530,19
                // L 540,21 548,19 555,21
                // C 570,25 580,26 600,24"
                //     />
                // </g>

                <g transform="translate(112, 0)">
                    <rect class="cursor" x="0" y="0" width="12" height={height.to_string()} fill="url(#dither)" />
                </g>
            </svg>
        )
    } else {
        html!()
    };

    html!(
        <div class="timeline" ref={container_ref}>
            {component_inner}
        </div>
    )
}

type Series = Rc<[(f64, f64)]>;

#[derive(PartialEq, Properties)]
pub struct LineChartProps {
    name: AttrValue,
    series: Series,
    width: f64,
    height: f64,
    print_y: Callback<f64, String>,
}

#[component]
pub fn LineChart(props: &LineChartProps) -> Html {
    let (x_max, y_max) = if let Some((x, y)) = props
        .series
        .iter()
        .copied()
        .reduce(|(acc_x, acc_y), (x, y)| {
            (f64::max(acc_x, x), f64::max(acc_y, y))
        }) {
        (x, y)
    } else {
        return html!();
    };

    let points = {
        let mut points = vec![];
        for (x, y) in props.series.iter() {
            let x = (x / x_max) * props.width;
            let y = (y / y_max) * props.height;
            points.push(format!("{x},{y}"))
        }
        points
    };

    html!(
        <g class="line-chart">
            <polyline class="border" points={format!("0,{height} {width},{height}", height=props.height, width=props.width)} />
            <g transform={format!("translate(6, {})", props.height / 2.0)}>
                <g transform="rotate(270 0 0)">
                    <text class="label">{props.name.clone()}</text>
                </g>
            </g>
            <g transform="translate(12, 0)">
                <polyline
                fill="none"
                stroke-width="1"
                points={points.join(" ")}
                />
            </g>
        </g>
    )
}

#[component]
fn DitherPattern() -> Html {
    html!(
        <pattern id="dither" width="1" height="1" patternUnits="userSpaceOnUse">
            <circle cx="1" cy="1" r="0.5" fill="currentColor" opacity="0.5" />
        </pattern>
    )
}
