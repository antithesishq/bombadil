use std::cell::RefCell;
use std::collections::VecDeque;

use boa_engine::{
    Context, JsObject, JsValue, NativeFunction, Source,
    context::ContextBuilder, js_string, object::builtins::JsUint8Array,
};
use hegel::{
    TestCase,
    generators::{integers, vecs},
};

thread_local! {
    static RANDOM_BYTES: RefCell<VecDeque<u8>> = const { RefCell::new(VecDeque::new()) };
}

fn load_random_module(
    random_bytes: Vec<u8>,
) -> Result<(Context, JsObject), String> {
    use crate::specification::bundler::bundle;

    RANDOM_BYTES.with(|buf| *buf.borrow_mut() = VecDeque::from(random_bytes));

    let mut context = ContextBuilder::default()
        .build()
        .map_err(|e| e.to_string())?;

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
        .map_err(|e| e.to_string())?;

    let bundle_code = bundle(".", "@antithesishq/bombadil/random")
        .map_err(|e| e.to_string())?;

    let specification_exports_value = context
        .eval(Source::from_bytes(&bundle_code))
        .map_err(|e| e.to_string())?;
    let specification_exports_obj = specification_exports_value
        .as_object()
        .ok_or_else(|| "specification exports is not an object".to_string())?;

    Ok((context, specification_exports_obj.clone()))
}

fn call_random_range(
    context: &mut Context,
    exports_obj: &JsObject,
    min: f64,
    max: f64,
) -> Result<f64, String> {
    let random_range = exports_obj
        .get(js_string!("randomRange"), context)
        .map_err(|e| e.to_string())?
        .as_callable()
        .ok_or_else(|| "randomRange is not a function".to_string())?;

    let result = random_range
        .call(
            &JsValue::undefined(),
            &[JsValue::from(min), JsValue::from(max)],
            context,
        )
        .map_err(|e| e.to_string())?;

    result
        .as_number()
        .ok_or_else(|| "randomRange did not return a number".to_string())
}

#[hegel::test]
fn test_random_range(tc: TestCase) {
    let min = tc.draw(
        integers()
            .min_value(-1_000_000_000_000i64)
            .max_value(999_999_999_999),
    );
    let spread =
        tc.draw(integers().min_value(1i64).max_value(1_000_000_000_000));
    // 8 bytes covers both the small path (4 bytes) and the large path (8 bytes)
    let random_bytes = tc.draw(vecs(integers::<u8>()).min_size(8).max_size(8));

    let max = min + spread;
    let (mut context, exports_obj) = load_random_module(random_bytes).unwrap();
    let n =
        call_random_range(&mut context, &exports_obj, min as f64, max as f64)
            .unwrap();
    assert!(n >= min as f64, "value {n} < min {min}");
    assert!(n < max as f64, "value {n} >= max {max}");
    assert!(n.fract() == 0.0, "value {n} is not an integer");
}
