use anyhow::Result;
use bombadil::specification::bundler::bundle;
use bombadil::specification::verifier::{Specification, Verifier};
use rand::{RngExt, TryRng};
use url::Url;

pub use bombadil::runner::{
    ControlFlow, PropertyViolation, RunStrategy, Runner,
};

use crate::browser::{BrowserOptions, DebuggerOptions};
use crate::driver::BrowserDriver;

pub fn launch<Rng: TryRng + RngExt + 'static>(
    rng: Rng,
    origin: Url,
    specification: Specification,
    browser_options: BrowserOptions,
    debugger_options: DebuggerOptions,
) -> Result<Runner<BrowserDriver>> {
    let specification_bundle = bundle(".", &specification.module_specifier)?;
    let verifier = Verifier::new(&specification_bundle, rng)?;

    let driver = BrowserDriver::launch(
        origin,
        browser_options,
        debugger_options,
        specification_bundle,
    )?;

    Ok(Runner::new(driver, verifier))
}
