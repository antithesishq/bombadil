use std::rc::Rc;

use bombadil_browser_keys::key_name;
use bombadil_schema::{Point, Time, TraceEntry};
use yew::component;
use yew::prelude::*;

use crate::container_size::use_container_size;
use crate::list_autoscroll::use_list_autoscroll;
use crate::time::Duration;

#[derive(PartialEq, Properties)]
pub struct ActionsListProps {
    pub trace: Rc<[TraceEntry]>,
    pub selected_index: usize,
    pub on_select: Callback<usize>,
}

#[component]
pub fn ActionsList(props: &ActionsListProps) -> Html {
    let test_start =
        props.trace.first().expect("no first trace entry").timestamp;
    let list_ref = use_list_autoscroll(props.selected_index);

    html!(
        <ol ref={list_ref}>
        {
            props.trace.iter().enumerate().map(|(i, entry)| {
                html!(
                    <ActionEntry
                        entry={Rc::new(entry.clone())}
                        is_selected={i == props.selected_index}
                            test_start={test_start}
                            index={i}
                            on_select={&props.on_select} />
                )
            }).collect::<Html>()
        }
        </ol>
    )
}

#[derive(PartialEq, Properties)]
struct HistoryEntryProps {
    pub test_start: Time,
    pub entry: Rc<TraceEntry>,
    pub index: usize,
    pub is_selected: bool,
    pub on_select: Callback<usize>,
}

#[component]
fn ActionEntry(props: &HistoryEntryProps) -> Html {
    let (container_ref, container_size) = use_container_size();

    let (action_header, details): (Html, Option<Vec<(&str, String)>>) =
        match &props.entry.action {
            Some(action) => match action {
                bombadil_schema::BrowserAction::Back => {
                    (html!(<span class="action-name">{"Back"}</span>), None)
                }
                bombadil_schema::BrowserAction::Forward => {
                    (html!(<span class="action-name">{"Forward"}</span>), None)
                }
                bombadil_schema::BrowserAction::Click {
                    point,
                    name,
                    content,
                } => (
                    html!(
                        <>
                            <span class="action-name">{"Click"}</span>
                            <span class="element-tag">
                                {"<"}<span class="element-name">{name}</span>{" />"}
                            </span>
                        </>
                    ),
                    Some(vec![
                        ("Position", format_point(point)),
                        (
                            "Content",
                            format!(
                                "{:?}",
                                content.clone().unwrap_or("".into())
                            ),
                        ),
                    ]),
                ),
                bombadil_schema::BrowserAction::DoubleClick {
                    point,
                    delay_millis,
                    name,
                    content,
                } => (
                    html!(
                        <>
                            <span class="action-name">{"Double-click"}</span>
                            <span class="element-tag">
                                {"<"}<span class="element-name">{name}</span>{" />"}
                            </span>
                        </>
                    ),
                    Some(vec![
                        ("Position", format_point(point)),
                        ("Delay", format!("{}ms", delay_millis)),
                        (
                            "Content",
                            format!(
                                "{:?}",
                                content.clone().unwrap_or("".into())
                            ),
                        ),
                    ]),
                ),
                bombadil_schema::BrowserAction::TypeText {
                    text,
                    delay_millis,
                } => (
                    html!(
                        <>
                            <span class="action-name">{"Type"}</span>
                            <span class="text">{format!("{text:?}")}</span>
                        </>
                    ),
                    Some(vec![
                        ("Text", format!("{text:?}")),
                        ("Delay", delay_millis.to_string()),
                    ]),
                ),
                bombadil_schema::BrowserAction::PressKey { code, .. } => (
                    html!(
                        <>
                            <span class="action-name">{"Press"}</span>
                            <span>{key_name(*code).unwrap_or("Unknown")}</span>
                        </>
                    ),
                    Some(vec![("Code", code.to_string())]),
                ),
                bombadil_schema::BrowserAction::ScrollUp {
                    origin,
                    distance,
                } => (
                    html!(<span class="action-name">{"Scroll up"}</span>),
                    Some(vec![
                        ("Origin", format_point(origin)),
                        ("Distance", format!("{}px", distance)),
                    ]),
                ),
                bombadil_schema::BrowserAction::ScrollDown {
                    origin,
                    distance,
                } => (
                    html!(<span class="action-name">{"Scroll down"}</span>),
                    Some(vec![
                        ("Origin", format_point(origin)),
                        ("Distance", format!("{}px", distance)),
                    ]),
                ),
                bombadil_schema::BrowserAction::Reload => {
                    (html!(<span class="action-name">{"Reload"}</span>), None)
                }
                bombadil_schema::BrowserAction::Wait => {
                    (html!(<span class="action-name">{"Wait"}</span>), None)
                }
            },
            None => return html! {},
        };
    let li_class = if props.is_selected { "selected" } else { "" };
    let duration_since_start = props
        .entry
        .timestamp
        .duration_since(props.test_start)
        .unwrap_or_default();

    let index: usize = props.index;
    let on_select = props.on_select.clone();
    let on_click = move |_| on_select.emit(index);

    html! {
        <li class={li_class}>
            <button onclick={on_click} ref={container_ref}>
                {
                    if props.is_selected && let Some((width, height)) = container_size {
                        html!(
                            <svg class="background" xmlns="http://www.w3.org/2000/svg">
                                <rect width={width.to_string()} height={height.to_string()} fill="url(#dither)" />
                            </svg>
                        )
                    } else {
                        html!()
                    }
                }
                <header>
                    <div class="action-header">{action_header}</div>
                    <Duration value={duration_since_start} include_millis={true} />
                </header>
                {if let Some(details) = details && props.is_selected {
                    html!(
                        <table class="details">
                        {details.iter().map(|(name, value)| {
                            html!(
                                <tr>
                                    <th>{name}</th>
                                    <td>{value}</td>
                                </tr>
                            )
                        }).collect::<Html>()}
                        </table>

                    )
                } else { Html::default() }}
            </button>
        </li>
    }
}

fn format_point(point: &Point) -> String {
    format!("{:.1}, {:.1}", point.x, point.y)
}
