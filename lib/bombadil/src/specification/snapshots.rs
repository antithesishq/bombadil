use boa_engine::{Context, JsValue, js_string};

use crate::specification::js::BombadilExports;
use crate::specification::result::{Result, SpecificationError};

pub fn with_snapshot_tracking<R>(
    context: &mut Context,
    bombadil_exports: &BombadilExports,
    f: impl FnOnce(&mut Context) -> Result<R>,
) -> Result<(Vec<usize>, R)> {
    let runtime = bombadil_exports.runtime.clone();

    let start = runtime
        .get(js_string!("startTracking"), context)?
        .as_callable()
        .ok_or(SpecificationError::OtherError(
            "startTracking is not callable".to_string(),
        ))?;

    let stop = runtime
        .get(js_string!("stopTracking"), context)?
        .as_callable()
        .ok_or(SpecificationError::OtherError(
            "stopTracking is not callable".to_string(),
        ))?;

    start.call(&JsValue::from(runtime.clone()), &[], context)?;
    let result = f(context);
    let accesses_js =
        stop.call(&JsValue::from(runtime.clone()), &[], context)?;
    let indices = serde_json::from_value(
        accesses_js.to_json(context)?.unwrap_or_default(),
    )
    .map_err(|error| {
        SpecificationError::OtherError(format!(
            "failed to deserialize extractor indices: {}",
            error
        ))
    })?;

    Ok((indices, result?))
}
