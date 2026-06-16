# Reference

## Command-line interface

<!-- **TODO:** generate this automatically but in structured HTML -->

### Exit codes

| Code | Meaning |
|-----:|---------|
| 0 | Test completed normally (including time limit) |
| 1 | Other error |
| 2 | Property violation(s) detected |

::: browser
### bombadil browser test

`bombadil` `test` [`[OPTIONS]`](#options-test) [`<ORIGIN>`](#arguments-test) [`[SPECIFICATION_FILE]`](#arguments-test)


::: {#arguments-test}
| Argument | Description |
|----------|-------------|
| `<ORIGIN>` | Starting URL of the test (also used as a boundary so that Bombadil doesn't navigate to other websites) |
| `[SPECIFICATION_FILE]` | A custom specification in TypeScript or JavaScript, using the `@antithesishq/bombadil` package on NPM |
:::

::: {#options-test}
| Option | Description | Default |
|--------|-------------|---------:|
| `--output-path <OUTPUT_PATH>` | Where to store output data (trace, screenshots, etc.) | |
| `--output-path-overwrite` | Overwrite any existing `trace.jsonl` at `--output-path`. Without this flag, Bombadil refuses to write when one already exists. | |
| `--exit-on-violation` | Whether to exit the test when first failing property is found (useful in development and CI) | |
| `--time-limit <TIME_LIMIT>` | Maximum time to run the test; reaching the limit is treated as normal completion. Accepts a number with a unit suffix: s (seconds), m (minutes), h (hours), or d (days). Examples: 30s, 5m, 2h, 1d | |
| `--width <WIDTH>` | Browser viewport width in pixels | 1024 |
| `--height <HEIGHT>` | Browser viewport height in pixels | 768 |
| `--device-scale-factor <DEVICE_SCALE_FACTOR>` | Scaling factor of the browser viewport, mostly useful on high-DPI monitors when in headed mode | 2 |
| `--instrument-javascript <INSTRUMENT_JAVASCRIPT>` | What types of JavaScript to instrument for coverage tracking. Comma-separated list of: "files", "inline" | files,inline |
| `--chrome-grant-permissions <CHROME_GRANT_PERMISSIONS>` | Comma-separated list of Chrome permissions to grant. Examples: local-network-access, geolocation, notifications. | local-network-access,local-network,loopback-network |
| `--header <KEY=VALUE>` | Extra HTTP header to send with all browser requests, in `KEY=VALUE format`. Can be specified multiple times. | |
| `--reproduce <TRACE_FILE>` | Reproduce a previous test run from a trace file, instead of random exploration. Mutually exclusive with `--time-limit` and `--exit-on-violation`. | |
| `--headless` | Whether the browser should run in a visible window or not | |
| `--no-sandbox` | Disable Chromium sandboxing | |
| `-h, --help` | Print help | |
:::

### bombadil browser test-external

`bombadil` `test-external` [`[OPTIONS]`](#options-test-external) [`<ORIGIN>`](#arguments-test-external) [`[SPECIFICATION_FILE]`](#arguments-test-external)

::: {#arguments-test-external}
| Argument | Description |
|----------|-------------|
| `<ORIGIN>` | Starting URL of the test (also used as a boundary so that Bombadil doesn't navigate to other websites) |
| `[SPECIFICATION_FILE]` | A custom specification in TypeScript or JavaScript, using the `@antithesishq/bombadil` package on NPM |
:::

::: {#options-test-external}
| Option | Description | Default |
|--------|-------------|---------:|
| `--output-path <OUTPUT_PATH>` | Where to store output data (trace, screenshots, etc.) | |
| `--output-path-overwrite` | Overwrite any existing `trace.jsonl` at `--output-path`. Without this flag, Bombadil refuses to write when one already exists. | |
| `--exit-on-violation` | Whether to exit the test when first failing property is found (useful in development and CI) | |
| `--time-limit <TIME_LIMIT>` | Maximum time to run the test; reaching the limit is treated as normal completion. Accepts a number with a unit suffix: s (seconds), m (minutes), h (hours), or d (days). Examples: 30s, 5m, 2h, 1d | |
| `--width <WIDTH>` | Browser viewport width in pixels | 1024 |
| `--height <HEIGHT>` | Browser viewport height in pixels | 768 |
| `--device-scale-factor <DEVICE_SCALE_FACTOR>` | Scaling factor of the browser viewport, mostly useful on high-DPI monitors when in headed mode | 2 |
| `--instrument-javascript <INSTRUMENT_JAVASCRIPT>` | What types of JavaScript to instrument for coverage tracking. Comma-separated list of: "files", "inline" | files,inline |
| `--chrome-grant-permissions <CHROME_GRANT_PERMISSIONS>` | Comma-separated list of Chrome permissions to grant. Examples: local-network-access, geolocation, notifications. | local-network-access,local-network,loopback-network |
| `--header <KEY=VALUE>` | Extra HTTP header to send with all browser requests, in `KEY=VALUE format`. Can be specified multiple times. | |
| `--reproduce <TRACE_FILE>` | Reproduce a previous test run from a trace file, instead of random exploration. Mutually exclusive with `--time-limit` and `--exit-on-violation`. | |
| `--remote-debugger <REMOTE_DEBUGGER>` | Address to the remote debugger's server, e.g. http://localhost:9222 | |
| `--create-target` | Whether Bombadil should create a new tab and navigate to the origin URL in it, as part of starting the test (this should probably be false if you test an Electron app) | |
| `-h, --help` | Print help | |
:::

### bombadil browser inspect

`bombadil` `inspect` [`[OPTIONS]`](#options-inspect) [`<TRACE_PATH>`](#arguments-inspect)

::: {#arguments-inspect}
| Argument | Description |
|----------|-------------|
| `<TRACE_PATH>` | Path to trace.jsonl file or directory containing it |
:::

::: {#options-inspect}
| Option | Description | Default |
|--------|-------------|---------:|
| `--port <PORT>` | Port to bind the inspect server to | 1073 |
| `--no-open` | Skip auto-opening browser | |
| `-h, --help` | Print help | |
:::
:::

::: terminal
### bombadil terminal test (EXPERIMENTAL!)

`bombadil` `terminal` `test` [`[OPTIONS]`](#options-terminal-test) [`[COMMAND]*`](#arguments-terminal-test)

::: {#arguments-terminal-test}
| Argument | Description |
|----------|-------------|
| `[COMMAND]*` | The command to run for each test case (i.e. program name and arguments, space-separated) |
:::

::: {#options-terminal-test}
| Option | Description | Default |
|--------|-------------|---------:|
| `--specification <SPECIFICATION_FILE>` | Path to a TypeScript specification file (uses the `@antithesishq/bombadil/terminal` API). Unless specified, Bombadil will use the default specification for terminal UIs. | |
| `--exit-on-violation` | Whether to exit the test when first failing property is found (useful in development and CI) | |
| `--time-limit <TIME_LIMIT>` | Maximum time to run the test; reaching the limit is treated as normal completion. Accepts a number with a unit suffix: s (seconds), m (minutes), h (hours), or d (days). Examples: 30s, 5m, 2h, 1d | |
| `--columns <COLUMNS>` | Terminal columns at startup | 100 |
| `--rows <ROWS>` | Terminal rows at startup | 40 |
| `--scrollback-lines-max <SCROLLBACK_LINES_MAX>` | Maximum line count to keep in scrollback buffer  | 100 |
| `--seed <SEED>` | Random generator seed | |
| `--render-append` | Whether to append render output (otherwise clear screen before every render) | |
| `--output-path <OUTPUT_PATH>` | Where to store output data (trace.jsonl). Defaults to a fresh temporary directory. | |
| `--output-path-overwrite` | Overwrite any existing `trace.jsonl` at `--output-path`. Without this flag, Bombadil refuses to write when one already exists. | |
| `--reproduce <TRACE_FILE>` | Reproduce a previous test run from a trace file (file path or directory containing `trace.jsonl`). Replays the recorded actions in order instead of generating new ones.| |
| `-h, --help` | Print help | |
:::
:::
