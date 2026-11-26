use rquickjs::{Array, Ctx, Value, function::This};

use crate::specification::js::BombadilExports;
use crate::specification::result::{JsResultExt, Result, SpecificationError};

pub fn with_snapshot_tracking<'js, R>(
    ctx: &Ctx<'js>,
    bombadil_exports: &BombadilExports,
    f: impl FnOnce(&Ctx<'js>) -> Result<R>,
) -> Result<(Vec<usize>, R)> {
    let runtime = bombadil_exports.runtime.clone().restore(ctx).js(ctx)?;

    let start = runtime.get::<_, Value>("startTracking").js(ctx)?;
    let start = start.as_function().ok_or(SpecificationError::OtherError(
        "startTracking is not callable".to_string(),
    ))?;

    let stop = runtime.get::<_, Value>("stopTracking").js(ctx)?;
    let stop = stop.as_function().ok_or(SpecificationError::OtherError(
        "stopTracking is not callable".to_string(),
    ))?;

    start.call::<_, Value>((This(runtime.clone()),)).js(ctx)?;
    let result = f(ctx);
    let accesses = stop.call::<_, Value>((This(runtime.clone()),)).js(ctx)?;
    let accesses = Array::from_value(accesses).js(ctx)?;
    let indices = (0..accesses.len())
        .map(|i| accesses.get::<usize>(i).js(ctx))
        .collect::<Result<Vec<usize>>>()?;

    Ok((indices, result?))
}
