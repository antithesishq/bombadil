use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use anyhow::Result;
use bombadil::driver::{InterfaceDriver, RunId, TraceWriter};
use bombadil::runner;
use bombadil::specification::bundler::bundle;
use bombadil::specification::verifier::Specification;
use url::Url;

pub use bombadil::runner::{ControlFlow, PropertyViolation, RunStrategy};

use crate::browser::BrowserOptions;
use crate::driver::{BrowserDriver, BrowserSession, DebuggerOptions};

#[allow(clippy::too_many_arguments)]
pub fn launch<
    S: RunStrategy<BrowserSession>,
    Writer: TraceWriter<BrowserSession>,
>(
    run_id: RunId,
    origin: Url,
    specification: Specification,
    browser_options: BrowserOptions,
    debugger_options: DebuggerOptions,
    mut trace_writer: Writer,
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
    };

    let (mut session, verifier) = driver.new_session(run_id)?;

    runner::run(
        &mut session,
        strategy,
        verifier,
        &mut trace_writer,
        interrupted,
    )
}
