use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::SystemTime;

use anyhow::Result;
use bombadil::render::Format;
use bombadil_schema::Time;
use serde::{Deserialize, Serialize};
use serde_json as json;
use tempfile::NamedTempFile;

use bombadil::driver::{
    ActionTemplate, DriverEvent, FromGeneratedAction, InterfaceDriver,
    InterfaceSession, RunState,
};
use bombadil::runner::{self, ControlFlow, PropertiesState, RunStrategy};
use bombadil::specification::bundler::bundle;
use bombadil::specification::domain::Snapshot;
use bombadil::specification::verifier::Verifier;
use bombadil::tree::Tree;

#[derive(Clone, Debug, Serialize, Deserialize)]
struct FakeAction;

impl Format for FakeAction {
    fn format(
        &self,
        f: &mut std::fmt::Formatter,
    ) -> std::prelude::v1::Result<(), std::fmt::Error> {
        write!(f, "Fake action")
    }
}

impl ActionTemplate<FakeAction> for FakeAction {
    fn generate<Rng: rand::TryRng + rand::RngExt>(
        &self,
        _rng: &mut Rng,
    ) -> FakeAction {
        self.clone()
    }

    fn accepts(&self, _original: &FakeAction) -> bool {
        true
    }
}

impl FromGeneratedAction for FakeAction {
    fn from_generated(_value: json::Value) -> Result<Self> {
        Ok(FakeAction)
    }
}

#[derive(Debug)]
struct FakeState;

impl RunState for FakeState {
    fn timestamp(&self) -> Time {
        Time::from_system_time(SystemTime::UNIX_EPOCH)
    }
}

struct FakeDriver {
    initiated: Arc<AtomicBool>,
    terminated: Arc<AtomicBool>,
    next_event_calls: Arc<AtomicUsize>,
}

impl InterfaceDriver for FakeDriver {
    type Session = FakeSession;

    fn initiate(
        &self,
    ) -> std::result::Result<(FakeSession, Verifier), anyhow::Error> {
        self.initiated.store(true, Ordering::SeqCst);
        Ok((
            FakeSession {
                terminated: self.terminated.clone(),
                next_event_calls: self.next_event_calls.clone(),
            },
            dummy_verifier(),
        ))
    }
}

struct FakeSession {
    terminated: Arc<AtomicBool>,
    next_event_calls: Arc<AtomicUsize>,
}

impl InterfaceSession for FakeSession {
    type Action = FakeAction;
    type ActionTemplate = FakeAction;
    type State = FakeState;

    fn terminate(&mut self) -> Result<()> {
        self.terminated.store(true, Ordering::SeqCst);
        Ok(())
    }

    fn next_event(&mut self) -> Option<DriverEvent<Self::State>> {
        self.next_event_calls.fetch_add(1, Ordering::SeqCst);
        Some(DriverEvent::StateChanged(Arc::new(FakeState)))
    }

    fn apply(
        &mut self,
        _action: FakeAction,
        _state: Arc<FakeState>,
    ) -> Result<()> {
        Ok(())
    }

    fn extract_snapshots(
        &mut self,
        _state: Arc<FakeState>,
        _last_action: Option<&FakeAction>,
    ) -> Result<Vec<Snapshot>> {
        Ok(vec![])
    }

    fn state_timestamp(_state: &FakeState) -> SystemTime {
        SystemTime::UNIX_EPOCH
    }
}

struct FakeStrategy {
    on_interrupted_calls: Arc<AtomicUsize>,
}

impl RunStrategy<FakeSession> for FakeStrategy {
    type StopValue = ();

    fn on_new_state(
        &mut self,
        _state: &FakeState,
        _tree: Tree<FakeAction>,
        _last_action: Option<&FakeAction>,
        _snapshots: &[Snapshot],
        _properties: PropertiesState,
    ) -> Result<ControlFlow<(), FakeAction>> {
        panic!("on_new_state must not run once interrupted");
    }

    fn on_interrupted(&mut self) -> Result<()> {
        self.on_interrupted_calls.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

fn dummy_verifier() -> Verifier {
    // Verifier::new requires at least one action generator, but no
    // property is ever evaluated because the runner never enters the
    // loop when `interrupted` is already set.
    let mut file = NamedTempFile::with_suffix(".ts").unwrap();
    file.write_all(
        b"import { actions } from \"@antithesishq/bombadil\";\n\
          export const _actions = actions(() => []);\n",
    )
    .unwrap();
    let bundle_code = bundle(".", &file.path().display().to_string()).unwrap();
    Verifier::new(&bundle_code).unwrap()
}

#[test]
fn interrupt_before_run_terminates_driver_and_invokes_on_interrupted() {
    let interrupted = Arc::new(AtomicBool::new(true));
    let initiated = Arc::new(AtomicBool::new(false));
    let terminated = Arc::new(AtomicBool::new(false));
    let next_event_calls = Arc::new(AtomicUsize::new(0));
    let on_interrupted_calls = Arc::new(AtomicUsize::new(0));

    let driver = FakeDriver {
        initiated: initiated.clone(),
        terminated: terminated.clone(),
        next_event_calls: next_event_calls.clone(),
    };

    let mut strategy = FakeStrategy {
        on_interrupted_calls: on_interrupted_calls.clone(),
    };

    let (mut session, verifier) =
        driver.initiate().expect("driver failed to initiate");
    runner::run(&mut session, &mut strategy, verifier, interrupted)
        .expect("runner returned Err");

    assert!(
        initiated.load(Ordering::SeqCst),
        "driver.initiate was never called",
    );
    assert!(
        terminated.load(Ordering::SeqCst),
        "driver.terminate was never called after interrupt",
    );
    assert_eq!(
        on_interrupted_calls.load(Ordering::SeqCst),
        1,
        "strategy.on_interrupted should be called exactly once",
    );
    assert_eq!(
        next_event_calls.load(Ordering::SeqCst),
        0,
        "driver.next_event must not be called when interrupted before loop",
    );
}
