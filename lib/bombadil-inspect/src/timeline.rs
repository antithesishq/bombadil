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

    let mut series_heap = Vec::with_capacity(props.entries.len());
    let mut series_cpu = Vec::with_capacity(props.entries.len());
    {
        for (i, entry) in props.entries.iter().enumerate() {
            let x = entry
                .timestamp
                .duration_since(props.test_start)
                .expect("couldn't calculate offset time")
                .as_millis() as f64;
            series_heap.push((x, entry.resources.js_heap_used as f64));

            let cpu = if i > 0
                && let Some(entry_previous) = props.entries.get(i - 1)
            {
                let wall = entry.resources.timestamp
                    - entry_previous.resources.timestamp;
                if wall <= 0.0 {
                    0.0
                } else {
                    let cpu = entry.resources.thread_time
                        - entry_previous.resources.thread_time;
                    (cpu / wall).clamp(0.0, 1.0)
                }
            } else {
                0.0
            };
            series_cpu.push((x, cpu));
        }
    };
    let series_heap: Series = series_heap.into();
    let series_cpu: Series = series_cpu.into();

    let print_y_bytes = Callback::from(move |y: f64| format_bytes(y as u64));
    let print_y_percent =
        Callback::from(move |y: f64| format!("{:.0}%", y * 100.0));

    let component_inner = if let Some((width, height)) = container_size {
        let panel_count = 3;
        let padding_y = 6.0;
        let panel_height = (height - padding_y) / panel_count as f64;
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

                <g transform={format!("translate(0, {})", padding_y)}>
                    <LineChart
                        name="Heap"
                        width={width}
                        height={panel_height}
                        series={series_heap}
                        print_y={print_y_bytes} />
                </g>

                <g transform={format!("translate(0, {})", padding_y * 2.0 + panel_height)}>
                    <LineChart
                        name="CPU"
                        width={width}
                        height={panel_height}
                        series={series_cpu}
                        print_y={print_y_percent}
                        y_max={1.0}
                        />
                </g>

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
    #[prop_or_default]
    y_max: Option<f64>,
}

#[component]
pub fn LineChart(props: &LineChartProps) -> Html {
    let (x_max, mut y_max) = if let Some((x, y)) = props
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

    if let Some(y) = props.y_max {
        y_max = y;
    }

    let padding_left = 18.0;
    let padding_right = 30.0;
    let spacing_ticks = 4.0;
    let line_width = props.width - padding_left - padding_right;

    let points = {
        let mut points = vec![];
        for (x, y) in props.series.iter() {
            let x = (x / x_max) * line_width;
            let y = props.height - ((y / y_max) * props.height);
            points.push(format!("{x},{y}"))
        }
        points
    };

    html!(
        <g class="line-chart">
            <polyline class="border" points={format!("{left},0 {left},{bottom} {right},{bottom} {right},0 {left},0", bottom=props.height, right=line_width + padding_left, left=padding_left)} />
            <g transform={format!("translate({left}, {top})", left=padding_left / 2.0, top=props.height / 2.0)}>
                <g transform="rotate(270 0 0)">
                    <text class="label">{props.name.clone()}</text>
                </g>
            </g>
            <g transform={format!("translate({left}, {top})", left=line_width + padding_left + spacing_ticks, top=0)}>
                <text class="tick-label max">{props.print_y.emit(y_max)}</text>
            </g>
            <g transform={format!("translate({left}, {top})", left=line_width + padding_left + spacing_ticks, top=props.height)}>
                <text class="tick-label min">{props.print_y.emit(0.0)}</text>
            </g>
            <g transform={format!("translate({left}, 0)", left=padding_left)}>
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

fn format_bytes(bytes: u64) -> String {
    const G: f64 = 1_073_741_824.0;
    const M: f64 = 1_048_576.0;
    const K: f64 = 1_024.0;

    let b = bytes as f64;
    let (val, suffix) = if b >= G {
        (b / G, "G")
    } else if b >= M {
        (b / M, "M")
    } else if b >= K {
        (b / K, "K")
    } else {
        return format!("{bytes}B");
    };

    if val >= 10.0 {
        format!("{}{}", val as u64, suffix)
    } else {
        format!(".{}{}", (val * 10.0) as u64 % 10, suffix)
    }
}
