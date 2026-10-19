# Workflows

This section describes how to run tests and debug violations with Bombadil.
Each workflow is a building block in a composable set of tools for finding,
understanding, and fixing bugs in user interfaces.

## Reproducing violations

If Bombadil finds a bug in a project you're working on, you might want to
reproduce that test case when working on a bug fix. That is, have Bombadil
perform the same sequence of actions to reach the same state.

Use the `--reproduce` option and point it to the output directory of the
original test run:

::: browser
```bash
bombadil browser test --reproduce=my-test http://example.com
```
:::

::: terminal
```bash
bombadil terminal test --reproduce=my-test your-cli
```
:::

Reproductions are not guaranteed to succeed. If they diverge, Bombadil fails
with an error. For reproductions to succeed, it's important to use the same
options as in the original test. After a test run, Bombadil prints both the
`inspect` and `--reproduce` commands you can use.

## Shrinking violations

When Bombadil finds a violation, the trace leading up to it is often long and
full of actions that have nothing to do with the bug. The `shrink` command
takes such a trace and searches for a shorter one that still violates the
same property.

::: {.callout .callout-warning}
Shrinking is experimental. The command-line options and output format may
change between releases.
:::

::: browser
```bash
bombadil browser shrink --headless --trace my-test https://example.com
```
:::

::: terminal
```bash
bombadil terminal shrink --trace my-test your-cli
```
:::

It's important that the shrink invocation uses the same options as the original
`test` or `fuzz` invocation, just as when [reproducing violations][Reproducing violations].

Shrinking begins with a calibration phase that tries to assess the flakiness of
reproducing the violation. Calibration can be controlled with
`--calibration-runs` and `--confirmation-runs-max` if needed. At the end of
shrinking, you get a new trace in the output directory, configured with
`--output-path` and `--output-path-overwrite`, that can be
[reproduced][Reproducing violations] or [inspected][Inspecting test results].

## Inspecting test results

::: browser

When you've run a test and found one or more violations, you likely
want to understand what happened. The [inspect][bombadil browser inspect] tool
shows a single trace using an interactive web view.

```bash
bombadil browser inspect my-test
```

This command automatically opens the web page in your browser. You will see the
actions listed on the left, and timeline in the bottom, and in the main pane
the transition caused by the selected action. A transition shows state before
and state after. Along with screenshots, you see violations and all captured
snapshot values per state.

To look at a trace in the terminal instead, `bombadil trace` lists its states
with their actions, URLs and violations. With `--screenshots`, it also writes
each state's screenshot to a directory, named by the state's index:

```bash
bombadil trace my-test --screenshots my-test-screenshots
```

Pick states with `--state` (`--state 40..50` shows states 40 to 49), or print
one JSON object per state with `--json` to process the trace with tools like
`jq`. Running `--screenshots` again leaves the screenshots already written as
they are.

:::

::: terminal

When you've run a test and found one or more violations, you likely want to
understand what happened. The `bombadil trace` command shows a trace in the
terminal, one state at a time:

```bash
bombadil trace my-test --screen
```

Each state lists the action that led to it, the time since the first state,
and the properties it violated. With `--screen`, you also see the terminal
screen as the properties saw it. Pick states with `--state` (`--state 40..50`
shows states 40 to 49), keep the screen's colors with `--color`, or print one
JSON object per state with `--json` (with `--screen`, including the screen) to
process the trace with tools like `jq`.

:::
