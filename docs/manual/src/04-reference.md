# Reference

## Command-line interface

<!-- **TODO:** generate this automatically but in structured HTML -->

### Exit codes

| Code | Meaning |
|-----:|---------|
| 0 | Test completed normally (including time limit) |
| 2 | Property violation detected (with `--exit-on-violation`) |

### bombadil test

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
| `--exit-on-violation` | Whether to exit the test when first failing property is found (useful in development and CI) | |
| `--time-limit <DURATION>` | Maximum time to run the test; reaching the limit is treated as normal completion. Accepts a number with a unit suffix: s (seconds), m (minutes), h (hours), or d (days). Examples: 30s, 5m, 2h, 1d | |
| `--width <WIDTH>` | Browser viewport width in pixels | 1024 |
| `--height <HEIGHT>` | Browser viewport height in pixels | 768 |
| `--device-scale-factor <DEVICE_SCALE_FACTOR>` | Scaling factor of the browser viewport, mostly useful on high-DPI monitors when in headed mode | 2 |
| `--instrument-javascript <INSTRUMENT_JAVASCRIPT>` | What types of JavaScript to instrument for coverage tracking. Comma-separated list of: "files", "inline" | files,inline |
| `--chrome-grant-permissions <CHROME_GRANT_PERMISSIONS>` | Comma-separated list of Chrome permissions to grant. Examples: local-network-access, geolocation, notifications. | local-network-access |
| `--headless` | Whether the browser should run in a visible window or not | |
| `--no-sandbox` | Disable Chromium sandboxing | |
| `-h, --help` | Print help | |
:::

### bombadil test-external

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
| `--exit-on-violation` | Whether to exit the test when first failing property is found (useful in development and CI) | |
| `--time-limit <DURATION>` | Maximum time to run the test; reaching the limit is treated as normal completion. Accepts a number with a unit suffix: s (seconds), m (minutes), h (hours), or d (days). Examples: 30s, 5m, 2h, 1d | |
| `--width <WIDTH>` | Browser viewport width in pixels | 1024 |
| `--height <HEIGHT>` | Browser viewport height in pixels | 768 |
| `--device-scale-factor <DEVICE_SCALE_FACTOR>` | Scaling factor of the browser viewport, mostly useful on high-DPI monitors when in headed mode | 2 |
| `--instrument-javascript <INSTRUMENT_JAVASCRIPT>` | What types of JavaScript to instrument for coverage tracking. Comma-separated list of: "files", "inline" | files,inline |
| `--chrome-grant-permissions <CHROME_GRANT_PERMISSIONS>` | Comma-separated list of Chrome permissions to grant. Examples: local-network-access, geolocation, notifications. | local-network-access |
| `--remote-debugger <REMOTE_DEBUGGER>` | Address to the remote debugger's server, e.g. http://localhost:9222 | |
| `--create-target` | Whether Bombadil should create a new tab and navigate to the origin URL in it, as part of starting the test (this should probably be false if you test an Electron app) | |
| `-h, --help` | Print help | |
:::

### bombadil inspect

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
