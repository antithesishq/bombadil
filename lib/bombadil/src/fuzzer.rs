use anyhow::{Result, anyhow};
use bombadil_schema::Time;
use crossbeam_channel as mpmc;
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fmt::{Display, Write as _},
    hash::{DefaultHasher, Hasher},
    io::Write,
    sync::{
        Arc, Mutex, RwLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::{Duration, SystemTime},
};

use rand::{Rng, RngExt, SeedableRng, TryRng, prelude::StdRng};
use stdx::ring_buffer::RingBuffer;

use crate::{
    driver::{
        ActionTemplate, InterfaceDriver, InterfaceSession, OutputWriter, RunId,
        RunState,
    },
    render::Formatted,
    runner::{
        self, ControlFlow, PropertiesState, PropertyViolation, RunStrategy,
    },
    specification::domain::Snapshot,
    styled::{maybe_bold, maybe_red},
    tree::Tree,
};

const FUZZ_WORKER_COUNT: usize = 8;
const FUZZ_WORKER_ACTIONS_COUNT_MAX: usize = 32;

#[derive(Clone, Copy, Debug)]
struct WorkerId(usize);

impl Display for WorkerId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
enum WorkerMessage<Session: InterfaceSession> {
    Start {
        worker_id: WorkerId,
        run_id: RunId,
    },
    Step {
        worker_id: WorkerId,
        run_id: RunId,
        time_relative: Duration,
        violations: Vec<PropertyViolation>,
        action_selected: Session::Action,
    },
}

#[derive(Debug)]
struct Worker<D: InterfaceDriver> {
    worker_id: WorkerId,
    run_id: Option<RunId>,
    handle: thread::JoinHandle<()>,
    actions: RingBuffer<
        (
            Duration,
            <<D as InterfaceDriver>::Session as InterfaceSession>::Action,
        ),
        FUZZ_WORKER_ACTIONS_COUNT_MAX,
    >,
    violations_count: u64,
}

impl<D: InterfaceDriver> Worker<D> {
    fn reset(&mut self, run_id: RunId) {
        self.run_id = Some(run_id);
        self.actions = RingBuffer::new(); // TODO: add `.clear()`
        self.violations_count = 0;
    }
}

#[derive(Debug)]
struct FuzzState<D: InterfaceDriver> {
    workers: Vec<Worker<D>>,
    property_violation_run_ids: BTreeMap<String, BTreeSet<RunId>>,
}

#[derive(Debug)]
pub struct FuzzOptions<
    D: InterfaceDriver + Send + Sync + 'static,
    Writer: OutputWriter<D::Session>,
    Rng: TryRng + RngExt,
> {
    pub rng: Rng,
    pub driver: Arc<D>,
    pub output_writer: Writer,
    pub interrupted: Arc<AtomicBool>,
    pub time_limit_fuzz: Duration,
    pub time_limit_run: Duration,
    pub swarm: bool,
}

pub fn fuzz<
    D: InterfaceDriver + Send + Sync + 'static,
    Writer: OutputWriter<D::Session> + Send + Sync + 'static,
    Rng: TryRng + RngExt,
>(
    FuzzOptions {
        mut rng,
        driver,
        interrupted,
        time_limit_fuzz,
        time_limit_run,
        swarm,
        output_writer,
    }: FuzzOptions<D, Writer, Rng>,
) -> Result<()>
where
    <<D as InterfaceDriver>::Session as InterfaceSession>::Action: Send + Sync,
{
    let fuzz_start = Time::from_system_time(SystemTime::now());
    let fuzz_deadline = fuzz_start + time_limit_fuzz;

    let output_writer = Arc::new(Mutex::new(output_writer));
    let (worker_tx, worker_rx) = mpmc::unbounded();
    let run_id_next = Arc::new(AtomicU64::new(0));

    let mut workers: Vec<Worker<D>> = Vec::with_capacity(FUZZ_WORKER_COUNT);
    for i in 0..FUZZ_WORKER_COUNT {
        let worker_tx = worker_tx.clone();
        let worker_id = WorkerId(i);
        let run_id_next = run_id_next.clone();
        let driver = driver.clone();
        let output_writer = output_writer.clone();
        let seed = rng.next_u64();
        let interrupted = interrupted.clone();
        log::debug!("spawning {worker_id}");
        let handle = thread::spawn(move || {
            let fuzz_worker_thread = FuzzWorkerThread {
                driver,
                output_writer,
                worker_tx,
                interrupted,
                seed,
                time_limit: time_limit_run,
                fuzz_deadline,
                worker_id,
                run_id_next,
                swarm,
            };
            let outcome =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    fuzz_worker_thread.run()
                }));
            if let Err(payload) = outcome {
                let msg = payload
                    .downcast_ref::<&'static str>()
                    .map(|s| (*s).to_string())
                    .or_else(|| payload.downcast_ref::<String>().cloned())
                    .unwrap_or_else(|| {
                        "<non-string panic payload>".to_string()
                    });
                log::error!("worker {worker_id} panicked: {msg}");
            }
        });
        workers.push(Worker {
            worker_id,
            run_id: None,
            handle,
            actions: RingBuffer::new(),
            violations_count: 0,
        });
    }
    // Unless we drop this Sender this the channel will live forever, even after the workers have
    // finished, and the the loop below will never exit.
    drop(worker_tx);

    let fuzz_state = Arc::new(RwLock::new(FuzzState {
        workers,
        property_violation_run_ids: BTreeMap::new(),
    }));

    let render_loop_handle =
        render_loop_spawn(interrupted.clone(), fuzz_state.clone());

    while let Ok(message) = worker_rx.recv()
        && !interrupted.load(Ordering::SeqCst)
    {
        match message {
            WorkerMessage::Start {
                worker_id: WorkerId(worker_index),
                run_id,
            } => {
                let mut state =
                    fuzz_state.write().expect("failed to acquire state lock");
                state.workers[worker_index].reset(run_id);
            }
            WorkerMessage::Step {
                worker_id: WorkerId(worker_index),
                run_id,
                time_relative,
                action_selected,
                violations,
            } => {
                let mut state =
                    fuzz_state.write().expect("failed to acquire state lock");

                state.workers[worker_index]
                    .actions
                    .push((time_relative, action_selected));
                state.workers[worker_index].violations_count +=
                    violations.len() as u64;
                for violation in violations {
                    log::info!(
                        "{}/{}, violation of {}: {:?}",
                        state.workers[worker_index].worker_id,
                        run_id,
                        violation.name,
                        violation.violation
                    );
                    state
                        .property_violation_run_ids
                        .entry(violation.name)
                        .or_default()
                        .insert(run_id);
                }
            }
        };
    }

    println!("Shutting down...");

    interrupted.store(true, Ordering::SeqCst);
    render_loop_handle
        .join()
        .map_err(|_| anyhow!("render loop thread panicked"))??;

    {
        let state = Arc::try_unwrap(fuzz_state)
            .unwrap_or_else(|_| panic!("fuzz state still has outstanding refs"))
            .into_inner()
            .expect("failed to get inner fuzz state");
        for worker in state.workers {
            worker
                .handle
                .join()
                .map_err(|_| anyhow!("fuzz run thread panicked"))?;
        }
    }

    Ok(())
}

fn render_loop_spawn<D: InterfaceDriver + 'static>(
    interrupted: Arc<AtomicBool>,
    state: Arc<RwLock<FuzzState<D>>>,
) -> thread::JoinHandle<Result<()>>
where
    <<D as InterfaceDriver>::Session as InterfaceSession>::Action: Send + Sync,
{
    thread::spawn(move || {
        while !interrupted.load(Ordering::SeqCst) {
            thread::sleep(Duration::from_millis(100));
            let Ok(state) = state.try_read() else {
                log::warn!("failed to acquire state lock, skipping render");
                continue;
            };
            let mut buffer = String::new();
            const SEP: &str = "  ";
            write!(buffer, "\x1b[2J\x1b[H")?;
            writeln!(
                buffer,
                "{}",
                maybe_bold(format!(
                    "{:^6}{SEP}{:^3}{SEP}{:^10}{SEP}{:^6}{SEP}{:^9}{SEP}Action",
                    "Worker", "Run", "Violations", "SPS", "Time"
                ))
            )?;
            for worker in &state.workers {
                write!(buffer, "{:^6}", worker.worker_id.0)?;
                write!(buffer, "{SEP}")?;
                write!(
                    buffer,
                    "{:^3}",
                    worker
                        .run_id
                        .map(|id| format!("{}", id))
                        .unwrap_or("-".into())
                )?;
                write!(buffer, "{SEP}")?;
                write!(buffer, "{:^10}", worker.violations_count)?;
                write!(buffer, "{SEP}")?;
                if let Some((action_last_time, action_last)) =
                    worker.actions.last()
                {
                    if worker.actions.len() > 1
                        && let Some(states_per_second) = worker
                            .actions
                            .first()
                            .map(|(action_first_time, _)| {
                                (worker.actions.len() as f64)
                                    / action_last_time
                                        .checked_sub(*action_first_time)
                                        .expect("action times are not ordered")
                                        .as_secs_f64()
                            })
                    {
                        write!(buffer, "{:>6.1}", states_per_second)?;
                    } else {
                        write!(buffer, "{:>6}", "")?;
                    }
                    write!(buffer, "{SEP}")?;

                    writeln!(
                        buffer,
                        "{:>9}{SEP}{}",
                        Formatted(action_last_time),
                        Formatted(action_last)
                    )?;
                } else {
                    writeln!(buffer)?;
                }
            }
            writeln!(buffer)?;
            if !state.property_violation_run_ids.is_empty() {
                writeln!(
                    buffer,
                    "{}\n",
                    maybe_bold(
                        "Violated properties (and their run IDs):".to_string()
                    ),
                )?;
            }
            for (property_name, run_ids) in &state.property_violation_run_ids {
                if !run_ids.is_empty() {
                    writeln!(
                        buffer,
                        "{}: {}",
                        maybe_red(property_name.clone()),
                        run_ids
                            .iter()
                            .map(|id| format!("{id}"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )?;
                }
            }

            print!("{}", buffer);
            std::io::stdout().flush()?;
        }
        Ok(())
    })
}

struct FuzzWorkerThread<D: InterfaceDriver, Writer: OutputWriter<D::Session>> {
    driver: Arc<D>,
    output_writer: Arc<Mutex<Writer>>,
    worker_tx: mpmc::Sender<WorkerMessage<D::Session>>,
    interrupted: Arc<AtomicBool>,
    seed: u64,
    time_limit: Duration,
    fuzz_deadline: Time,
    worker_id: WorkerId,
    run_id_next: Arc<AtomicU64>,
    swarm: bool,
}

impl<D: InterfaceDriver, Writer: OutputWriter<D::Session>>
    FuzzWorkerThread<D, Writer>
{
    #[hotpath::measure]
    fn run(self) {
        let mut rng = StdRng::seed_from_u64(self.seed);

        for iteration in 0.. {
            if self.interrupted.load(Ordering::SeqCst) {
                break;
            }
            let run_id = RunId(self.run_id_next.fetch_add(1, Ordering::SeqCst));
            log::info!(
                "worker {} entering iteration {} (run_id={})",
                self.worker_id,
                iteration,
                run_id,
            );
            let test_start = Time::from_system_time(SystemTime::now());
            if test_start > self.fuzz_deadline {
                break;
            }
            let run_seed = rng.next_u64();
            let action_probability = rng.random_range(0.3..0.8);
            let mode = if self.swarm {
                FuzzMode::Swarm {
                    seed: run_seed,
                    action_probability,
                }
            } else {
                FuzzMode::RandomWalk
            };
            let (mut session, verifier) = self
                .driver
                .new_session(run_id)
                .expect("driver initiate failed");

            let mut strategy = FuzzStrategy {
                worker_id: self.worker_id,
                run_id,
                rng: StdRng::seed_from_u64(run_seed),
                test_start,
                test_deadline: test_start + self.time_limit,
                worker_tx: self.worker_tx.clone(),
                mode,
                excluded: HashMap::new(),
            };

            let mut output_writer = self
                .output_writer
                .lock()
                .expect("failed to acquire lock for output writer");
            let mut trace_writer = output_writer
                .trace_writer(run_id)
                .expect("initializing trace writer failed");

            if let Err(error) = self.worker_tx.send(WorkerMessage::Start {
                worker_id: self.worker_id,
                run_id,
            }) {
                log::error!("failed to send worker message: {:#}", error);
            }

            let result = runner::run(
                &mut session,
                &mut strategy,
                verifier,
                &mut trace_writer,
                self.interrupted.clone(),
            );

            log::info!(
                "worker {} finished runner::run (iteration={iteration}, run_id={run_id}, result={result:?})",
                self.worker_id,
            );

            if log::log_enabled!(log::Level::Debug) && self.swarm {
                for (hash, templates) in strategy.excluded {
                    let mut buffer = String::new();
                    write!(
                        buffer,
                        "worker {} excluded with hash {hash}): ",
                        self.worker_id
                    )
                    .expect("write failed");
                    for template in templates {
                        write!(buffer, "\n{}, ", Formatted(&template))
                            .expect("write failed");
                    }
                    log::debug!("{}", buffer);
                }
            }

            log::debug!("terminating session");
            if let Err(error) = session.terminate() {
                log::error!("failed to terminate session: {:#}", error);
            }
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum ExitReason {
    TimeLimit,
    Interrupted,
    AllDefinite,
}

enum FuzzMode {
    Swarm { seed: u64, action_probability: f64 },
    RandomWalk,
}

struct FuzzStrategy<Rng, Session: InterfaceSession> {
    worker_id: WorkerId,
    run_id: RunId,
    rng: Rng,
    test_start: Time,
    test_deadline: Time,
    worker_tx: crossbeam_channel::Sender<WorkerMessage<Session>>,
    mode: FuzzMode,

    // debugging
    excluded: HashMap<u64, Vec<Session::ActionTemplate>>,
}

impl<Rng: TryRng + RngExt, Session: InterfaceSession>
    FuzzStrategy<Rng, Session>
{
    const SWARM_HASH_BIT_WIDTH: usize = 32;

    #[hotpath::measure]
    fn swarm(
        seed: u64,
        action_probability: f64,
        tree: &Tree<Session::ActionTemplate>,
    ) -> Option<Tree<Session::ActionTemplate>> {
        tree.clone()
            .filter(&|template| {
                let mut hasher = DefaultHasher::default();
                hasher.write_u64(seed);
                template.category_hash(&mut hasher);
                let n = ((hasher.finish() >> (64 - Self::SWARM_HASH_BIT_WIDTH))
                    as f64)
                    / 2.0_f64.powf(Self::SWARM_HASH_BIT_WIDTH as f64);
                n < action_probability
            })
            .prune()
    }

    #[hotpath::measure]
    fn pick_action(
        &mut self,
        _state: &Session::State,
        tree: Tree<Session::ActionTemplate>,
    ) -> Result<Session::Action> {
        // TODO: somehow inject the following old behavior for browser fuzzing specifically. Some
        // set of "tactics" that can filter the action tree?

        // let tree = if is_within_domain(&state.url, &self.origin) {
        //     tree
        // } else {
        //     tree.filter(&|a| matches!(a, BrowserAction::Back))
        // }
        //

        if log::log_enabled!(log::Level::Debug)
            && let FuzzMode::Swarm {
                seed,
                action_probability,
            } = self.mode
        {
            for template in tree.values() {
                let mut hasher = DefaultHasher::default();
                hasher.write_u64(seed);
                template.category_hash(&mut hasher);
                let hash = hasher.finish();
                let n = ((hash >> Self::SWARM_HASH_BIT_WIDTH) as f64)
                    / 2.0_f64.powf(Self::SWARM_HASH_BIT_WIDTH as f64);
                let included = n < action_probability;
                if !included {
                    let entry = self.excluded.entry(hash).or_default();
                    // Poor man's set, as we can't require Ord or Hash for actions.
                    if !entry.contains(template) {
                        entry.push(template.clone());
                    }
                }
            }
        }

        let tree = match self.mode {
            FuzzMode::Swarm {
                seed,
                action_probability,
            } => Self::swarm(seed, action_probability, &tree).unwrap_or(tree),
            FuzzMode::RandomWalk => tree,
        }
        .prune()
        .ok_or_else(|| anyhow::anyhow!("no actions available"))?;

        let template = tree.pick(&mut self.rng)?.clone();
        let action = template.generate(&mut self.rng);
        Ok(action)
    }
}

impl<Session: InterfaceSession, Rng: TryRng + RngExt> RunStrategy<Session>
    for FuzzStrategy<Rng, Session>
{
    type StopValue = ExitReason;

    #[hotpath::measure]
    fn on_new_state(
        &mut self,
        state: &Session::State,
        tree: Tree<Session::ActionTemplate>,
        _last_action: Option<&Session::Action>,
        _snapshots: &[Snapshot],
        properties: PropertiesState<'_>,
    ) -> anyhow::Result<ControlFlow<Self::StopValue, Session::Action>> {
        if properties.all_definite {
            log::info!("all properties are definite, stopping");
            return Ok(ControlFlow::Stop(ExitReason::AllDefinite));
        }

        if state.timestamp() >= self.test_deadline {
            log::info!("time limit reached, stopping");
            return Ok(ControlFlow::Stop(ExitReason::TimeLimit));
        }

        let action = self.pick_action(state, tree)?;

        let time_relative = std::time::Duration::from_micros(
            state
                .timestamp()
                .as_micros()
                .saturating_sub(self.test_start.as_micros()),
        );

        if let Err(error) = self.worker_tx.send(WorkerMessage::Step {
            worker_id: self.worker_id,
            run_id: self.run_id,
            time_relative,
            action_selected: action.clone(),
            violations: properties.violations.to_vec(),
        }) {
            log::error!("failed to send worker message: {:#}", error);
        }

        Ok(ControlFlow::Continue(action))
    }

    fn on_interrupted(&mut self) -> anyhow::Result<Self::StopValue> {
        Ok(ExitReason::Interrupted)
    }
}
