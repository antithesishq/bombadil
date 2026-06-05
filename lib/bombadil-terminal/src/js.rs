use anyhow::{Result, ensure};
use boa_engine::{
    Context, JsString, JsValue, js_string, object::ObjectInitializer,
    object::builtins::JsArray, property::Attribute,
};
use bombadil::driver::FromGeneratedAction;
use bombadil_schema::{
    TerminalCell, TerminalColor, TerminalGrid, TerminalSize, TerminalStyle,
    TerminalUnderline,
};
use serde::{Deserialize, Serialize};
use serde_json as json;

use crate::{driver::TerminalAction, state::TerminalState};

impl FromGeneratedAction for TerminalAction {
    fn from_generated(value: json::Value) -> Result<Self> {
        let js_action: JsTerminalAction = json::from_value(value)?;
        js_action.try_into()
    }
}

/// Builds the JavaScript `State` value handed to extractors directly with the
/// Boa API, reading the grid and scrollback by reference. This avoids cloning
/// the grids into intermediate `Vec`s and round-tripping the whole terminal
/// state through `serde_json`.
pub fn terminal_state_to_js(
    state: &TerminalState,
    context: &mut Context,
) -> JsValue {
    let grid = grid_to_js(&state.grid, context);
    let scrollback = grid_to_js(&state.scrollback, context);
    let last_action = match &state.last_action {
        Some(action) => action_to_js(action, context),
        None => JsValue::null(),
    };
    ObjectInitializer::new(context)
        .property(js_string!("grid"), grid, Attribute::all())
        .property(js_string!("scrollback"), scrollback, Attribute::all())
        .property(
            js_string!("scrollOffset"),
            JsValue::from(state.scroll_offset as f64),
            Attribute::all(),
        )
        .property(
            js_string!("terminated"),
            JsValue::from(state.terminated),
            Attribute::all(),
        )
        .property(js_string!("lastAction"), last_action, Attribute::all())
        .build()
        .into()
}

fn grid_to_js(grid: &TerminalGrid, context: &mut Context) -> JsValue {
    let rows = JsArray::new(context).expect("create rows array");
    for row_index in 0..grid.size.rows {
        let row = JsArray::new(context).expect("create row array");
        for column_index in 0..grid.size.columns {
            let cell = cell_to_js(&grid[(row_index, column_index)], context);
            row.push(cell, context).expect("push cell into row");
        }
        rows.push(row, context).expect("push row into grid");
    }
    let size = size_to_js(grid.size, context);
    ObjectInitializer::new(context)
        .property(js_string!("rows"), rows, Attribute::all())
        .property(js_string!("size"), size, Attribute::all())
        .build()
        .into()
}

fn cell_to_js(cell: &TerminalCell, context: &mut Context) -> JsValue {
    match cell {
        TerminalCell::Empty => js_string!("Empty").into(),
        TerminalCell::Continuation => js_string!("Continuation").into(),
        TerminalCell::Occupied {
            contents,
            wide,
            style,
        } => {
            let style = style_to_js(style, context);
            let occupied = ObjectInitializer::new(context)
                .property(
                    js_string!("contents"),
                    JsString::from(contents.to_string().as_str()),
                    Attribute::all(),
                )
                .property(
                    js_string!("wide"),
                    JsValue::from(*wide),
                    Attribute::all(),
                )
                .property(js_string!("style"), style, Attribute::all())
                .build();
            ObjectInitializer::new(context)
                .property(js_string!("Occupied"), occupied, Attribute::all())
                .build()
                .into()
        }
    }
}

fn style_to_js(style: &TerminalStyle, context: &mut Context) -> JsValue {
    let foreground = color_to_js(&style.foreground_color, context);
    let background = color_to_js(&style.background_color, context);
    let underline_color = color_to_js(&style.underline_color, context);
    let underline: JsValue =
        JsString::from(underline_name(&style.underline)).into();
    ObjectInitializer::new(context)
        .property(js_string!("foreground_color"), foreground, Attribute::all())
        .property(js_string!("background_color"), background, Attribute::all())
        .property(
            js_string!("underline_color"),
            underline_color,
            Attribute::all(),
        )
        .property(js_string!("underline"), underline, Attribute::all())
        .property(
            js_string!("attributes"),
            JsValue::from(style.attributes.bits() as f64),
            Attribute::all(),
        )
        .build()
        .into()
}

fn color_to_js(color: &TerminalColor, context: &mut Context) -> JsValue {
    match color {
        TerminalColor::None => js_string!("None").into(),
        TerminalColor::Palette(index) => ObjectInitializer::new(context)
            .property(
                js_string!("Palette"),
                JsValue::from(*index as f64),
                Attribute::all(),
            )
            .build()
            .into(),
        TerminalColor::RGB { r, g, b } => {
            let rgb = ObjectInitializer::new(context)
                .property(
                    js_string!("r"),
                    JsValue::from(*r as f64),
                    Attribute::all(),
                )
                .property(
                    js_string!("g"),
                    JsValue::from(*g as f64),
                    Attribute::all(),
                )
                .property(
                    js_string!("b"),
                    JsValue::from(*b as f64),
                    Attribute::all(),
                )
                .build();
            ObjectInitializer::new(context)
                .property(js_string!("RGB"), rgb, Attribute::all())
                .build()
                .into()
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

fn size_to_js(size: TerminalSize, context: &mut Context) -> JsValue {
    ObjectInitializer::new(context)
        .property(
            js_string!("columns"),
            JsValue::from(size.columns as f64),
            Attribute::all(),
        )
        .property(
            js_string!("rows"),
            JsValue::from(size.rows as f64),
            Attribute::all(),
        )
        .build()
        .into()
}

fn action_to_js(action: &TerminalAction, context: &mut Context) -> JsValue {
    let (key, payload) = match action {
        TerminalAction::TypeText { text } => {
            let payload = ObjectInitializer::new(context)
                .property(
                    js_string!("text"),
                    JsString::from(text.as_str()),
                    Attribute::all(),
                )
                .build();
            (js_string!("TypeText"), payload)
        }
        TerminalAction::PressKey { code } => {
            let payload = ObjectInitializer::new(context)
                .property(
                    js_string!("code"),
                    JsValue::from(*code as f64),
                    Attribute::all(),
                )
                .build();
            (js_string!("PressKey"), payload)
        }
        TerminalAction::Resize { size } => {
            let size = size_to_js(*size, context);
            let payload = ObjectInitializer::new(context)
                .property(js_string!("size"), size, Attribute::all())
                .build();
            (js_string!("Resize"), payload)
        }
        TerminalAction::ScrollUp {} => (
            js_string!("ScrollUp"),
            ObjectInitializer::new(context).build(),
        ),
        TerminalAction::ScrollDown {} => (
            js_string!("ScrollDown"),
            ObjectInitializer::new(context).build(),
        ),
    };
    ObjectInitializer::new(context)
        .property(key, payload, Attribute::all())
        .build()
        .into()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum JsTerminalAction {
    #[serde(rename_all = "camelCase")]
    TypeText {
        text: String,
    },
    #[serde(rename_all = "camelCase")]
    PressKey {
        code: f64,
    },
    #[serde(rename_all = "camelCase")]
    Resize {
        size: JsTerminalSize,
    },
    ScrollUp {},
    ScrollDown {},
}

impl TryFrom<JsTerminalAction> for TerminalAction {
    type Error = anyhow::Error;
    fn try_from(value: JsTerminalAction) -> Result<Self> {
        match value {
            JsTerminalAction::TypeText { text } => {
                Ok(TerminalAction::TypeText { text })
            }
            JsTerminalAction::PressKey { code } => {
                ensure!(code.is_normal(), "key code must be a normal number");
                Ok(TerminalAction::PressKey { code: code as u32 })
            }
            JsTerminalAction::Resize { size } => Ok(TerminalAction::Resize {
                size: size.try_into()?,
            }),
            JsTerminalAction::ScrollUp {} => Ok(TerminalAction::ScrollUp {}),
            JsTerminalAction::ScrollDown {} => {
                Ok(TerminalAction::ScrollDown {})
            }
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JsTerminalSize {
    rows: f64,
    columns: f64,
}

impl From<TerminalSize> for JsTerminalSize {
    fn from(value: TerminalSize) -> Self {
        JsTerminalSize {
            rows: value.rows as f64,
            columns: value.columns as f64,
        }
    }
}

impl TryFrom<JsTerminalSize> for TerminalSize {
    type Error = anyhow::Error;
    fn try_from(value: JsTerminalSize) -> Result<Self> {
        ensure!(value.rows.is_normal(), "rows must be a normal number");
        ensure!(value.columns.is_normal(), "columns must be a normal number");
        Ok(TerminalSize {
            rows: value.rows as u16,
            columns: value.columns as u16,
        })
    }
}
