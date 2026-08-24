use std::cmp::Ordering;
use std::rc::Rc;
use std::time::Duration;

use bombadil_schema::browser::BrowserTraceEntry;
use bombadil_schema::{PropertyViolation, Time};
use yew::component;
use yew::prelude::*;

use crate::container_size::use_container_size;
use crate::duration::FormatDurationOptions;
use crate::duration::format_duration;

const SPACING_LEFT: f64 = 24.0;
const SPACING_RIGHT: f64 = 32.0;
const SPACING_Y: f64 = 6.0;
const MIN_ZOOM_SELECTION_WIDTH: f64 = 5.0;
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
    pub entries: Rc<[BrowserTraceEntry]>,
    pub test_start: Time,
    pub selected_index: usize,
    pub on_select: Callback<usize>,
}

pub struct TimelineData {
    series_heap: Series,
    series_cpu: Series,
    series_violations: Rc<[(f64, Rc<[PropertyViolation]>)]>,
    series_index_and_time: Rc<[(usize, f64)]>,
    x_max: Option<f64>,
}

#[component]
pub fn Timeline(
    TimelineProps {
        entries,
        test_start,
        selected_index,
        on_select,
    }: &TimelineProps,
) -> Html {
    let (container_ref, container_size) = use_container_size();
    let drag_start = use_mut_ref(|| None::<(f64, bool)>);
    let drag_range = use_state(|| None::<(f64, f64)>);
    let zoom_range = use_state(|| None::<(f64, f64)>);

    let data: Rc<TimelineData> = use_memo(
        (entries.clone(), *test_start),
        move |(entries, test_start)| {
            let mut series_heap = Vec::with_capacity(entries.len());
            let mut series_cpu = Vec::with_capacity(entries.len());
            let mut series_violations: Vec<(f64, Rc<[PropertyViolation]>)> =
                Vec::with_capacity(entries.len());
            {
                for (i, entry) in entries.iter().enumerate() {
                    let x = entry
                        .timestamp
                        .duration_since(*test_start)
                        .expect("couldn't calculate offset time")
                        .as_micros() as f64;
                    series_heap
                        .push((x, entry.state.resources.js_heap_used as f64));

                    let cpu = if i > 0
                        && let Some(entry_previous) = entries.get(i - 1)
                    {
                        let wall = entry.state.resources.timestamp
                            - entry_previous.state.resources.timestamp;
                        if wall <= 0.0 {
                            0.0
                        } else {
                            let cpu = entry.state.resources.thread_time
                                - entry_previous.state.resources.thread_time;
                            (cpu / wall).clamp(0.0, 1.0)
                        }
                    } else {
                        0.0
                    };
                    series_cpu.push((x, cpu));

                    series_violations
                        .push((x, entry.violations.clone().into()));
                }
            };

            let series_index_and_time: Rc<[(usize, f64)]> = series_heap
                .iter()
                .map(|(x, _)| x)
                .copied()
                .enumerate()
                .collect();

            let x_max = if let Some(x) =
                series_heap.iter().map(|(x, _)| *x).reduce(f64::max)
                && x > 0.0
            {
                Some(x)
            } else {
                None
            };

            TimelineData {
                series_heap: series_heap.into(),
                series_cpu: series_cpu.into(),
                series_violations: series_violations.into(),
                series_index_and_time,
                x_max,
            }
        },
    );

    let (time_before, time_after) = if *selected_index > 0
        && let (Some((_, before)), Some((_, after))) = (
            data.series_index_and_time.get(selected_index - 1),
            data.series_index_and_time.get(*selected_index),
        ) {
        (*before, *after)
    } else {
        return html!();
    };

    let Some(x_max) = data.x_max else {
        return html!();
    };
    let (x_start, x_end) = (*zoom_range).unwrap_or((0.0, x_max));

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

        let series_index_and_time = data.series_index_and_time.clone();
        let on_select = on_select.clone();
        let select_at_x = Callback::from(move |x: f64| {
            let click_x_axis = x_start
                + ((x.clamp(SPACING_LEFT, width - SPACING_RIGHT)
                    - SPACING_LEFT)
                    / axis_x_width)
                    * (x_end - x_start);

            let windows: Vec<&[(usize, f64); 2]> =
                series_index_and_time.array_windows().collect();

            let index = windows.binary_search_by(|[(_, start), (_, end)]| {
                if *end <= click_x_axis {
                    Ordering::Less
                } else if *start > click_x_axis {
                    Ordering::Greater
                } else {
                    Ordering::Equal
                }
            });

            match index {
                Ok(index) => on_select.emit(index + 1),
                Err(_) => on_select.emit(series_index_and_time.len() - 1),
            }
        });
        let on_mouse_down = {
            let drag_start = drag_start.clone();
            let drag_range = drag_range.clone();
            Callback::from(move |event: MouseEvent| {
                *drag_start.borrow_mut() =
                    Some((f64::from(event.client_x()), event.shift_key()));
                drag_range.set(None);
            })
        };
        let on_mouse_move = {
            let drag_start = drag_start.clone();
            let drag_range = drag_range.clone();
            let select_at_x = select_at_x.clone();
            Callback::from(move |event: MouseEvent| {
                if let Some((start, zooming)) = *drag_start.borrow()
                    && zooming
                {
                    let end = f64::from(event.client_x());
                    if (end - start).abs() >= MIN_ZOOM_SELECTION_WIDTH {
                        drag_range.set(Some((start, end)));
                    }
                } else if event.buttons() != 0 {
                    select_at_x.emit(f64::from(event.client_x()));
                }
            })
        };
        let on_mouse_up = {
            let drag_start = drag_start.clone();
            let drag_range = drag_range.clone();
            let zoom_range = zoom_range.clone();
            Callback::from(move |event: MouseEvent| {
                let Some((start, zooming)) = drag_start.borrow_mut().take()
                else {
                    return;
                };
                let end = f64::from(event.client_x());
                drag_range.set(None);

                if !zooming || (end - start).abs() < MIN_ZOOM_SELECTION_WIDTH {
                    select_at_x.emit(end);
                    return;
                }

                let to_time = |x: f64| {
                    x_start
                        + ((x.clamp(SPACING_LEFT, width - SPACING_RIGHT)
                            - SPACING_LEFT)
                            / axis_x_width)
                            * (x_end - x_start)
                };
                let start_time = to_time(start);
                let end_time = to_time(end);
                zoom_range.set(Some((
                    start_time.min(end_time),
                    start_time.max(end_time),
                )));
            })
        };
        let on_mouse_leave = {
            let drag_start = drag_start.clone();
            let drag_range = drag_range.clone();
            Callback::from(move |_: MouseEvent| {
                *drag_start.borrow_mut() = None;
                drag_range.set(None);
            })
        };

        let heap_y_max = if let Some(y) =
            data.series_heap.iter().map(|(_, y)| *y).reduce(f64::max)
            && y > 0.0
        {
            let unit = if y >= T {
                T
            } else if y >= G {
                G
            } else if y >= M {
                M
            } else if y >= K {
                K
            } else {
                1.0
            };

            let value_in_unit = y / unit;
            let order = 10_f64.powf(value_in_unit.log10().floor()).max(10.0);
            let normalized = value_in_unit / order;
            let rounded = if normalized <= 1.0 {
                order
            } else if normalized <= 5.0 {
                5.0 * order
            } else {
                10.0 * order
            };
            Some((rounded * unit).max(M))
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
                onmouseleave={on_mouse_leave}
                onmouseup={on_mouse_up}
            >
                <g transform={format!("translate(0, {})", SPACING_Y)}>
                    <LineChart
                        name="Heap"
                        width={width}
                        height={chart_height}
                        series={data.series_heap.clone()}
                        x_start={x_start}
                        x_end={x_end}
                        y_max={heap_y_max}
                        print_y={print_y_bytes} />
                </g>

                <g transform={format!("translate(0, {})", SPACING_Y * 2.0 + chart_height)}>
                    <LineChart
                        name="CPU"
                        width={width}
                        height={chart_height}
                        series={data.series_cpu.clone()}
                        print_y={print_y_percent}
                        x_start={x_start}
                        x_end={x_end}
                        y_max={1.0}
                        />
                </g>

                <g transform={format!("translate(0, {})", SPACING_Y * 3.0 + chart_height * 2.0)}>
                    <Timescale
                        width={width}
                        height={TIMESCALE_HEIGHT}
                        series={data.series_violations.clone()}
                        x_start={x_start}
                        x_end={x_end}
                        />
                </g>

                {if let Some((start, end)) = *drag_range {
                    let start = start.clamp(SPACING_LEFT, width - SPACING_RIGHT);
                    let end = end.clamp(SPACING_LEFT, width - SPACING_RIGHT);
                    html!(<rect
                        class="zoom-selection"
                        x={start.min(end).to_string()}
                        y="0"
                        width={(end - start).abs().to_string()}
                        height={height.to_string()}
                    />)
                } else {
                    html!()
                }}

                {if time_after >= x_start && time_before <= x_end {
                    let start = time_before.max(x_start);
                    let end = time_after.min(x_end);
                    let left = scale_x(start, x_start, x_end, axis_x_width);
                    let right = scale_x(end, x_start, x_end, axis_x_width);
                    let cursor_width = right - left;
                    let cursor_height = height - TIMESCALE_AXIS_HEIGHT - SPACING_Y;
                    html!(
                        <g transform={format!("translate({}, 0)", SPACING_LEFT + left)} class="cursor">
                            <line x1="0" y1="0" x2="0" y2={cursor_height.to_string()} />
                            <line x1={cursor_width.to_string()} y1="0" x2={cursor_width.to_string()} y2={cursor_height.to_string()} />
                            <rect
                                x="0"
                                y="0"
                                width={cursor_width.to_string()}
                                height={cursor_height.to_string()}
                                fill="url(#dither)"
                                />
                        </g>
                    )
                } else {
                    html!()
                }}

            </svg>
        )
    } else {
        html!()
    };

    let reset_zoom = {
        let zoom_range = zoom_range.clone();
        Callback::from(move |_| zoom_range.set(None))
    };

    html!(
        <div class="timeline" ref={container_ref}>
            {if zoom_range.is_some() {
                html!(<button
                    type="button"
                    class="reset-zoom"
                    onclick={reset_zoom}
                >
                    {"Reset zoom"}
                </button>)
            } else {
                html!()
            }}
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
    x_start: f64,
    x_end: f64,
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
        for (x, y) in props
            .series
            .iter()
            .filter(|(x, _)| *x >= props.x_start && *x <= props.x_end)
        {
            let x = scale_x(*x, props.x_start, props.x_end, line_width);
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
    x_start: f64,
    x_end: f64,
}

#[component]
pub fn Timescale(props: &TimescaleProps) -> Html {
    let scale_width = props.width - SPACING_LEFT - SPACING_RIGHT;
    let visible_duration = props.x_end - props.x_start;
    let include_millis = visible_duration < 10_000_000.0;
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
                            <text
                                class="time-label"
                                x={format!("{x}")}
                                y={format!("{top}", top=TIMESCALE_VIOLATIONS_HEIGHT + TIMESCALE_TICK_HEIGHT * 2.0 + TIMESCALE_TEXT_HEIGHT / 2.0)}>
                                {format_duration(
                                    Duration::from_micros((props.x_start + visible_duration * tick) as u64),
                                    FormatDurationOptions { include_millis },
                                )}
                            </text>
                        </>
                    )
                }).collect::<Html>()
            }
        </g>
        <g>
        {
            props.series.iter().map(|(x, violations)| {
                if violations.is_empty() || *x < props.x_start || *x > props.x_end {
                    html!()
                } else {
                    html!(
                        <g class="violation" transform={format!("translate({}, {})", scale_x(*x, props.x_start, props.x_end, scale_width), TIMESCALE_VIOLATIONS_HEIGHT / 2.0)}>
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

fn scale_x(x: f64, x_start: f64, x_end: f64, width: f64) -> f64 {
    ((x - x_start) / (x_end - x_start)) * width
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
