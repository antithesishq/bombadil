use anyhow::{Result, anyhow, bail, ensure};
use cdp_protocol::cdp::{
    browser_protocol::target::SessionId,
    js_protocol::{
        debugger,
        runtime::{self, RemoteObjectType},
    },
};
use serde::de::DeserializeOwned;
use serde_json as json;

pub fn evaluate_expression_in_debugger<Output: DeserializeOwned>(
    connection: &cdp::Connection,
    session_id: &SessionId,
    call_frame_id: &debugger::CallFrameId,
    expression: impl Into<String>,
) -> Result<Output> {
    let returns: debugger::EvaluateOnCallFrameReturns = connection
        .send(
            debugger::EvaluateOnCallFrameParams::builder()
                .call_frame_id(call_frame_id.clone())
                .expression(expression)
                .throw_on_side_effect(false)
                .return_by_value(true)
                .build()
                .map_err(|err| anyhow!(err))?,
            Some(session_id),
        )
        .map_err(|err| anyhow!(err))?;
    if let Some(exception) = returns.exception_details {
        bail!(
            "evaluate_function failed: {}",
            format_exception_details(&exception)
        )
    } else {
        match returns.result.value.clone() {
            Some(value) => json::from_value(value).map_err(|err| anyhow!(err)),
            None => {
                if let Some(runtime::RemoteObjectSubtype::Null) =
                    returns.result.subtype
                {
                    json::from_value(json::Value::Null)
                        .map_err(|err| anyhow!(err))
                } else if let Some(ref value) =
                    returns.result.unserializable_value
                    && returns.result.r#type == RemoteObjectType::Bigint
                {
                    let s = value
                        .inner()
                        .strip_suffix('n')
                        .unwrap_or(value.inner());
                    json::from_value(json::json!(s)).map_err(|err| {
                        anyhow!(
                            "failed to parse bigint string as output: {}",
                            err
                        )
                    })
                } else {
                    bail!(
                        "no return value from function call: {:?}",
                        returns.result
                    );
                }
            }
        }
    }
}

pub fn evaluate_script_in_debugger(
    connection: &cdp::Connection,
    session_id: &SessionId,
    call_frame_id: &debugger::CallFrameId,
    script: impl Into<String>,
) -> Result<()> {
    let returns: debugger::EvaluateOnCallFrameReturns = connection
        .send(
            debugger::EvaluateOnCallFrameParams::builder()
                .call_frame_id(call_frame_id.clone())
                .expression(format!("(() => {{ {} }})()", script.into()))
                .throw_on_side_effect(false)
                .return_by_value(true)
                .build()
                .map_err(|err| anyhow!(err))?,
            Some(session_id),
        )
        .map_err(|err| anyhow!(err))?;
    if let Some(exception) = returns.exception_details {
        bail!(
            "evaluate_function failed: {}",
            format_exception_details(&exception)
        )
    }
    ensure!(
        returns.result.value.is_none(),
        "evaluated script shouldn't return a value"
    );
    Ok(())
}

fn format_exception_details(details: &runtime::ExceptionDetails) -> String {
    if let Some(description) = details
        .exception
        .as_ref()
        .and_then(|obj| obj.description.as_deref())
    {
        return description.to_string();
    }

    if let Some(stack_trace) = &details.stack_trace {
        let mut message = details.text.clone();
        for frame in &stack_trace.call_frames {
            let location = match &details.url {
                Some(url) if !url.is_empty() => {
                    format!(
                        "{}:{}:{}",
                        url, frame.line_number, frame.column_number
                    )
                }
                _ => format!(
                    "<anonymous>:{}:{}",
                    frame.line_number, frame.column_number
                ),
            };
            if frame.function_name.is_empty() {
                message.push_str(&format!("\n    at {location}"));
            } else {
                message.push_str(&format!(
                    "\n    at {} ({location})",
                    frame.function_name
                ));
            }
        }
        return message;
    }

    format!(
        "{}: line {}, column {}",
        details.text, details.line_number, details.column_number
    )
}

pub fn evaluate_function_call_in_debugger<Output: DeserializeOwned>(
    connection: &cdp::Connection,
    session_id: &SessionId,
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

    evaluate_expression_in_debugger(
        connection,
        session_id,
        call_frame_id,
        expression,
    )
}
