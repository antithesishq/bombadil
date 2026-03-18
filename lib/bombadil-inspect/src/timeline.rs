use std::rc::Rc;
use std::time::Duration;
use std::time::SystemTime;

use bombadil_inspect_api::PropertyViolation;
use bombadil_inspect_api::TraceEntry;
use yew::component;
use yew::prelude::*;

use crate::container_size::use_container_size;
use crate::duration::format_duration;

const SPACING_LEFT: f64 = 24.0;
const SPACING_RIGHT: f64 = 32.0;
const SPACING_Y: f64 = 6.0;
const TIMESCALE_VIOLATIONS_HEIGHT: f64 = 20.0;
const TIMESCALE_TICK_HEIGHT: f64 = 4.0;
const TIMESCALE_TEXT_HEIGHT: f64 = 9.0;
const TIMESCALE_AXIS_HEIGHT: f64 =
    TIMESCALE_TICK_HEIGHT * 2.0 + TIMESCALE_TEXT_HEIGHT;
const TIMESCALE_HEIGHT: f64 =
    TIMESCALE_VIOLATIONS_HEIGHT + TIMESCALE_AXIS_HEIGHT;

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
    let mut series_violations: Vec<(f64, Rc<[PropertyViolation]>)> =
        Vec::with_capacity(props.entries.len());
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

            series_violations.push((x, entry.violations.clone().into()));
        }
    };
    let series_heap: Series = series_heap.into();
    let series_cpu: Series = series_cpu.into();
    let series_violations: Series<Rc<[PropertyViolation]>> =
        series_violations.into();

    let print_y_bytes = Callback::from(move |y: f64| format_bytes(y as u64));
    let print_y_percent =
        Callback::from(move |y: f64| format!("{:.0}%", y * 100.0));

    let component_inner = if let Some((width, height)) = container_size {
        let chart_count = 2;
        let spacing_y_total = SPACING_Y * ((chart_count + 2) as f64);
        let chart_height =
            (height - spacing_y_total - TIMESCALE_HEIGHT) / chart_count as f64;
        assert_eq!(
            (spacing_y_total
                + chart_height * chart_count as f64
                + TIMESCALE_HEIGHT) as u64,
            height as u64
        );
        html!(
            <svg class="timeline" viewBox={format!("0 0 {width} {height}")} xmlns="http://www.w3.org/2000/svg" >
                <defs>
                    <DitherPattern />
                    <ViolationPattern />
                </defs>

                <g transform={format!("translate(0, {})", SPACING_Y)}>
                    <LineChart
                        name="Heap"
                        width={width}
                        height={chart_height}
                        series={series_heap}
                        print_y={print_y_bytes} />
                </g>

                <g transform={format!("translate(0, {})", SPACING_Y * 2.0 + chart_height)}>
                    <LineChart
                        name="CPU"
                        width={width}
                        height={chart_height}
                        series={series_cpu}
                        print_y={print_y_percent}
                        y_max={1.0}
                        />
                </g>

                <g transform={format!("translate(0, {})", SPACING_Y * 3.0 + chart_height * 2.0)}>
                <Timescale
                    width={width}
                    height={TIMESCALE_HEIGHT}
                    series={series_violations}
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

type Series<T = f64> = Rc<[(f64, T)]>;

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

    let spacing_ticks = 4.0;
    let line_width = props.width - SPACING_LEFT - SPACING_RIGHT;

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
            <polyline class="border" points={format!("{left},0 {left},{bottom} {right},{bottom} {right},0 {left},0", bottom=props.height, right=line_width + SPACING_LEFT, left=SPACING_LEFT)} />
            <g transform={format!("translate({left}, {top})", left=SPACING_LEFT / 2.0, top=props.height / 2.0)}>
                <g transform="rotate(270 0 0)">
                    <text class="label">{props.name.clone()}</text>
                </g>
            </g>
            <g transform={format!("translate({left}, {top})", left=line_width + SPACING_LEFT + spacing_ticks, top=0)}>
                <text class="tick-label max">{props.print_y.emit(y_max)}</text>
            </g>
            <g transform={format!("translate({left}, {top})", left=line_width + SPACING_LEFT + spacing_ticks, top=props.height)}>
                <text class="tick-label min">{props.print_y.emit(0.0)}</text>
            </g>
            <g transform={format!("translate({left}, 0)", left=SPACING_LEFT)}>
                <polyline
                fill="none"
                stroke-width="1"
                points={points.join(" ")}
                />
            </g>
        </g>
    )
}

#[derive(PartialEq, Properties)]
pub struct TimescaleProps {
    series: Series<Rc<[PropertyViolation]>>,
    width: f64,
    height: f64,
}

#[component]
pub fn Timescale(props: &TimescaleProps) -> Html {
    let scale_width = props.width - SPACING_LEFT - SPACING_RIGHT;

    let x_max = if let Some(x) =
        props.series.iter().map(|(x, _)| *x).reduce(f64::max)
    {
        x
    } else {
        return html!();
    };
    html!(
    <g class="timescale" transform={format!("translate({SPACING_LEFT}, 0)")}>
        <g>
            <polyline class="border" points={format!(" 0,{top} {right},{top} ", top=TIMESCALE_VIOLATIONS_HEIGHT, right=scale_width)} />
            {
                [0.0, 0.25, 0.5, 0.75, 1.0].iter().map(|tick| {
                    let x = tick * scale_width;
                    html!(
                        <>
                            <polyline class="border" points={format!(" {x},{top} {x},{bottom} ", top=TIMESCALE_VIOLATIONS_HEIGHT, bottom=TIMESCALE_VIOLATIONS_HEIGHT + TIMESCALE_TICK_HEIGHT)} />
                            // TODO: pass in Durations rather than f64 for time
                            <text class="time-label" x={format!("{x}")} y={format!("{top}", top=TIMESCALE_VIOLATIONS_HEIGHT + TIMESCALE_TICK_HEIGHT * 2.0 + TIMESCALE_TEXT_HEIGHT / 2.0)}>{format_duration(Duration::from_millis((x_max * tick) as u64))}</text>
                        </>
                    )
                }).collect::<Html>()
            }
        </g>
        <g>
        {
            props.series.iter().map(|(x, violations)| {
                if violations.is_empty() {
                    html!()
                } else {
                    html!(
                        <g class="violation" transform={format!("translate({}, {})", (x / x_max) * scale_width, TIMESCALE_VIOLATIONS_HEIGHT / 2.0)}>
                            <title>{format!("{} violations in state", violations.len())}</title>
                            <rect x="-6" y="-6" width="12" height="12" fill="url(#violation)" />
                            <text x="0" y="0">{"!"}</text>
                        </g>
                    )
                }
            }).collect::<Html>()
        }
        </g>
    </g>
    )
}

#[component]
fn DitherPattern() -> Html {
    html!(
        <pattern id="dither" width="1" height="1" patternUnits="userSpaceOnUse">
                <circle cx="1" cy="1" r=".5" opacity="0.2" fill="currentColor" />
        </pattern>
    )
}

#[component]
fn ViolationPattern() -> Html {
    html!(
        <pattern id="violation" width="1" height="2" patternUnits="userSpaceOnUse">
            <rect width="1" height="1" opacity="0.2" />
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
