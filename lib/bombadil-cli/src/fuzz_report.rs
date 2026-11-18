use std::{fmt::Write as _, path::Path};

use anyhow::{Result, bail};
use bombadil::{
    fuzzer::{FuzzExitReason, FuzzResult},
    styled,
};

/// The summary printed after a campaign. `inspect_command` is what shows a
/// trace file given as its last argument.
pub fn render(
    result: &FuzzResult,
    output_path: &Path,
    inspect_command: &str,
) -> String {
    let run_ids = result.violation_run_ids();
    let runs = match result.runs_count {
        1 => "1 run".to_string(),
        n => format!("{n} runs"),
    };
    let findings = match run_ids.len() {
        0 => format!(" with no violations in {runs}"),
        n => format!(", finding violations in {n} of {runs}"),
    };
    let heading = styled::maybe_bold(match result.exit_reason {
        FuzzExitReason::ExitOnViolation => {
            format!("Fuzzing finished{findings}!")
        }
        FuzzExitReason::TimeLimit => {
            format!("Fuzzing finished after time limit{findings}!")
        }
        FuzzExitReason::Interrupted => {
            format!("Fuzzing was interrupted by SIGINT{findings}!")
        }
        FuzzExitReason::WorkersPanicked => {
            format!("Fuzzing stopped as all workers panicked{findings}!")
        }
    });

    let mut buffer = String::new();
    let heading = if run_ids.is_empty()
        && result.worker_panics.is_empty()
        && result.runs_failed_count == 0
    {
        heading
    } else {
        styled::maybe_red(heading)
    };
    writeln!(buffer, "\n{heading}").expect("write failed");
    for panic in &result.worker_panics {
        writeln!(buffer, "\nA fuzz worker panicked: {panic}")
            .expect("write failed");
    }
    if let Some((run_id, error)) = &result.run_failed_first {
        let failed = match result.runs_failed_count {
            1 => format!("Run {run_id} failed"),
            n => format!("{n} runs failed, the first being run {run_id}"),
        };
        writeln!(buffer, "\n{failed}: {error}").expect("write failed");
    }
    let Some(run_id) = run_ids.first() else {
        return buffer;
    };
    let lead = if run_ids.len() == 1 {
        "Inspect the violation using:"
    } else {
        "Inspect a violation like this (replacing the run ID in the path):"
    };
    let command = styled::maybe_italic(format!(
        "{inspect_command} {}",
        bombadil_trace::numbered_trace_path(output_path, run_id.0).display()
    ));
    writeln!(buffer, "\n{lead}\n\n  {command}").expect("write failed");
    buffer
}

/// Exits with code 2 if the campaign found violations. Failed runs and
/// worker panics are an error only without violations.
pub fn exit(result: &FuzzResult) -> Result<()> {
    if !result.property_violation_run_ids.is_empty() {
        std::process::exit(2);
    }
    if !result.worker_panics.is_empty() {
        bail!("{} fuzz worker(s) panicked", result.worker_panics.len());
    }
    if result.runs_failed_count > 0 {
        bail!("{} fuzz run(s) failed", result.runs_failed_count);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use bombadil::driver::RunId;
    use hegel::generators::{integers, sampled_from, text, vecs};
    use hegel::{Generator, TestCase};
    use std::collections::{BTreeMap, BTreeSet};

    #[hegel::composite]
    fn results(tc: &TestCase) -> FuzzResult {
        let runs_count = tc.draw(integers::<u64>().max_value(8));
        let mut property_violation_run_ids: BTreeMap<String, BTreeSet<RunId>> =
            BTreeMap::new();
        if runs_count > 0 {
            let run_ids = || {
                vecs(integers::<u64>().max_value(runs_count - 1)).min_size(1)
            };
            for name in tc.draw(vecs(text()).max_size(3)) {
                property_violation_run_ids
                    .entry(name)
                    .or_default()
                    .extend(tc.draw(run_ids()).into_iter().map(RunId));
            }
        }
        let runs_failed_count =
            tc.draw(integers::<u64>().max_value(runs_count));
        FuzzResult {
            runs_failed_count,
            run_failed_first: (runs_failed_count > 0)
                .then(|| (RunId(0), tc.draw(text()))),
            exit_reason: tc.draw_silent(sampled_from(vec![
                FuzzExitReason::TimeLimit,
                FuzzExitReason::Interrupted,
                FuzzExitReason::ExitOnViolation,
                FuzzExitReason::WorkersPanicked,
            ])),
            runs_count,
            property_violation_run_ids,
            worker_panics: tc.draw(vecs(text()).max_size(2)),
        }
    }

    #[hegel::test]
    fn inspect_command_names_a_violating_run(tc: TestCase) {
        let result = tc.draw(results().print_as_debug());
        let rendered = render(&result, Path::new("out/run.bin"), "inspect");
        let commands: Vec<&str> = rendered
            .split("inspect out/run-")
            .skip(1)
            .map(|rest| rest.split(".bin").next().unwrap())
            .collect();
        for panic in &result.worker_panics {
            assert!(rendered.contains(panic));
        }
        if let Some((_, error)) = &result.run_failed_first {
            assert!(rendered.contains(error));
        }

        let run_ids = result.violation_run_ids();
        if run_ids.is_empty() {
            assert!(commands.is_empty());
            assert!(rendered.contains("no violations"));
            return;
        }
        let [command] = commands[..] else {
            panic!("expected one command in {rendered:?}");
        };
        let run_id: u64 = command.parse().unwrap();
        assert!(run_ids.contains(&RunId(run_id)));
        assert_eq!(
            rendered.contains("replacing the run ID"),
            run_ids.len() > 1
        );
        assert!(rendered.contains(&format!(
            "violations in {} of {}",
            run_ids.len(),
            result.runs_count
        )));
    }
}
