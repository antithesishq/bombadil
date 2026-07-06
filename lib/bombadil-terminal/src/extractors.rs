use std::sync::Arc;

use anyhow::Result;
use bombadil::specification::domain::Snapshot;
use bombadil::specification::extractor_harness::ExtractorHarness;
use bombadil_schema::Time;

use crate::{js::terminal_state_to_js, state::TerminalState};

pub struct Extractors {
    harness: ExtractorHarness,
}

impl Extractors {
    pub fn initialize(bundle_code: &str) -> Result<Self> {
        Ok(Extractors {
            harness: ExtractorHarness::new(bundle_code)?,
        })
    }

    #[hotpath::measure]
    pub fn run_extractors(
        &mut self,
        state: Arc<TerminalState>,
    ) -> Result<Vec<Snapshot>> {
        let time = Time::from_system_time(state.timestamp);
        let state_value =
            terminal_state_to_js(state, self.harness.context_mut());
        self.harness.run(state_value, time)
    }
}
