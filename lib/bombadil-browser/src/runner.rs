use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use anyhow::Result;
use bombadil::runner;
use bombadil::specification::bundler::bundle;
use bombadil::specification::verifier::{Specification, Verifier};
use url::Url;

pub use bombadil::runner::{ControlFlow, PropertyViolation, RunStrategy};

use crate::browser::BrowserOptions;
use crate::driver::{BrowserDriver, DebuggerOptions};

pub fn launch<S: RunStrategy<BrowserDriver>>(
    origin: Url,
    specification: Specification,
    browser_options: BrowserOptions,
    debugger_options: DebuggerOptions,
    interrupted: Arc<AtomicBool>,
    strategy: &mut S,
) -> Result<S::StopValue> {
    let specification_bundle = bundle(".", &specification.module_specifier)?;
    let verifier = Verifier::new(&specification_bundle)?;

    let mut driver = BrowserDriver::launch(
        origin,
        browser_options,
        debugger_options,
        specification_bundle,
    )?;

    runner::run(&mut driver, strategy, verifier, interrupted)
}
