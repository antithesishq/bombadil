use anyhow::{Result, anyhow};
use bombadil_schema::Time;
use crossbeam_channel as mpmc;
use std::{
    collections::{BTreeMap, HashMap},
    fmt::Display,
    fmt::Write as _,
    hash::{DefaultHasher, Hasher},
    io::Write,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, SystemTime},
};

use rand::{Rng, RngExt, SeedableRng, TryRng, prelude::StdRng};
use stdx::ring_buffer::RingBuffer;

use crate::{
    driver::{ActionTemplate, InterfaceDriver, InterfaceSession, RunState},
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

#[derive(Clone, Copy)]
struct WorkerId(usize);

impl Display for WorkerId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Clone, Copy, PartialOrd, Ord, PartialEq, Eq)]
struct RunId(usize);

impl RunId {
    pub fn next(&self) -> Self {
        RunId(self.0 + 1)
    }
}

impl Display for RunId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

enum WorkerMessage<Session: InterfaceSession> {
    Step {
        worker_id: WorkerId,
        run_id: RunId,
        time_relative: Duration,
        violations: Vec<PropertyViolation>,
        action_selected: Session::Action,
    },
}

struct Worker<D: InterfaceDriver> {
    worker_id: WorkerId,
    run_id: RunId,
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
        self.run_id = run_id;
        self.actions = RingBuffer::new(); // TODO: add `.clear()`
        self.violations_count = 0;
    }
}

pub fn fuzz<D: InterfaceDriver + Send + Sync + 'static, Rng: TryRng + RngExt>(
    mut rng: Rng,
    driver: Arc<D>,
    interrupted: Arc<AtomicBool>,
    time_limit_fuzz: Duration,
    time_limit_run: Duration,
    swarm: bool,
) -> Result<()>
where
    <<D as InterfaceDriver>::Session as InterfaceSession>::Action: Send,
{
    use std::fmt::Write;

    let fuzz_start = Time::from_system_time(SystemTime::now());
    let fuzz_deadline = fuzz_start + time_limit_fuzz;
    let (worker_tx, worker_rx) = mpmc::unbounded();

    let mut workers: Vec<Worker<D>> = Vec::with_capacity(FUZZ_WORKER_COUNT);
    for i in 0..FUZZ_WORKER_COUNT {
        let worker_tx = worker_tx.clone();
        let worker_id = WorkerId(i);
        let run_id = RunId(0);
        let driver = driver.clone();
        let seed = rng.next_u64();
        let interrupted = interrupted.clone();
        log::debug!("spawning {worker_id}");
        let handle = thread::spawn(move || {
            let fuzz_worker_thread = FuzzWorkerThread {
                driver,
                worker_tx,
                interrupted,
                seed,
                time_limit: time_limit_run,
                fuzz_deadline,
                worker_id,
                run_id,
                swarm,
            };
            if let Err(error) = fuzz_worker_thread.run() {
                log::error!("run failed: {error:#}");
            }
        });
        workers.push(Worker {
            worker_id,
            run_id,
            handle,
            actions: RingBuffer::new(),
            violations_count: 0,
        });
    }
    // Unless we drop this Sender this the channel will live forever, even after the workers have
    // finished, and the the loop below will never exit.
    drop(worker_tx);

    let mut property_violation_counts: BTreeMap<String, u64> = BTreeMap::new();
    while let Ok(message) = worker_rx.recv()
        && !interrupted.load(Ordering::SeqCst)
    {
        match message {
            WorkerMessage::Step {
                worker_id,
                run_id,
                time_relative,
                action_selected,
                violations,
            } => {
                let worker = &mut workers[worker_id.0];
                if run_id > worker.run_id {
                    worker.reset(run_id);
                }

                worker.actions.push((time_relative, action_selected));
                worker.violations_count += violations.len() as u64;
                for violation in violations {
                    log::info!(
                        "{}/{}, violation of {}: {:?}",
                        worker.worker_id,
                        worker.run_id,
                        violation.name,
                        violation.violation
                    );
                    *property_violation_counts
                        .entry(violation.name)
                        .or_default() += 1;
                }
            }
        }

        let mut buffer = String::new();
        const SEP: &str = "  ";
        write!(buffer, "\x1b[2J\x1b[H")?;
        writeln!(
            buffer,
            "{}",
            maybe_bold(format!(
                "{:^6}{SEP}{:^3}{SEP}{:^10}{SEP}{:>4}{SEP}{:^9}{SEP}Action",
                "Worker", "Run", "Violations", "SPS", "Time"
            ))
        )?;
        for worker in &workers {
            write!(buffer, "{:^6}", worker.worker_id.0)?;
            write!(buffer, "{SEP}")?;
            write!(buffer, "{:^3}", worker.run_id.0)?;
            write!(buffer, "{SEP}")?;
            write!(buffer, "{:^10}", worker.violations_count)?;
            write!(buffer, "{SEP}")?;
            if let Some((action_last_time, action_last)) = worker.actions.last()
            {
                if worker.actions.len() > 1
                    && let Some(states_per_second) =
                        worker.actions.first().map(|(action_first_time, _)| {
                            (worker.actions.len() as f64)
                                / action_last_time
                                    .checked_sub(*action_first_time)
                                    .expect("action times are not ordered")
                                    .as_secs_f64()
                        })
                {
                    write!(buffer, "{:>4.1}", states_per_second)?;
                } else {
                    write!(buffer, "{:>4}", "")?;
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
        if !property_violation_counts.is_empty() {
            writeln!(
                buffer,
                "{}\n",
                maybe_bold("Violated properties:".to_string()),
            )?;
        }
        for (property_name, count) in &property_violation_counts {
            writeln!(
                buffer,
                "{}: {}",
                property_name.clone(),
                if *count > 0 {
                    maybe_red(format!("{}", count))
                } else {
                    "0".into()
                },
            )?;
        }

        print!("{}", buffer);
        std::io::stdout().flush()?;
    }
    println!("Shutting down...");

    for worker in workers {
        worker
            .handle
            .join()
            .map_err(|_| anyhow!("fuzz run thread panicked"))?;
    }

    Ok(())
}

struct FuzzWorkerThread<D: InterfaceDriver> {
    driver: Arc<D>,
    worker_tx: mpmc::Sender<WorkerMessage<D::Session>>,
    interrupted: Arc<AtomicBool>,
    seed: u64,
    time_limit: Duration,
    fuzz_deadline: Time,
    worker_id: WorkerId,
    run_id: RunId,
    swarm: bool,
}

impl<D: InterfaceDriver> FuzzWorkerThread<D> {
    #[hotpath::measure]
    fn run(mut self) -> Result<()> {
        let mut rng = StdRng::seed_from_u64(self.seed);

        while !self.interrupted.load(Ordering::SeqCst) {
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
            let mut strategy = FuzzStrategy {
                worker_id: self.worker_id,
                run_id: self.run_id,
                rng: StdRng::seed_from_u64(run_seed),
                test_start,
                test_deadline: test_start + self.time_limit,
                worker_tx: self.worker_tx.clone(),
                mode,
                excluded: HashMap::new(),
            };
            let (mut session, verifier) = self.driver.initiate()?;

            let result = runner::run(
                &mut session,
                &mut strategy,
                verifier,
                self.interrupted.clone(),
            )?;
            log::debug!("worker {}: got result: {result:?}", self.worker_id);

            if log::log_enabled!(log::Level::Debug) && self.swarm {
                for (hash, templates) in strategy.excluded {
                    let mut buffer = String::new();
                    write!(
                        buffer,
                        "worker {} excluded with hash {hash}): ",
                        self.worker_id
                    )?;
                    for template in templates {
                        write!(buffer, "\n{}, ", Formatted(&template))?;
                    }
                    log::debug!("{}", buffer);
                }
            }
            self.run_id = self.run_id.next();
            if let Err(error) = session.terminate() {
                log::error!("failed to terminate session: {:#}", error);
            }
        }
        Ok(())
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
        // self.writer.write(
        //     state,
        //     last_action,
        //     snapshots,
        //     properties.violations,
        // )?;

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
