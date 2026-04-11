use std::cell::RefCell;
use std::rc::Rc;

use wasm_bindgen::JsCast;
use yew::prelude::*;

#[hook]
pub fn use_list_autoscroll(
    selected_index: usize,
    is_following: Rc<RefCell<bool>>,
    on_select: Callback<usize>,
) -> NodeRef {
    let list_ref = use_node_ref();
    let current_index = use_mut_ref(|| selected_index);
    *current_index.borrow_mut() = selected_index;

    // Scroll listener: update is_following based on scroll position.
    // When transitioning from following to not-following, snapshot the
    // current selected_index so the view stays put.
    {
        let list_ref = list_ref.clone();
        use_effect_with((), move |_| {
            let container = list_ref
                .cast::<web_sys::HtmlElement>()
                .and_then(|list| list.parent_element())
                .and_then(|el| el.dyn_into::<web_sys::HtmlElement>().ok());

            let listener = container.map(|container| {
                let scroll_container = container.clone();
                let closure = wasm_bindgen::closure::Closure::<dyn Fn()>::wrap(
                    Box::new(move || {
                        let at_bottom = scroll_container.scroll_top() as f64
                            + scroll_container.client_height() as f64
                            >= scroll_container.scroll_height() as f64 - 5.0;
                        let was_following = *is_following.borrow();
                        *is_following.borrow_mut() = at_bottom;
                        if was_following && !at_bottom {
                            on_select.emit(*current_index.borrow());
                        }
                    }),
                );

                let js_fn: js_sys::Function =
                    closure.as_ref().unchecked_ref::<js_sys::Function>().clone();
                let _ = container
                    .add_event_listener_with_callback("scroll", &js_fn);

                (container, js_fn, closure)
            });

            move || {
                if let Some((container, js_fn, _closure)) = listener {
                    let _ = container.remove_event_listener_with_callback(
                        "scroll", &js_fn,
                    );
                }
            }
        });
    }

    // Scroll selected item into view.
    {
        let list_ref = list_ref.clone();
        use_effect_with(selected_index, move |_| {
            if let Some(list) = list_ref.cast::<web_sys::HtmlElement>()
                && let Some(item) =
                    list.query_selector(".selected").ok().flatten()
            {
                let opts = web_sys::ScrollIntoViewOptions::new();
                opts.set_block(web_sys::ScrollLogicalPosition::Nearest);
                item.scroll_into_view_with_scroll_into_view_options(&opts);
            }
        });
    }

    list_ref
}
