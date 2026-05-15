use anyhow::Result;
use bombadil::specification::bundler::bundle;
use bombadil::specification::verifier::Specification;
use bombadil::specification::worker::VerifierWorker;
use url::Url;

pub use bombadil::runner::{
    ControlFlow, PropertyViolation, RunStrategy, Runner,
};

use crate::browser::{Browser, BrowserOptions, DebuggerOptions};
use crate::driver::BrowserDriver;

/// Convenience constructor: starts the verifier worker, launches the
/// browser, evaluates the bundled specification, and hands back a
/// Runner ready to `run(&mut strategy)`.
pub async fn launch(
    origin: Url,
    specification: Specification,
    browser_options: BrowserOptions,
    debugger_options: DebuggerOptions,
) -> Result<Runner<BrowserDriver>> {
    let verifier = VerifierWorker::start(specification.clone()).await?;

    let browser =
        Browser::new(origin, browser_options, debugger_options).await?;

    browser
        .ensure_script_evaluated(
            &bundle(".", &specification.module_specifier).await?,
        )
        .await?;

    Ok(Runner::new(BrowserDriver::new(browser), verifier))
}
