use std::time;

use yew::component;
use yew::prelude::*;

use crate::duration::FormatDurationOptions;
use crate::duration::format_duration;

#[derive(PartialEq, Properties)]
pub struct DurationProps {
    pub value: time::Duration,
    #[prop_or_default]
    pub include_millis: bool,
}

#[component]
pub fn Duration(props: &DurationProps) -> Html {
    let options = FormatDurationOptions {
        include_millis: props.include_millis,
    };
    html! {
        <time title={format!("{:?}", props.value)}>{format_duration(props.value, options)}</time>
    }
}
