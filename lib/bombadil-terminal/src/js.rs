use std::{ops::RangeInclusive, sync::Arc};

use anyhow::Result;
use bombadil::{
    driver::FromGeneratedAction,
    specification::js::{JsRange, JsStringGenerator},
};
use bombadil_schema::terminal::{
    ProcessExitStatus, TerminalCell, TerminalColor, TerminalCursor,
    TerminalCursorPosition, TerminalCursorVisualStyle, TerminalGrid,
    TerminalSize, TerminalStyle, TerminalUnderline,
};
use rquickjs::{
    Array, Coerced, Ctx, Function, IntoJs, Object, Value, function::Opt,
};
use serde::{Deserialize, Serialize};
use serde_json as json;

use crate::{
    driver::{TerminalAction, TerminalActionTemplate},
    state::TerminalState,
};

impl FromGeneratedAction for TerminalActionTemplate {
    fn from_generated(value: json::Value) -> Result<Self> {
        let js_action: JsTerminalAction = json::from_value(value)?;
        js_action.try_into()
    }
}

#[derive(Clone, Copy)]
enum GridKind {
    Screen,
    Scrollback,
}

#[hotpath::measure]
pub fn terminal_state_to_js<'js>(
    state: Arc<TerminalState>,
    ctx: &Ctx<'js>,
) -> rquickjs::Result<Value<'js>> {
    let grid = grid_to_js(&state, GridKind::Screen, ctx)?;
    let scrollback = grid_to_js(&state, GridKind::Scrollback, ctx)?;
    let cursor = cursor_to_js(&state.cursor, ctx)?;
    let last_action = match &state.last_action {
        Some(action) => action_to_js(action, ctx)?,
        None => Value::new_null(ctx.clone()),
    };
    let exit_status: Value<'js> = match &state.exit_status {
        Some(ProcessExitStatus { code, signal }) => {
            let object = Object::new(ctx.clone())?;
            object.set("code", *code)?;
            object.set("signal", string_or_null(signal.as_deref(), ctx)?)?;
            object.into()
        }
        None => Value::new_null(ctx.clone()),
    };
    let object = Object::new(ctx.clone())?;
    object.set("grid", grid)?;
    object.set("scrollback", scrollback)?;
    object.set("scrollOffset", state.scroll_offset as f64)?;
    object.set("cursor", cursor)?;
    object.set("exitStatus", exit_status)?;
    object.set("lastAction", last_action)?;
    Ok(object.into())
}

fn string_or_null<'js>(
    text: Option<&str>,
    ctx: &Ctx<'js>,
) -> rquickjs::Result<Value<'js>> {
    match text {
        Some(text) => text.into_js(ctx),
        None => Ok(Value::new_null(ctx.clone())),
    }
}

fn cursor_to_js<'js>(
    cursor: &TerminalCursor,
    ctx: &Ctx<'js>,
) -> rquickjs::Result<Value<'js>> {
    let object = Object::new(ctx.clone())?;
    object.set("position", cursor_position_to_js(cursor.position, ctx)?)?;
    object.set("visible", cursor.visible)?;
    object.set("blinking", cursor.blinking)?;
    object.set(
        "visualStyle",
        cursor_visual_style_name(&cursor.visual_style),
    )?;
    object.set("color", color_to_js(&cursor.color, ctx)?)?;
    Ok(object.into())
}

fn cursor_position_to_js<'js>(
    position: TerminalCursorPosition,
    ctx: &Ctx<'js>,
) -> rquickjs::Result<Value<'js>> {
    let object = Object::new(ctx.clone())?;
    object.set("column", position.column as f64)?;
    object.set("row", position.row as f64)?;
    Ok(object.into())
}

fn cursor_visual_style_name(
    visual_style: &TerminalCursorVisualStyle,
) -> &'static str {
    match visual_style {
        TerminalCursorVisualStyle::Bar => "Bar",
        TerminalCursorVisualStyle::Block => "Block",
        TerminalCursorVisualStyle::Underline => "Underline",
        TerminalCursorVisualStyle::BlockHollow => "BlockHollow",
        TerminalCursorVisualStyle::Unknown => "Unknown",
    }
}

fn grid_to_js<'js>(
    state: &Arc<TerminalState>,
    kind: GridKind,
    ctx: &Ctx<'js>,
) -> rquickjs::Result<Value<'js>> {
    let object = Object::new(ctx.clone())?;
    object.set("size", size_to_js(state_grid(state, kind).size, ctx)?)?;
    object.set("row", grid_function(ctx, state, kind, "row", row_at)?)?;
    object.set(
        "rowText",
        grid_function(ctx, state, kind, "rowText", row_text_at)?,
    )?;
    Ok(object.into())
}

fn state_grid(state: &Arc<TerminalState>, kind: GridKind) -> &TerminalGrid {
    match kind {
        GridKind::Screen => &state.grid,
        GridKind::Scrollback => &state.scrollback,
    }
}

fn grid_function<'js>(
    ctx: &Ctx<'js>,
    state: &Arc<TerminalState>,
    kind: GridKind,
    name: &str,
    function: fn(
        &Ctx<'js>,
        &TerminalGrid,
        Opt<Value<'js>>,
    ) -> rquickjs::Result<Value<'js>>,
) -> rquickjs::Result<Function<'js>> {
    let state = Arc::clone(state);
    Function::new(ctx.clone(), move |ctx: Ctx<'js>, index: Opt<Value<'js>>| {
        function(&ctx, state_grid(&state, kind), index)
    })?
    .with_name(name)?
    .with_length(1)
}

fn to_uint32(number: f64) -> u32 {
    if !number.is_finite() {
        return 0;
    }
    number.trunc().rem_euclid(4_294_967_296.0) as u32
}

fn row_index_arg(
    index: Opt<Value<'_>>,
    grid: &TerminalGrid,
) -> rquickjs::Result<Option<u16>> {
    let Opt(Some(value)) = index else {
        return Ok(None);
    };
    let index = to_uint32(value.get::<Coerced<f64>>()?.0);
    if index >= grid.size.rows as u32 {
        return Ok(None);
    }
    Ok(Some(index as u16))
}

#[hotpath::measure]
fn row_text_at<'js>(
    ctx: &Ctx<'js>,
    grid: &TerminalGrid,
    index: Opt<Value<'js>>,
) -> rquickjs::Result<Value<'js>> {
    let Some(row_index) = row_index_arg(index, grid)? else {
        return Ok(Value::new_undefined(ctx.clone()));
    };
    row_text(grid, row_index).as_str().into_js(ctx)
}

/// The text of row `row_index` of `grid`, as `rowText` gives it: a space
/// per empty cell.
pub(crate) fn row_text(grid: &TerminalGrid, row_index: u16) -> String {
    let mut text = String::with_capacity(grid.size.columns as usize);
    for column_index in 0..grid.size.columns {
        match &grid[(row_index, column_index)] {
            TerminalCell::Occupied { contents, .. } => {
                use std::fmt::Write;
                let _ = write!(text, "{contents}");
            }
            TerminalCell::Empty { .. } => text.push(' '),
            TerminalCell::Continuation { .. } => {}
        }
    }
    text
}

#[hotpath::measure]
fn row_at<'js>(
    ctx: &Ctx<'js>,
    grid: &TerminalGrid,
    index: Opt<Value<'js>>,
) -> rquickjs::Result<Value<'js>> {
    let Some(row_index) = row_index_arg(index, grid)? else {
        return Ok(Value::new_undefined(ctx.clone()));
    };
    let row = Array::new(ctx.clone())?;
    for column_index in 0..grid.size.columns {
        let cell = cell_to_js(&grid[(row_index, column_index)], ctx)?;
        row.set(column_index as usize, cell)?;
    }
    Ok(row.into())
}

fn cell_to_js<'js>(
    cell: &TerminalCell,
    ctx: &Ctx<'js>,
) -> rquickjs::Result<Value<'js>> {
    let (contents, wide, style) = match cell {
        TerminalCell::Empty { style } => (" ".to_string(), false, style),
        TerminalCell::Continuation { style } => (String::new(), false, style),
        TerminalCell::Occupied {
            contents,
            wide,
            style,
        } => (contents.to_string(), *wide, style),
    };
    let object = Object::new(ctx.clone())?;
    object.set("contents", contents.as_str())?;
    object.set("wide", wide)?;
    object.set("style", style_to_js(style, ctx)?)?;
    Ok(object.into())
}

fn style_to_js<'js>(
    style: &TerminalStyle,
    ctx: &Ctx<'js>,
) -> rquickjs::Result<Value<'js>> {
    let object = Object::new(ctx.clone())?;
    object.set(
        "foregroundColor",
        color_to_js(&style.foreground_color, ctx)?,
    )?;
    object.set(
        "backgroundColor",
        color_to_js(&style.background_color, ctx)?,
    )?;
    object.set("underlineColor", color_to_js(&style.underline_color, ctx)?)?;
    object.set("underline", underline_name(&style.underline))?;
    object.set("attributes", style.attributes.bits() as f64)?;
    Ok(object.into())
}

fn color_to_js<'js>(
    color: &TerminalColor,
    ctx: &Ctx<'js>,
) -> rquickjs::Result<Value<'js>> {
    match color {
        TerminalColor::None => "None".into_js(ctx),
        TerminalColor::Palette(index) => {
            let object = Object::new(ctx.clone())?;
            object.set("Palette", *index as f64)?;
            Ok(object.into())
        }
        TerminalColor::RGB { r, g, b } => {
            let rgb = Object::new(ctx.clone())?;
            rgb.set("r", *r as f64)?;
            rgb.set("g", *g as f64)?;
            rgb.set("b", *b as f64)?;
            let object = Object::new(ctx.clone())?;
            object.set("RGB", rgb)?;
            Ok(object.into())
        }
    }
}

fn underline_name(underline: &TerminalUnderline) -> &'static str {
    match underline {
        TerminalUnderline::None => "None",
        TerminalUnderline::Single => "Single",
        TerminalUnderline::Double => "Double",
        TerminalUnderline::Curly => "Curly",
        TerminalUnderline::Dotted => "Dotted",
        TerminalUnderline::Dashed => "Dashed",
    }
}

fn size_to_js<'js>(
    size: TerminalSize,
    ctx: &Ctx<'js>,
) -> rquickjs::Result<Value<'js>> {
    let object = Object::new(ctx.clone())?;
    object.set("columns", size.columns as f64)?;
    object.set("rows", size.rows as f64)?;
    Ok(object.into())
}

fn action_to_js<'js>(
    action: &TerminalAction,
    ctx: &Ctx<'js>,
) -> rquickjs::Result<Value<'js>> {
    let (key, payload): (&str, Value<'js>) = match action {
        TerminalAction::TypeText { text } => {
            ("TypeText", text.as_str().into_js(ctx)?)
        }
        TerminalAction::Resize { size } => {
            let payload = Object::new(ctx.clone())?;
            payload.set("size", size_to_js(*size, ctx)?)?;
            ("Resize", payload.into())
        }
        TerminalAction::ScrollUp {} => {
            ("ScrollUp", Object::new(ctx.clone())?.into())
        }
        TerminalAction::ScrollDown {} => {
            ("ScrollDown", Object::new(ctx.clone())?.into())
        }
        TerminalAction::Click { row, column } => {
            let payload = Object::new(ctx.clone())?;
            payload.set("row", *row)?;
            payload.set("column", *column)?;
            ("Click", payload.into())
        }
    };
    let object = Object::new(ctx.clone())?;
    object.set(key, payload)?;
    Ok(object.into())
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum JsTerminalAction {
    TypeText(JsStringGenerator),
    Resize(JsTerminalSize),
    #[serde(rename_all = "camelCase")]
    Click {
        row: JsRange,
        column: JsRange,
    },
    ScrollUp {},
    ScrollDown {},
}

impl TryFrom<JsTerminalAction> for TerminalActionTemplate {
    type Error = anyhow::Error;
    fn try_from(value: JsTerminalAction) -> Result<Self> {
        match value {
            JsTerminalAction::TypeText(text) => Ok(TerminalAction::TypeText {
                text: text.try_into()?,
            }),
            JsTerminalAction::Resize(size) => Ok(TerminalAction::Resize {
                size: size.try_into()?,
            }),
            JsTerminalAction::ScrollUp {} => Ok(TerminalAction::ScrollUp {}),
            JsTerminalAction::ScrollDown {} => {
                Ok(TerminalAction::ScrollDown {})
            }
            JsTerminalAction::Click { row, column } => {
                Ok(TerminalAction::Click {
                    row: row.try_into()?,
                    column: column.try_into()?,
                })
            }
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JsTerminalSize {
    rows: JsRange,
    columns: JsRange,
}

impl TryFrom<TerminalSize<RangeInclusive<u16>>> for JsTerminalSize {
    type Error = anyhow::Error;
    fn try_from(
        value: TerminalSize<RangeInclusive<u16>>,
    ) -> anyhow::Result<Self> {
        Ok(JsTerminalSize {
            rows: value.rows.try_into()?,
            columns: value.columns.try_into()?,
        })
    }
}

impl TryFrom<JsTerminalSize> for TerminalSize<RangeInclusive<u16>> {
    type Error = anyhow::Error;
    fn try_from(value: JsTerminalSize) -> Result<Self> {
        Ok(TerminalSize {
            rows: value.rows.try_into()?,
            columns: value.columns.try_into()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::to_uint32;

    #[test]
    fn to_uint32_follows_ecmascript() {
        assert_eq!(to_uint32(2.0), 2);
        assert_eq!(to_uint32(2.7), 2);
        assert_eq!(to_uint32(-1.0), u32::MAX);
        assert_eq!(to_uint32(4_294_967_296.0 + 3.0), 3);
        assert_eq!(to_uint32(f64::NAN), 0);
        assert_eq!(to_uint32(f64::INFINITY), 0);
    }
}
