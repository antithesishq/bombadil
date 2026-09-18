use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use anyhow::Result;
use bombadil::driver::{InterfaceDriver, RunId, TraceWriterOutput};
use bombadil::runner;
use bombadil::specification::bundler::bundle;
use bombadil::specification::verifier::Specification;
use url::Url;

pub use bombadil::runner::{ControlFlow, PropertyViolation, RunStrategy};

use crate::browser::BrowserOptions;
use crate::driver::{BrowserDriver, BrowserSession, DebuggerOptions};

#[allow(clippy::too_many_arguments)]
pub fn launch<S: RunStrategy<BrowserSession>>(
    run_id: RunId,
    origin: Url,
    specification: Specification,
    browser_options: BrowserOptions,
    debugger_options: DebuggerOptions,
    trace_writer_output: Option<TraceWriterOutput>,
    interrupted: Arc<AtomicBool>,
    strategy: &mut S,
) -> Result<S::StopValue> {
    let specification_bundle =
        Arc::from(bundle(".", &specification.module_specifier)?);

    let driver = BrowserDriver {
        origin,
        browser_options,
        debugger_options,
        specification_bundle,
        trace_writer_output,
    };

    let (mut session, verifier, mut trace_writer) =
        driver.new_session(run_id)?;

    runner::run(
        &mut session,
        strategy,
        verifier,
        &mut trace_writer,
        interrupted,
    )
}
