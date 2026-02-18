use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use proptest::prelude::*;

use boa_engine::{
    Context, JsValue, Module, NativeFunction, context::ContextBuilder,
    js_string, object::builtins::JsUint8Array, property::PropertyKey,
};

use crate::specification::js;
use crate::specification::module_loader::{
    HybridModuleLoader, load_bombadil_module, load_modules,
};

thread_local! {
    static RANDOM_BYTES: RefCell<VecDeque<u8>> = const { RefCell::new(VecDeque::new()) };
}

fn load_random_module(random_bytes: Vec<u8>) -> (Context, Module) {
    RANDOM_BYTES.with(|buf| *buf.borrow_mut() = VecDeque::from(random_bytes));

    let loader = Rc::new(HybridModuleLoader::new().unwrap());
    let mut context = ContextBuilder::default()
        .module_loader(loader.clone())
        .build()
        .unwrap();

    context
        .register_global_builtin_callable(
            js_string!("__bombadil_random_bytes"),
            1,
            NativeFunction::from_copy_closure(|_this, args, context| {
                let n = args
                    .first()
                    .map(|v| v.to_u32(context))
                    .transpose()?
                    .unwrap_or(0) as usize;
                let bytes: Vec<u8> = RANDOM_BYTES
                    .with(|buf| buf.borrow_mut().drain(..n).collect());
                Ok(JsUint8Array::from_iter(bytes, context)?.into())
            }),
        )
        .unwrap();

    let module = load_bombadil_module("random.js", &mut context).unwrap();
    load_modules(&mut context, std::slice::from_ref(&module)).unwrap();
    (context, module)
}

fn call_random_range(
    context: &mut Context,
    module: &Module,
    min: f64,
    max: f64,
) -> f64 {
    let random_range = js::module_exports(module, context)
        .unwrap()
        .get(&PropertyKey::String(js_string!("random_range")))
        .unwrap()
        .clone();
    let result = random_range
        .as_callable()
        .unwrap()
        .call(
            &JsValue::undefined(),
            &[JsValue::from(min), JsValue::from(max)],
            context,
        )
        .unwrap();
    result.as_number().unwrap()
}

proptest! {
    #[test]
    fn test_random_range(
        min in -1_000_000_000_000i64..999_999_999_999,
        spread in 1i64..1_000_000_000_000,
        // 8 bytes covers both the small path (4 bytes) and the large path (8 bytes)
        random_bytes in prop::collection::vec(any::<u8>(), 8),
    ) {
        let max = min + spread;
        let (mut context, module) = load_random_module(random_bytes);
        let n = call_random_range(&mut context, &module, min as f64, max as f64);
        prop_assert!(n >= min as f64, "value {n} < min {min}");
        prop_assert!(n < max as f64, "value {n} >= max {max}");
        prop_assert!(n.fract() == 0.0, "value {n} is not an integer");
    }
}
