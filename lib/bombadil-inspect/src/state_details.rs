use std::rc::Rc;

use bombadil_inspect_api::TraceEntry;
use serde_json as json;
use yew::component;
use yew::prelude::*;

#[derive(PartialEq, Properties)]
pub struct StateDetailsProps {
    pub entry: Rc<TraceEntry>,
}

#[component]
pub fn StateDetails(props: &StateDetailsProps) -> Html {
    html!(
        <details>
            <summary>{"Snapshots"}</summary>
            <table>
            {
                props
                    .entry
                    .snapshots
                    .iter()
                    .map(|snapshot| html!(<tr><th>{snapshot.name.clone()}</th><td>{json::to_string_pretty(&snapshot.value).unwrap_or("invalid json".into())}</td></tr>))
                    .collect::<Html>()
            }
            </table>
        </details>
    )
}
