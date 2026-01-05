use anyhow::{anyhow, bail, Result};
use chromiumoxide::{
    cdp::js_protocol::{debugger, runtime},
    Page,
};
use serde::de::DeserializeOwned;
use serde_json as json;

pub async fn evaluate_expression_in_debugger<Output: DeserializeOwned>(
    page: &Page,
    call_frame_id: &debugger::CallFrameId,
    expression: impl Into<String>,
) -> Result<Output> {
    let returns: debugger::EvaluateOnCallFrameReturns = page
        .execute(
            debugger::EvaluateOnCallFrameParams::builder()
                .call_frame_id(call_frame_id.clone())
                .expression(expression)
                .throw_on_side_effect(false)
                .return_by_value(true)
                .build()
                .map_err(|err| anyhow!(err))?,
        )
        .await
        .map_err(|err| anyhow!(err))?
        .result;
    if let Some(exception) = returns.exception_details {
        bail!("evaluate_function failed: {:?}", exception)
    } else {
        match returns.result.value.clone() {
            Some(value) => json::from_value(value),
            None => {
                if let Some(runtime::RemoteObjectSubtype::Null) =
                    returns.result.subtype
                {
                    json::from_value(json::Value::Null)
                } else {
                    bail!(
                        "no return value from function call: {:?}",
                        returns.result
                    );
                }
            }
        }
        .map_err(|err| anyhow!(err))
    }
}

pub async fn evaluate_function_call_in_debugger<Output: DeserializeOwned>(
    page: &Page,
    call_frame_id: &debugger::CallFrameId,
    function_expression: impl Into<String>,
    arguments: Vec<json::Value>,
) -> Result<Output> {
    let mut arguments_json = Vec::with_capacity(arguments.len());
    for arg in arguments {
        arguments_json.push(json::to_string(&arg)?);
    }
    let expression = format!(
        "({})({})",
        function_expression.into(),
        arguments_json.join(", ")
    );

    evaluate_expression_in_debugger(page, call_frame_id, expression).await
}
