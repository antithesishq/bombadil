use std::rc::Rc;
use std::time::Duration;
use std::time::SystemTime;

use bombadil_schema::PropertyViolation;
use bombadil_schema::TraceEntry;
use yew::component;
use yew::prelude::*;

use crate::container_size::use_container_size;
use crate::duration::format_duration;

const SPACING_LEFT: f64 = 24.0;
const SPACING_RIGHT: f64 = 32.0;
const SPACING_Y: f64 = 6.0;
const TIMESCALE_VIOLATIONS_HEIGHT: f64 = 22.0;
const TIMESCALE_TICK_HEIGHT: f64 = 4.0;
const TIMESCALE_TEXT_HEIGHT: f64 = 9.0;
const TIMESCALE_AXIS_HEIGHT: f64 =
    TIMESCALE_TICK_HEIGHT * 2.0 + TIMESCALE_TEXT_HEIGHT;
const TIMESCALE_HEIGHT: f64 =
    TIMESCALE_VIOLATIONS_HEIGHT + TIMESCALE_AXIS_HEIGHT;

const K: f64 = 1_024.0;
const M: f64 = K * K;
const G: f64 = K * K * K;
const T: f64 = K * K * K * K;

#[derive(PartialEq, Properties)]
pub struct TimelineProps {
    pub entries: Rc<[TraceEntry]>,
    pub test_start: SystemTime,
    pub selected_index: usize,
    pub on_select: Callback<usize>,
}

#[component]
pub fn Timeline(props: &TimelineProps) -> Html {
    let (container_ref, container_size) = use_container_size();
    let is_mouse_down = use_mut_ref(|| false);

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

    let series_index_and_time: Rc<[(usize, f64)]> = series_heap
        .iter()
        .map(|(x, _)| x)
        .copied()
        .enumerate()
        .collect();

    let (time_before, time_after) = {
        if props.selected_index > 0
            && let (Some((_, before)), Some((_, after))) = (
                &series_index_and_time.get(props.selected_index - 1),
                &series_index_and_time.get(props.selected_index),
            )
        {
            (*before, *after)
        } else {
            return html!();
        }
    };
    let x_max = if let Some(x) =
        series_heap.iter().map(|(x, _)| *x).reduce(f64::max)
        && x > 0.0
    {
        x
    } else {
        return html!();
    };

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
        let axis_x_width = width - SPACING_LEFT - SPACING_RIGHT;

        let series_index_and_time = series_index_and_time.clone();
        let on_select = props.on_select.clone();
        let select_at_x = Callback::from(move |x: u64| {
            let click_x_axis = ((x
                .clamp(SPACING_LEFT as u64, (width - SPACING_RIGHT) as u64)
                .saturating_sub(SPACING_LEFT as u64)
                as f64)
                / axis_x_width)
                * x_max;

            for (index, time) in series_index_and_time.iter() {
                if click_x_axis < *time {
                    on_select.emit(*index);
                    return;
                }
            }
            on_select.emit(series_index_and_time.len() - 1);
        });
        let on_mouse_down = {
            let select_at_x = select_at_x.clone();
            let is_mouse_down = is_mouse_down.clone();
            Callback::from(move |event: MouseEvent| {
                *is_mouse_down.borrow_mut() = true;
                select_at_x.emit(event.client_x() as u64);
            })
        };
        let on_mouse_move = {
            let select_at_x = select_at_x.clone();
            let is_mouse_down = is_mouse_down.clone();
            Callback::from(move |event: MouseEvent| {
                if *is_mouse_down.borrow() {
                    select_at_x.emit(event.client_x() as u64);
                }
            })
        };
        let on_mouse_up = {
            let is_mouse_down = is_mouse_down.clone();
            Callback::from(move |_: MouseEvent| {
                *is_mouse_down.borrow_mut() = false;
            })
        };

        let heap_y_max = if let Some(y) =
            series_heap.iter().map(|(_, y)| *y).reduce(f64::max)
            && y > 0.0
        {
            let (unit_divisor, unit_multiplier) = if y >= T {
                (T, T)
            } else if y >= G {
                (G, G)
            } else if y >= M {
                (M, M)
            } else if y >= K {
                (K, K)
            } else {
                (1.0, 1.0)
            };

            let value_in_unit = y / unit_divisor;
            let order = 10_f64.powf(value_in_unit.log10().floor()).max(10.0);
            let normalized = value_in_unit / order;
            let rounded = if normalized <= 1.0 {
                order
            } else if normalized <= 5.0 {
                5.0 * order
            } else {
                10.0 * order
            };
            Some((rounded * unit_multiplier).max(M))
        } else {
            None
        };

        html!(
            <svg
                class="timeline"
                viewBox={format!("0 0 {width} {height}")}
                xmlns="http://www.w3.org/2000/svg"
                onmousedown={on_mouse_down}
                onmousemove={on_mouse_move}
                onmouseleave={on_mouse_up.clone()}
                onmouseup={on_mouse_up}
            >
                <g transform={format!("translate(0, {})", SPACING_Y)}>
                    <LineChart
                        name="Heap"
                        width={width}
                        height={chart_height}
                        series={series_heap}
                        x_max={x_max}
                        y_max={heap_y_max}
                        print_y={print_y_bytes} />
                </g>

                <g transform={format!("translate(0, {})", SPACING_Y * 2.0 + chart_height)}>
                    <LineChart
                        name="CPU"
                        width={width}
                        height={chart_height}
                        series={series_cpu}
                        print_y={print_y_percent}
                        x_max={x_max}
                        y_max={1.0}
                        />
                </g>

                <g transform={format!("translate(0, {})", SPACING_Y * 3.0 + chart_height * 2.0)}>
                    <Timescale
                        width={width}
                        height={TIMESCALE_HEIGHT}
                        series={series_violations}
                        x_max={x_max}
                        />
                </g>

                {
                    {
                        let width = ((time_after - time_before) / x_max) * axis_x_width;
                        let height = height - TIMESCALE_AXIS_HEIGHT - SPACING_Y;
                        html!(
                            <g transform={format!("translate({}, 0)", SPACING_LEFT + (time_before / x_max) * axis_x_width)} class="cursor">
                                <line x1="0" y1="0" x2="0" y2={height.to_string()} />
                                <line x1={width.to_string()} y1="0" x2={width.to_string()} y2={height.to_string()} />
                                <rect
                                    x="0"
                                    y="0"
                                    width={width.to_string()}
                                    height={height.to_string()}
                                    fill="url(#dither)"
                                    />
                            </g>
                        )
                    }
                }

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
    x_max: f64,
    #[prop_or_default]
    y_max: Option<f64>,
}

#[component]
pub fn LineChart(props: &LineChartProps) -> Html {
    let mut y_max = if let Some(y) =
        props.series.iter().map(|(_, y)| *y).reduce(f64::max)
        && y > 0.0
    {
        y
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
            let x = (x / props.x_max) * line_width;
            let y = props.height - ((y / y_max) * props.height);
            points.push(format!("{x},{y}"))
        }
        points
    };

    html!(
        <g class="line-chart">
            <rect x={SPACING_LEFT.to_string()} y="0" width={line_width.to_string()} height={props.height.to_string()} class="background" />
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
    x_max: f64,
}

#[component]
pub fn Timescale(props: &TimescaleProps) -> Html {
    let scale_width = props.width - SPACING_LEFT - SPACING_RIGHT;
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
                            <text class="time-label" x={format!("{x}")} y={format!("{top}", top=TIMESCALE_VIOLATIONS_HEIGHT + TIMESCALE_TICK_HEIGHT * 2.0 + TIMESCALE_TEXT_HEIGHT / 2.0)}>{format_duration(Duration::from_millis((props.x_max * tick) as u64))}</text>
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
                        <g class="violation" transform={format!("translate({}, {})", (x / props.x_max) * scale_width, TIMESCALE_VIOLATIONS_HEIGHT / 2.0)}>
                            <title>{format!("{} violations in state", violations.len())}</title>
                            <rect x="-7" y="-7" width="14" height="14" class="background" />
                            <rect x="-7" y="-7" width="14" height="14" fill="url(#violation)" class="pattern" />
                            <text x="0" y="0" class="icon">{"!"}</text>
                        </g>
                    )
                }
            }).collect::<Html>()
        }
        </g>
    </g>
    )
}

fn format_bytes(size: u64) -> String {
    let size_float = size as f64;
    let (val, suffix) = if size_float >= T {
        (size_float / T, "T")
    } else if size_float >= G {
        (size_float / G, "G")
    } else if size_float >= M {
        (size_float / M, "M")
    } else if size_float >= K {
        (size_float / K, "K")
    } else {
        return format!("{size}B");
    };

    if val >= 10.0 {
        format!("{:.0}{}", val as u64, suffix)
    } else {
        format!(".{:.1}{}", (val * 10.0) as u64 % 10, suffix)
    }
}
