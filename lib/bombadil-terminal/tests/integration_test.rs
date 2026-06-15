use std::io::Write;
use std::sync::Once;
use std::sync::mpsc;
use std::time::Duration;

use anyhow::Result;
use bombadil::runner::{ControlFlow, PropertiesState, RunStrategy, Runner};
use bombadil::specification::domain::Snapshot;
use bombadil::specification::verifier::Specification;
use bombadil::tree::Tree;
use bombadil_schema::TerminalSize;
use bombadil_terminal::driver::{TerminalAction, TerminalDriver};
use bombadil_terminal::state::TerminalState;
use rand::rngs::ThreadRng;
use tempfile::NamedTempFile;

const MAX_SCROLLBACK: usize = 1_000;
const TEST_TIMEOUT: Duration = Duration::from_secs(10);

static INIT: Once = Once::new();

fn setup() {
    INIT.call_once(|| {
        let env = env_logger::Env::default().default_filter_or("debug");
        env_logger::Builder::from_env(env)
            .format_timestamp_millis()
            .is_test(true)
            .try_init()
            .ok();
    });
}

struct TerminalIntegrationTest {
    seed: u64,
    program: String,
    args: Vec<String>,
    size: TerminalSize,
    max_scrollback: usize,
    specification_source: String,
}

impl TerminalIntegrationTest {
    fn new(program: &str, args: &[&str]) -> Self {
        Self {
            seed: rand::random(),
            program: program.to_string(),
            args: args.iter().map(|arg| arg.to_string()).collect(),
            size: TerminalSize {
                columns: 80,
                rows: 24,
            },
            max_scrollback: MAX_SCROLLBACK,
            specification_source: String::new(),
        }
    }

    fn specification(mut self, source: &str) -> Self {
        self.specification_source = source.to_string();
        self
    }

    /// Runs the specification against the program and asserts zero property
    /// violations.
    fn run(self) {
        setup();
        let TerminalIntegrationTest {
            seed,
            program,
            args,
            size,
            max_scrollback,
            specification_source,
        } = self;

        let mut specification_file = NamedTempFile::with_suffix(".ts").unwrap();
        specification_file
            .write_all(specification_source.as_bytes())
            .unwrap();
        let specification = Specification {
            module_specifier: specification_file.path().display().to_string(),
        };

        let (sender, receiver) = mpsc::channel();
        let _ = std::thread::spawn(move || {
            // Keep the spec file alive for the whole run.
            let _specification_file = specification_file;
            let result = (|| -> Result<u64> {
                let (driver, verifier) = TerminalDriver::launch(
                    specification,
                    size,
                    max_scrollback,
                    Duration::from_millis(10),
                    &program,
                    &args,
                )?;
                let runner = Runner::new(driver, verifier);
                let mut strategy = IntegrationTestStrategy {
                    rng: rand::rng(),
                    violations_count: 0,
                };
                runner.run(&mut strategy)?;
                Ok(strategy.violations_count)
            })();
            let _ = sender.send(result);
        });

        let violations_count = receiver
            .recv_timeout(TEST_TIMEOUT)
            .unwrap_or_else(|_| {
                panic!("terminal integration test hung past {TEST_TIMEOUT:?}\n\ntry to reproduce with .seed({seed})")
            })
            .expect("terminal runner failed");

        assert_eq!(
            violations_count, 0,
            "expected zero violations, got {violations_count}\n\ntry to reproduce with .seed({seed})",
        );
    }
}

#[test]
fn test_eventually_ready() {
    TerminalIntegrationTest::new("sh", &["-c", "printf 'ready\\n'"])
        .specification(
            r#"
import { eventually } from "@antithesishq/bombadil";
import { actions, extract } from "@antithesishq/bombadil/terminal";

// Exercises the fast `rowText` path.
const screen = extract((state) => {
    const lines = [];
    for (let index = 0; index < state.grid.size.rows; index++) {
        lines.push(state.grid.rowText(index));
    }
    return lines.join("\n");
});

// Exercises the cell-level `row` path so both grid APIs stay covered.
const screenFromCells = extract((state) => {
    const lines = [];
    for (let index = 0; index < state.grid.size.rows; index++) {
        lines.push(state.grid.row(index).map((cell) => cell.contents).join(""));
    }
    return lines.join("\n");
});

export const eventuallyReady = eventually(
    () => screen.current.includes("ready"),
);

export const eventuallyReadyFromCells = eventually(
    () => screenFromCells.current.includes("ready"),
);

export const noop = actions(() => [{ TypeText: { text: "" } }]);
"#,
        )
        .run();
}

#[test]
fn test_yes() {
    TerminalIntegrationTest::new("yes", &[])
        .specification(
            r#"
import { eventually } from "@antithesishq/bombadil";
import { actions, extract } from "@antithesishq/bombadil/terminal";

const lines = extract((state) => {
    const lines = [];
    for (let index = 0; index < state.grid.size.rows; index++) {
        lines.push(state.grid.rowText(index));
    }
    return lines;
});

export const eventuallyReady = eventually(
    () => lines.current.every(line => line.includes("y")),
);

export const noop = actions(() => [{ TypeText: { text: "" } }]);
"#,
        )
        .run();
}

#[test]
fn test_colored_segments() {
    TerminalIntegrationTest::new(
        "sh",
        // Color-styled strings, each emitted separately with a short pause in
        // between. Each string embeds a non-ASCII grapheme so the test covers
        // multi-byte/multi-codepoint cell handling.
        &[
            "-c",
            "printf '\\033[31mred hot ❤️ chili peppers\\033[0m'; sleep 0.005; \
             printf '\\033[32mgrant 緑 green\\033[0m'; sleep 0.005; \
             printf '\\033[33myellow submarine 👨‍👩‍👧‍👦\\033[0m'; sleep 0.005; \
             printf '\\033[34mkind of blue 😎\\033[0m\\n'",
        ],
    )
    .specification(
        r#"
import { eventually } from "@antithesishq/bombadil";
import { actions, extract } from "@antithesishq/bombadil/terminal";

function colorKey(color) {
    if (color === "None") return "None";
    if ("Palette" in color) return "Palette:" + color.Palette;
    return `RGB:${color.RGB.r},${color.RGB.g},${color.RGB.b}`;
}

const segments = extract((state) => {
    const result = [];
    let current = null;
    for (let row = 0; row < state.grid.size.rows; row++) {
        for (const cell of state.grid.row(row)) {
            const key = colorKey(cell.style.foregroundColor);
            if (current !== null && current.key === key) {
                current.text += cell.contents;
            } else {
                current = {
                    key,
                    color: cell.style.foregroundColor,
                    text: cell.contents,
                };
                result.push(current);
            }
        }
    }
    return result
        .filter((segment) => segment.text.trim() !== "")
        .map((segment) => ({ color: segment.color, text: segment.text }));
});

function isPalette(color, index) {
    return (
        typeof color === "object" &&
        "Palette" in color &&
        color.Palette === index
    );
}

export const eventuallyColoredSegments = eventually(() => {
    const found = segments.current;
    return (
        found.length === 4 &&
        found[0].text === "red hot ❤️ chili peppers" &&
        isPalette(found[0].color, 1) &&
        found[1].text === "grant 緑 green" &&
        isPalette(found[1].color, 2) &&
        found[2].text === "yellow submarine 👨‍👩‍👧‍👦" &&
        isPalette(found[2].color, 3) &&
        found[3].text === "kind of blue 😎" &&
        isPalette(found[3].color, 4)
    );
});

export const noop = actions(() => [{ TypeText: { text: "" } }]);
"#,
    )
    .run();
}

#[test]
fn test_cursor_state() {
    TerminalIntegrationTest::new(
        "sh",
        &["-c", "printf 'abc\\033[2;5H\\033[3 q\\033]12;#010203\\007'"],
    )
    .specification(
        r#"
import { eventually } from "@antithesishq/bombadil";
import { actions, extract } from "@antithesishq/bombadil/terminal";

const cursor = extract((state) => state.cursor);

function isRGB(color, r, g, b) {
    return (
        typeof color === "object" &&
        "RGB" in color &&
        color.RGB.r === r &&
        color.RGB.g === g &&
        color.RGB.b === b
    );
}

export const eventuallyCursorState = eventually(() => {
    const current = cursor.current;
    return (
        current.position.row === 1 &&
        current.position.column === 4 &&
        current.visible &&
        current.visualStyle === "Underline" &&
        isRGB(current.color, 1, 2, 3)
    );
});

export const noop = actions(() => [{ TypeText: { text: "" } }]);
"#,
    )
    .run();
}

#[test]
fn test_wide_char_wraps_at_right_margin() {
    // Emoji doesn't fit first line as it's wide, so it wraps and ends up on second line.
    TerminalIntegrationTest::new(
        "sh",
        &[
            "-c",
            "i=0; while [ $i -lt 79 ]; do printf x; i=$((i+1)); done; \
             printf '😎\\n'",
        ],
    )
    .specification(
        r#"
import { always, eventually } from "@antithesishq/bombadil";
import { actions, extract } from "@antithesishq/bombadil/terminal";

const screen = extract((state) => ({
    row0: state.grid.rowText(0),
    row1: state.grid.rowText(1),
}));

export const wideCharNeverOnFirstRow = always(() => {
    const { row0 } = screen.current;
    return !row0.includes("😎") && row0.length === 80;
});

export const eventuallyWrapped = eventually(() => {
    const { row0, row1 } = screen.current;
    return row0 === "x".repeat(79) + " " && row1.startsWith("😎");
});

export const noop = actions(() => [{ TypeText: { text: "" } }]);
"#,
    )
    .run();
}

struct IntegrationTestStrategy {
    rng: ThreadRng,
    violations_count: u64,
}

impl RunStrategy<TerminalDriver> for IntegrationTestStrategy {
    type StopValue = ();

    fn on_new_state(
        &mut self,
        state: &TerminalState,
        tree: Tree<TerminalAction>,
        _last_action: Option<&TerminalAction>,
        _snapshots: &[Snapshot],
        properties: PropertiesState<'_>,
    ) -> Result<ControlFlow<(), TerminalAction>> {
        self.violations_count += properties.violations.len() as u64;
        if properties.all_definite {
            return Ok(ControlFlow::Stop(()));
        }
        if state.exit_status.is_some() {
            return Ok(ControlFlow::Stop(()));
        }
        Ok(ControlFlow::Continue(tree.pick(&mut self.rng)?.clone()))
    }

    fn on_interrupted(&mut self) -> Result<()> {
        Ok(())
    }
}
