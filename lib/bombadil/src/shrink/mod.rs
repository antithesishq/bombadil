//! Shrinking of traces with violations down to easier-to-understand examples.
//! Needs a single property to target. It uses a calibration pass of the original
//! trace to estimate reproducibility/flakiness, then uses that metric when verifying
//! generated candidates. Writes the currently known smallest trace to the output
//! direction as it searches.

use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    ffi::OsString,
    hash::{DefaultHasher, Hash, Hasher},
    num::NonZeroUsize,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use anyhow::{Result, anyhow, bail};
use bombadil_schema::Time;
use crossbeam_channel as mpmc;

use crate::{
    driver::{
        ActionTemplate, InterfaceDriver, InterfaceSession, RunId, TraceWriter,
    },
    runner::{self, ControlFlow, PropertiesState, RunStrategy},
    specification::domain::Snapshot,
    tree::Tree,
};

mod trace_actions;

pub use trace_actions::{TraceActions, load_trace_actions};

#[derive(Debug, Clone)]
pub struct ShrinkSettings {
    pub calibration_runs: NonZeroUsize,
    pub confirmation_runs_max: NonZeroUsize,
    pub confirmation_ratio_min: f64,
    pub pool_size: NonZeroUsize,
    /// The actual number depends on the calibrated reproduction rate.
    pub candidate_tries_max: NonZeroUsize,
}

impl Default for ShrinkSettings {
    fn default() -> Self {
        fn non_zero(value: usize) -> NonZeroUsize {
            NonZeroUsize::new(value).expect("invalid non zero usize")
        }
        ShrinkSettings {
            calibration_runs: non_zero(3),
            confirmation_runs_max: non_zero(5),
            confirmation_ratio_min: 0.25,
            pool_size: non_zero(1),
            candidate_tries_max: non_zero(5),
        }
    }
}

pub struct ShrinkOptions<Driver: InterfaceDriver, MakeStrategy, MakeWriter> {
    pub driver: Arc<Driver>,
    pub strategy: MakeStrategy,
    pub trace_writer: MakeWriter,
    pub trace: TraceActions<<Driver::Session as InterfaceSession>::Action>,
    pub property_name: Option<String>,
    pub output_path: PathBuf,
    pub output_path_overwrite: bool,
    pub parallelism: NonZeroUsize,
    pub interrupted: Arc<AtomicBool>,
    pub time_limit: Option<Duration>,
    pub settings: ShrinkSettings,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShrinkExitReason {
    /// No smaller failing sequence found.
    Minimal,
    TimeLimit,
    Interrupted,
}

#[derive(Debug, Clone)]
pub struct Reproductions {
    pub hit_count: usize,
    pub run_count: usize,
}

#[derive(Debug, Clone)]
pub struct ShrinkResult {
    pub property_name: String,
    pub exit_reason: ShrinkExitReason,
    pub trace_length_original: usize,
    pub trace_length_truncated: usize,
    pub trace_length_shrunk: usize,
    pub calibration: Reproductions,
    pub confirmation: Reproductions,
    pub run_count: u64,
    pub elapsed: Duration,
}

const ATTEMPTS_DIRECTORY_NAME: &str = ".shrink-attempts";

/// Acceptable risk of discarding a failing candidate that happened not to
/// reproduce in any of its tries.
const CANDIDATE_MISS_PROBABILITY_MAX: f64 = 0.1;

pub fn shrink<Driver, Inner, MakeStrategy, Writer, MakeWriter>(
    options: ShrinkOptions<Driver, MakeStrategy, MakeWriter>,
) -> Result<ShrinkResult>
where
    Driver: InterfaceDriver + Send + Sync + 'static,
    <Driver::Session as InterfaceSession>::Action: Send + Sync + 'static,
    Inner: RunStrategy<Driver::Session>,
    MakeStrategy: Fn() -> Inner + Send + Sync + 'static,
    Writer: TraceWriter<Driver::Session>,
    MakeWriter: Fn(PathBuf) -> Result<Writer> + Send + Sync + 'static,
{
    let ShrinkOptions {
        driver,
        strategy,
        trace_writer,
        trace,
        property_name,
        output_path,
        output_path_overwrite,
        parallelism,
        interrupted,
        time_limit,
        settings,
    } = options;

    let started = Instant::now();
    let deadline = time_limit.map(|limit| started + limit);

    let (property_name, trace_length_truncated) =
        trace.select_property(property_name.as_deref())?;
    let trace_length_original = trace.actions.len();
    let actions: Arc<[_]> = {
        let mut actions = trace.actions;
        actions.truncate(trace_length_truncated);
        actions.into()
    };

    if output_path.join("trace.jsonl").try_exists()? && !output_path_overwrite {
        bail!(
            "trace.jsonl already exists at {}. \
             Use --output-path-overwrite to overwrite, or choose a different --output-path.",
            output_path.display(),
        );
    }
    let attempts_path = output_path.join(ATTEMPTS_DIRECTORY_NAME);
    if attempts_path.try_exists()? {
        std::fs::remove_dir_all(&attempts_path)?;
    }
    std::fs::create_dir_all(&attempts_path)?;

    let bound = Arc::new(AtomicUsize::new(trace_length_truncated + 1));
    let worker_shared = Arc::new(WorkerShared {
        driver,
        strategy,
        trace_writer,
        actions,
        property_name: property_name.clone(),
        bound: bound.clone(),
        interrupted: interrupted.clone(),
        attempts_path: attempts_path.clone(),
    });

    let (job_sender, job_receiver) = mpmc::unbounded::<Job>();
    let (result_sender, result_receiver) = mpmc::unbounded::<JobResult>();
    let mut workers = Vec::with_capacity(parallelism.get());
    for _ in 0..parallelism.get() {
        let worker_shared = worker_shared.clone();
        let job_receiver = job_receiver.clone();
        let result_sender = result_sender.clone();
        workers.push(thread::spawn(move || {
            while let Ok(job) = job_receiver.recv() {
                let (outcome, executed) = worker_shared.run(&job);
                if result_sender
                    .send(JobResult {
                        job,
                        outcome,
                        executed,
                    })
                    .is_err()
                {
                    break;
                }
            }
        }));
    }
    drop(result_sender);

    let mut coordinator = Coordinator {
        settings,
        output_path: output_path.clone(),
        attempts_path: attempts_path.clone(),
        bound,
        truncated: (0..trace_length_truncated as u32).collect(),
        run_id_next: 0,
        in_flight: 0,
        calibration: Calibration::Pending {
            dispatched: 0,
            successes: Vec::new(),
            run_count: 0,
            errors: Vec::new(),
        },
        candidate_tries_max: 1,
        confirmation_hit_count_min: 1,
        pool: Vec::new(),
        files_best: Vec::new(),
        pending: BTreeMap::new(),
        retries: VecDeque::new(),
        tried: HashMap::new(),
        generator: None,
    };

    println!(
        "Shrinking for property `{property_name}`: \
         {trace_length_original} actions, \
         {trace_length_truncated} up to its earliest violation"
    );
    println!(
        "Calibrating by replaying the trace {} time(s)...",
        coordinator.settings.calibration_runs
    );

    let mut idle = parallelism.get();
    let loop_result = (|| -> Result<ShrinkExitReason> {
        loop {
            if interrupted.load(Ordering::SeqCst) {
                return Ok(ShrinkExitReason::Interrupted);
            }
            if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                return Ok(ShrinkExitReason::TimeLimit);
            }
            while idle > 0
                && let Some(job) = coordinator.next_job()
            {
                job_sender
                    .send(job)
                    .map_err(|_| anyhow!("all shrink workers stopped"))?;
                idle -= 1;
                coordinator.in_flight += 1;
            }
            if coordinator.in_flight == 0 {
                return Ok(ShrinkExitReason::Minimal);
            }
            match result_receiver.recv_timeout(Duration::from_millis(100)) {
                Ok(result) => {
                    idle += 1;
                    coordinator.in_flight -= 1;
                    coordinator.handle(result)?;
                }
                Err(mpmc::RecvTimeoutError::Timeout) => {}
                Err(mpmc::RecvTimeoutError::Disconnected) => {
                    bail!("all shrink workers stopped")
                }
            }
        }
    })();

    interrupted.store(true, Ordering::SeqCst);
    drop(job_sender);
    for worker in workers {
        if worker.join().is_err() {
            log::error!("shrink worker panicked");
        }
    }
    let attempts_removed = std::fs::remove_dir_all(&attempts_path);
    let exit_reason = loop_result?;
    attempts_removed?;

    let Some(best) = coordinator.pool.first() else {
        bail!("shrinking stopped before calibration finished");
    };

    let calibration = match &coordinator.calibration {
        Calibration::Done {
            hit_count,
            run_count,
        } => Reproductions {
            hit_count: *hit_count,
            run_count: *run_count,
        },
        Calibration::Pending { .. } => unreachable!("best exists"),
    };
    Ok(ShrinkResult {
        property_name,
        exit_reason,
        trace_length_original,
        trace_length_truncated,
        trace_length_shrunk: best.sequence.len(),
        calibration,
        confirmation: Reproductions {
            hit_count: best.hit_count,
            run_count: best.run_count,
        },
        run_count: coordinator.run_id_next,
        elapsed: started.elapsed(),
    })
}

/// Indices into the truncated original actions.
type Sequence = Arc<[u32]>;

fn sequence_hash(sequence: &[u32]) -> u64 {
    let mut hasher = DefaultHasher::new();
    sequence.hash(&mut hasher);
    hasher.finish()
}

#[derive(Debug)]
enum JobKind {
    Calibration,
    Candidate,
    Confirmation { entry: RunId },
}

#[derive(Debug)]
struct Job {
    run_id: RunId,
    kind: JobKind,
    sequence: Sequence,
}

struct JobResult {
    job: Job,
    outcome: Result<RunOutcome>,
    executed: Vec<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RunOutcome {
    Reproduced,
    NotReproduced,
    Pruned,
    Interrupted,
}

struct WorkerShared<Driver: InterfaceDriver, MakeStrategy, MakeWriter> {
    driver: Arc<Driver>,
    strategy: MakeStrategy,
    trace_writer: MakeWriter,
    actions: Arc<[<Driver::Session as InterfaceSession>::Action]>,
    property_name: String,
    bound: Arc<AtomicUsize>,
    interrupted: Arc<AtomicBool>,
    attempts_path: PathBuf,
}

impl<Driver, Inner, MakeStrategy, Writer, MakeWriter>
    WorkerShared<Driver, MakeStrategy, MakeWriter>
where
    Driver: InterfaceDriver,
    Inner: RunStrategy<Driver::Session>,
    MakeStrategy: Fn() -> Inner,
    Writer: TraceWriter<Driver::Session>,
    MakeWriter: Fn(PathBuf) -> Result<Writer>,
{
    fn run(&self, job: &Job) -> (Result<RunOutcome>, Vec<u32>) {
        let mut strategy = ShrinkStrategy {
            inner: (self.strategy)(),
            property_name: &self.property_name,
            actions: &self.actions,
            candidate: &job.sequence,
            position: 0,
            executed: Vec::with_capacity(job.sequence.len()),
            bound: &self.bound,
        };
        let outcome = (|| {
            let directory = self.attempts_path.join(job.run_id.to_string());
            let mut writer = (self.trace_writer)(directory)?;
            let (mut session, verifier) =
                self.driver.new_session(job.run_id)?;
            runner::run(
                &mut session,
                &mut strategy,
                verifier,
                &mut writer,
                self.interrupted.clone(),
            )
            // Drops (and flushes) the writer before the coordinator moves
            // the run directory.
        })();
        if let Err(error) = &outcome {
            log::warn!("shrink run {} failed: {:#}", job.run_id, error);
        }
        (outcome, strategy.executed)
    }
}

struct ShrinkStrategy<'a, Session: InterfaceSession, Inner> {
    inner: Inner,
    property_name: &'a str,
    actions: &'a [Session::Action],
    candidate: &'a [u32],
    position: usize,
    executed: Vec<u32>,
    bound: &'a AtomicUsize,
}

impl<Session: InterfaceSession, Inner: RunStrategy<Session>>
    RunStrategy<Session> for ShrinkStrategy<'_, Session, Inner>
{
    type StopValue = RunOutcome;

    fn on_new_state(
        &mut self,
        state: &Session::State,
        tree: Tree<Session::ActionTemplate>,
        last_action: Option<&Session::Action>,
        snapshots: &[Snapshot],
        properties: PropertiesState,
    ) -> Result<ControlFlow<RunOutcome, Session::Action>> {
        if properties
            .violations
            .iter()
            .any(|violation| violation.name == self.property_name)
        {
            return Ok(ControlFlow::Stop(RunOutcome::Reproduced));
        }

        let available = tree.clone();
        if let ControlFlow::Stop(_) = self.inner.on_new_state(
            state,
            tree,
            last_action,
            snapshots,
            properties,
        )? {
            return Ok(ControlFlow::Stop(RunOutcome::NotReproduced));
        }

        let available = available.values();
        while let Some(&index) = self.candidate.get(self.position) {
            self.position += 1;
            let action = &self.actions[index as usize];
            if available.iter().any(|template| template.accepts(action)) {
                if self.executed.len() + 1 >= self.bound.load(Ordering::SeqCst)
                {
                    return Ok(ControlFlow::Stop(RunOutcome::Pruned));
                }
                self.executed.push(index);
                return Ok(ControlFlow::Continue(action.clone()));
            }
            log::debug!("skipping unavailable action: {:?}", action);
        }
        Ok(ControlFlow::Stop(RunOutcome::NotReproduced))
    }

    fn deadline(&self) -> Option<Time> {
        self.inner.deadline()
    }

    fn constrain(
        &self,
        state: &Session::State,
        tree: Tree<Session::ActionTemplate>,
    ) -> Tree<Session::ActionTemplate> {
        self.inner.constrain(state, tree)
    }

    fn on_time_limit(&mut self) -> Result<RunOutcome> {
        Ok(RunOutcome::NotReproduced)
    }

    fn on_stalled(&mut self) -> Result<RunOutcome> {
        log::warn!("shrink run stalled");
        Ok(RunOutcome::NotReproduced)
    }

    fn on_interrupted(&mut self) -> Result<RunOutcome> {
        Ok(RunOutcome::Interrupted)
    }
}

enum Calibration {
    Pending {
        dispatched: usize,
        run_count: usize,
        successes: Vec<(Vec<u32>, PathBuf)>,
        errors: Vec<String>,
    },
    Done {
        hit_count: usize,
        run_count: usize,
    },
}

struct PoolEntry {
    sequence: Sequence,
    hit_count: usize,
    run_count: usize,
    /// Chunk deletion has been done with this as its base.
    exhausted: bool,
}

struct Pending {
    sequence: Sequence,
    directory: PathBuf,
    hit_count: usize,
    run_count: usize,
    in_flight: usize,
}

struct Coordinator {
    settings: ShrinkSettings,
    output_path: PathBuf,
    attempts_path: PathBuf,
    /// Length of the smallest confirmed failing sequence. Runs reaching it
    /// are pruned.
    bound: Arc<AtomicUsize>,
    truncated: Sequence,
    run_id_next: u64,
    in_flight: usize,
    calibration: Calibration,
    candidate_tries_max: usize,
    confirmation_hit_count_min: usize,
    /// Smallest first. The first is the result in the output directory.
    pool: Vec<PoolEntry>,
    /// Files in the output directory that belong to the result.
    files_best: Vec<OsString>,
    pending: BTreeMap<RunId, Pending>,
    retries: VecDeque<Sequence>,
    /// Dispatch counts by sequence hash.
    tried: HashMap<u64, usize>,
    generator: Option<ChunkDeletion>,
}

impl Coordinator {
    fn bound(&self) -> usize {
        self.bound.load(Ordering::SeqCst)
    }

    fn run_id_next(&mut self) -> RunId {
        let run_id = RunId(self.run_id_next);
        self.run_id_next += 1;
        run_id
    }

    fn run_directory(&self, run_id: RunId) -> PathBuf {
        self.attempts_path.join(run_id.to_string())
    }

    fn remove_run_directory(&self, run_id: RunId) {
        let directory = self.run_directory(run_id);
        if let Err(error) = std::fs::remove_dir_all(&directory)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            log::warn!("failed to remove {}: {}", directory.display(), error);
        }
    }

    fn next_job(&mut self) -> Option<Job> {
        if let Calibration::Pending { dispatched, .. } = &mut self.calibration {
            if *dispatched < self.settings.calibration_runs.get() {
                *dispatched += 1;
                let sequence = self.truncated.clone();
                return Some(Job {
                    run_id: self.run_id_next(),
                    kind: JobKind::Calibration,
                    sequence,
                });
            }
            return None;
        }

        // Confirmations first, smallest sequence first.
        let confirmation_runs_max = self.settings.confirmation_runs_max.get();
        let hit_count_min = self.confirmation_hit_count_min;
        let mut confirmable: Vec<_> = self
            .pending
            .iter()
            .filter(|(_, pending)| {
                pending.run_count + pending.in_flight < confirmation_runs_max
                    && pending.hit_count + pending.in_flight < hit_count_min
            })
            .map(|(&run_id, pending)| (pending.sequence.len(), run_id))
            .collect();
        confirmable.sort();
        if let Some(&(_, entry)) = confirmable.first() {
            let pending = self.pending.get_mut(&entry).expect("pending exists");
            pending.in_flight += 1;
            let sequence = pending.sequence.clone();
            return Some(Job {
                run_id: self.run_id_next(),
                kind: JobKind::Confirmation { entry },
                sequence,
            });
        }

        while let Some(sequence) = self.retries.pop_front() {
            if sequence.len() < self.bound() {
                *self.tried.entry(sequence_hash(&sequence)).or_default() += 1;
                return Some(Job {
                    run_id: self.run_id_next(),
                    kind: JobKind::Candidate,
                    sequence,
                });
            }
        }

        loop {
            if self.generator.is_none() {
                let entry =
                    self.pool.iter_mut().find(|entry| !entry.exhausted)?;
                entry.exhausted = true;
                log::info!(
                    "deleting chunks from a sequence of {} actions",
                    entry.sequence.len()
                );
                self.generator = Some(ChunkDeletion::new(
                    entry.sequence.clone(),
                    entry.sequence.len() / 2,
                ));
            }
            let generator = self.generator.as_mut().expect("generator exists");
            match generator.next() {
                Some(candidate) => {
                    let candidate: Sequence = candidate.into();
                    let tries = self
                        .tried
                        .entry(sequence_hash(&candidate))
                        .or_default();
                    if *tries > 0 {
                        continue;
                    }
                    *tries += 1;
                    return Some(Job {
                        run_id: self.run_id_next(),
                        kind: JobKind::Candidate,
                        sequence: candidate,
                    });
                }
                None => self.generator = None,
            }
        }
    }

    fn handle(&mut self, result: JobResult) -> Result<()> {
        let JobResult {
            job,
            outcome,
            executed,
        } = result;
        let reproduced = matches!(outcome, Ok(RunOutcome::Reproduced));
        match job.kind {
            JobKind::Calibration => {
                let directory = self.run_directory(job.run_id);
                if !reproduced {
                    self.remove_run_directory(job.run_id);
                }
                let Calibration::Pending {
                    run_count,
                    successes,
                    errors,
                    ..
                } = &mut self.calibration
                else {
                    unreachable!("calibration result after calibration")
                };
                *run_count += 1;
                match outcome {
                    Ok(RunOutcome::Reproduced) => {
                        successes.push((executed, directory))
                    }
                    Ok(_) => {}
                    Err(error) => errors.push(format!("{error:#}")),
                }
                if *run_count == self.settings.calibration_runs.get() {
                    self.finish_calibration()?;
                }
            }
            JobKind::Candidate => {
                if reproduced {
                    self.on_success(job.run_id, executed);
                } else {
                    self.remove_run_directory(job.run_id);
                    let tries = self
                        .tried
                        .get(&sequence_hash(&job.sequence))
                        .copied()
                        .unwrap_or(0);
                    let retry = match outcome {
                        Ok(RunOutcome::NotReproduced) | Err(_) => true,
                        Ok(RunOutcome::Pruned | RunOutcome::Interrupted) => {
                            false
                        }
                        Ok(RunOutcome::Reproduced) => unreachable!(),
                    };
                    if retry
                        && tries < self.candidate_tries_max
                        && job.sequence.len() < self.bound()
                    {
                        self.retries.push_back(job.sequence);
                    }
                }
            }
            JobKind::Confirmation { entry } => {
                if reproduced && executed.len() < job.sequence.len() {
                    // Failed earlier this time, a new candidate.
                    self.on_success(job.run_id, executed);
                } else {
                    self.remove_run_directory(job.run_id);
                }
                let confirmation_runs_max =
                    self.settings.confirmation_runs_max.get();
                let hit_count_min = self.confirmation_hit_count_min;
                let Some(pending) = self.pending.get_mut(&entry) else {
                    return Ok(());
                };
                pending.in_flight -= 1;
                pending.run_count += 1;
                if reproduced {
                    pending.hit_count += 1;
                }
                if pending.hit_count >= hit_count_min {
                    self.confirm(entry)?;
                } else if pending.run_count - pending.hit_count
                    > confirmation_runs_max - hit_count_min
                {
                    let pending =
                        self.pending.remove(&entry).expect("pending exists");
                    println!(
                        "  {} actions: rejected, reproduced {}/{}",
                        pending.sequence.len(),
                        pending.hit_count,
                        pending.run_count
                    );
                    self.remove_run_directory(entry);
                }
            }
        }
        Ok(())
    }

    fn finish_calibration(&mut self) -> Result<()> {
        let Calibration::Pending {
            run_count,
            successes,
            errors,
            ..
        } = std::mem::replace(
            &mut self.calibration,
            Calibration::Done {
                hit_count: 0,
                run_count: 0,
            },
        )
        else {
            unreachable!("calibration already finished")
        };
        let hit_count = successes.len();
        self.calibration = Calibration::Done {
            hit_count,
            run_count,
        };
        if hit_count == 0 {
            let mut message = format!(
                "the violation did not reproduce in any of the {run_count} calibration run(s)"
            );
            if let Some(error) = errors.first() {
                message.push_str(&format!(
                    " ({} run(s) failed, first error: {error})",
                    errors.len()
                ));
            }
            bail!(message);
        }

        let rate = hit_count as f64 / run_count as f64;
        self.candidate_tries_max = if rate >= 1.0 {
            1
        } else {
            (CANDIDATE_MISS_PROBABILITY_MAX.ln() / (1.0 - rate).ln()).ceil()
                as usize
        }
        .clamp(1, self.settings.candidate_tries_max.get());
        self.confirmation_hit_count_min =
            ((self.settings.confirmation_ratio_min
                * rate
                * self.settings.confirmation_runs_max.get() as f64)
                .ceil() as usize)
                .clamp(1, self.settings.confirmation_runs_max.get());

        let mut successes = successes;
        successes.sort_by_key(|(executed, _)| executed.len());
        let mut successes = successes.into_iter();
        let (executed, directory) = successes.next().expect("hit_count > 0");
        for (_, directory) in successes {
            let _ = std::fs::remove_dir_all(directory);
        }
        println!(
            "Reproduced in {hit_count}/{run_count} calibration run(s), retrying candidates up to {} time(s)",
            self.candidate_tries_max
        );
        self.promote(&directory)?;
        self.bound.store(executed.len(), Ordering::SeqCst);
        println!("  {} actions", executed.len());
        self.pool.push(PoolEntry {
            sequence: executed.into(),
            hit_count,
            run_count,
            exhausted: false,
        });
        Ok(())
    }

    fn on_success(&mut self, run_id: RunId, executed: Vec<u32>) {
        let known = |sequence: &Sequence| **sequence == *executed;
        if executed.len() >= self.bound()
            || self.pool.iter().any(|entry| known(&entry.sequence))
            || self
                .pending
                .values()
                .any(|pending| known(&pending.sequence))
        {
            self.remove_run_directory(run_id);
            return;
        }
        log::info!("{} actions reproduced, confirming", executed.len());
        self.pending.insert(
            run_id,
            Pending {
                sequence: executed.into(),
                directory: self.run_directory(run_id),
                hit_count: 0,
                run_count: 0,
                in_flight: 0,
            },
        );
    }

    fn confirm(&mut self, entry: RunId) -> Result<()> {
        let pending = self.pending.remove(&entry).expect("pending exists");
        let length = pending.sequence.len();
        let entry = PoolEntry {
            sequence: pending.sequence,
            hit_count: pending.hit_count,
            run_count: pending.run_count,
            exhausted: false,
        };
        if length < self.bound() {
            println!(
                "  {length} actions (reproduced {}/{})",
                entry.hit_count, entry.run_count
            );
            self.promote(&pending.directory)?;
            self.bound.store(length, Ordering::SeqCst);
            self.pool.insert(0, entry);
            // Continue at the granularity reached.
            let chunk = self
                .generator
                .as_ref()
                .map_or(length / 2, |generator| generator.chunk)
                .min(length / 2);
            self.pool[0].exhausted = true;
            self.generator =
                Some(ChunkDeletion::new(self.pool[0].sequence.clone(), chunk));
            let obsolete: Vec<RunId> = self
                .pending
                .iter()
                .filter(|(_, pending)| pending.sequence.len() >= length)
                .map(|(&run_id, _)| run_id)
                .collect();
            for run_id in obsolete {
                self.pending.remove(&run_id);
                self.remove_run_directory(run_id);
            }
            self.retries.retain(|sequence| sequence.len() < length);
        } else {
            let _ = std::fs::remove_dir_all(&pending.directory);
            let position = self
                .pool
                .iter()
                .position(|existing| existing.sequence.len() > length)
                .unwrap_or(self.pool.len());
            self.pool.insert(position, entry);
        }
        self.pool.truncate(self.settings.pool_size.get());
        Ok(())
    }

    /// Replace the result in the output directory with a run's trace.
    fn promote(&mut self, directory: &Path) -> Result<()> {
        let previous = self.attempts_path.join("previous");
        std::fs::create_dir_all(&previous)?;
        for name in self.files_best.drain(..) {
            std::fs::rename(
                self.output_path.join(&name),
                previous.join(&name),
            )?;
        }
        for entry in std::fs::read_dir(directory)? {
            let name = entry?.file_name();
            let target = self.output_path.join(&name);
            // Left over from an earlier shrink, when overwriting.
            if target.is_dir() {
                std::fs::remove_dir_all(&target)?;
            } else if target.try_exists()? {
                std::fs::remove_file(&target)?;
            }
            std::fs::rename(directory.join(&name), &target)?;
            self.files_best.push(name);
        }
        std::fs::remove_dir(directory)?;
        std::fs::remove_dir_all(&previous)?;
        Ok(())
    }
}

/// Deletes chunks of decreasing size, like ddmin and afl-tmin. For each
/// size it first tries keeping a single chunk (cheap to run), then removing
/// one.
struct ChunkDeletion {
    base: Sequence,
    chunk: usize,
    position: usize,
    complements: bool,
}

impl ChunkDeletion {
    fn new(base: Sequence, chunk: usize) -> Self {
        ChunkDeletion {
            base,
            chunk: chunk.max(1),
            position: 0,
            complements: false,
        }
    }

    fn next(&mut self) -> Option<Vec<u32>> {
        let length = self.base.len();
        loop {
            if self.chunk == 0 || length == 0 {
                return None;
            }
            let chunks = length.div_ceil(self.chunk);
            // With only two chunks, keeping one is the same as removing
            // the other.
            if self.position >= chunks || (!self.complements && chunks <= 2) {
                self.position = 0;
                if self.complements {
                    self.complements = false;
                    self.chunk =
                        if self.chunk == 1 { 0 } else { self.chunk / 2 };
                } else {
                    self.complements = true;
                }
                continue;
            }
            let start = self.position * self.chunk;
            let end = (start + self.chunk).min(length);
            self.position += 1;
            let candidate: Vec<u32> = if self.complements {
                self.base[..start]
                    .iter()
                    .chain(&self.base[end..])
                    .copied()
                    .collect()
            } else {
                self.base[start..end].to_vec()
            };
            if !candidate.is_empty() {
                return Some(candidate);
            }
        }
    }
}

#[cfg(test)]
mod fake_tests;

#[cfg(test)]
mod tests {
    use super::*;

    fn all_candidates(base: Vec<u32>, chunk: usize) -> Vec<Vec<u32>> {
        let mut generator = ChunkDeletion::new(base.into(), chunk);
        std::iter::from_fn(|| generator.next()).collect()
    }

    #[test]
    fn chunk_deletion_halves_then_quarters() {
        let candidates = all_candidates((0..4).collect(), 2);
        assert_eq!(
            candidates,
            vec![
                // chunk 2: complements only
                vec![2, 3],
                vec![0, 1],
                // chunk 1: subsets
                vec![0],
                vec![1],
                vec![2],
                vec![3],
                // chunk 1: complements
                vec![1, 2, 3],
                vec![0, 2, 3],
                vec![0, 1, 3],
                vec![0, 1, 2],
            ]
        );
    }

    #[test]
    fn chunk_deletion_single_element() {
        assert_eq!(all_candidates(vec![7], 0), Vec::<Vec<u32>>::new());
    }
}
