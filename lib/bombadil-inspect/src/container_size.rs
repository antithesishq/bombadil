use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use yew::prelude::*;

#[hook]
pub fn use_container_size() -> (NodeRef, Option<(f64, f64)>) {
    let container_ref = use_node_ref();
    let container_size = use_state(|| None::<(f64, f64)>);

    {
        let container_ref = container_ref.clone();
        let container_size = container_size.clone();
        use_effect_with((), move |_| {
            let state =
                container_ref.cast::<web_sys::Element>().map(|element| {
                    let closure = Closure::<dyn FnMut(js_sys::Array)>::new(
                        move |entries: js_sys::Array| {
                            if let Some(entry) = entries
                                .get(0)
                                .dyn_ref::<web_sys::ResizeObserverEntry>(
                            ) {
                                let rect = entry.content_rect();
                                container_size
                                    .set(Some((rect.width(), rect.height())));
                            }
                        },
                    );
                    let observer = web_sys::ResizeObserver::new(
                        closure.as_ref().unchecked_ref(),
                    )
                    .unwrap();
                    observer.observe(&element);
                    (observer, closure)
                });
            move || {
                if let Some((observer, _closure)) = state {
                    observer.disconnect();
                }
            }
        });
    }

    (container_ref, *container_size)
}
