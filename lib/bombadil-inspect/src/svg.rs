use yew::{Html, component, html};

#[component]
pub fn DitherPattern() -> Html {
    html!(
        <pattern id="dither" width="1.5" height="1.5" patternUnits="userSpaceOnUse">
                <circle cx="1" cy="1" r=".5" opacity="0.2" fill="currentColor" />
        </pattern>
    )
}

#[component]
pub fn ViolationPattern() -> Html {
    html!(
        <pattern id="violation" width="1" height="2" patternUnits="userSpaceOnUse">
            <rect width="1" height="1" opacity="0.2" />
        </pattern>
    )
}
