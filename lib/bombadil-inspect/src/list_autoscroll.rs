use gloo_timers::callback::Timeout;
use wasm_bindgen::JsCast;
use wasm_bindgen::JsValue;
use yew::prelude::*;

#[hook]
pub fn use_list_autoscroll(selected_index: usize) -> NodeRef {
    let list_ref = use_node_ref();

    {
        let list_ref = list_ref.clone();
        use_effect_with(selected_index, move |_| {
            let timeout = Timeout::new(0, move || {
                if let Some(list) = list_ref.cast::<web_sys::HtmlElement>()
                    && let Some(container) =
                        list.parent_element().and_then(|node| {
                            node.dyn_into::<web_sys::HtmlElement>().ok()
                        })
                    && let Some(selected_item) =
                        list.children().item(selected_index as u32).and_then(
                            |node| node.dyn_into::<web_sys::HtmlElement>().ok(),
                        )
                {
                    let container_js: &JsValue = container.as_ref();
                    let item_js: &JsValue = selected_item.as_ref();

                    if let Ok(get_rect) = js_sys::Reflect::get(
                        container_js,
                        &JsValue::from_str("getBoundingClientRect"),
                    ) && let Ok(func) =
                        get_rect.dyn_into::<js_sys::Function>()
                        && let Ok(container_rect_js) = func.call0(container_js)
                        && let Ok(item_rect_func) = js_sys::Reflect::get(
                            item_js,
                            &JsValue::from_str("getBoundingClientRect"),
                        )
                        .and_then(|f| f.dyn_into::<js_sys::Function>())
                        && let Ok(item_rect_js) = item_rect_func.call0(item_js)
                    {
                        let container_top = js_sys::Reflect::get(
                            &container_rect_js,
                            &JsValue::from_str("top"),
                        )
                        .ok()
                        .and_then(|v| v.as_f64())
                        .unwrap_or(0.0);
                        let container_bottom = js_sys::Reflect::get(
                            &container_rect_js,
                            &JsValue::from_str("bottom"),
                        )
                        .ok()
                        .and_then(|v| v.as_f64())
                        .unwrap_or(0.0);
                        let container_height = js_sys::Reflect::get(
                            &container_rect_js,
                            &JsValue::from_str("height"),
                        )
                        .ok()
                        .and_then(|v| v.as_f64())
                        .unwrap_or(0.0);

                        let item_top = js_sys::Reflect::get(
                            &item_rect_js,
                            &JsValue::from_str("top"),
                        )
                        .ok()
                        .and_then(|v| v.as_f64())
                        .unwrap_or(0.0);
                        let item_bottom = js_sys::Reflect::get(
                            &item_rect_js,
                            &JsValue::from_str("bottom"),
                        )
                        .ok()
                        .and_then(|v| v.as_f64())
                        .unwrap_or(0.0);
                        let item_height = js_sys::Reflect::get(
                            &item_rect_js,
                            &JsValue::from_str("height"),
                        )
                        .ok()
                        .and_then(|v| v.as_f64())
                        .unwrap_or(0.0);

                        let is_fully_visible = item_top >= container_top
                            && item_bottom <= container_bottom;

                        if !is_fully_visible {
                            let item_center = item_top + (item_height / 2.0);
                            let container_center =
                                container_top + (container_height / 2.0);

                            let scroll_offset = item_center - container_center;
                            let new_scroll_top =
                                container.scroll_top() as f64 + scroll_offset;

                            container.set_scroll_top(new_scroll_top as i32);
                        }
                    }
                }
            });
            timeout.forget();
        });
    }

    list_ref
}
