# Getting started

Bombadil runs on your development machine if you're on macOS or Linux. Use it
to validate changes to [TypeScript specifications](#properties), and to run
short tests while working on your system. Configure something like GitHub
Actions or a cron job to continously run longer tests on your main branch or in
a deployed staging environment. When you're ready to put your system under
extreme conditions, and test your full stack deterministically, run it inside
[Antithesis](https://antithesis.com).

## Installation

The most straightforward way for you to get started is downloading the
executable for your platform:

<div class="accordion">
<details name="install">
<summary>npm</summary>

Install as a development dependency in your project:

```bash
npm install --save-dev @antithesishq/bombadil
```

Add a script to your `package.json` to run Bombadil:

::: browser
```json
{
  "scripts": {
    "test": "bombadil browser test --time-limit=1m https://your-app.example.com"
  }
}
```
:::

::: terminal
```json
{
  "scripts": {
    "test": "bombadil terminal test --time-limit=1m your-cli --arg value"
  }
}
```
:::

Then run it with `npm test`. This also provides TypeScript type definitions for
writing specifications.

</details>
<details name="install">
<summary>macOS</summary>

Download the `bombadil` binary using `curl` (or `wget`) and make it executable:

```bash
curl -L -o bombadil https://github.com/antithesishq/bombadil/releases/download/v%version%/bombadil-aarch64-darwin
chmod +x bombadil
```

Put the binary somewhere on your `PATH`, like in `~/.local/bin` if that is
configured.

```bash
mv ./bombadil ~/.local/bin/bombadil
```

You should now be able to run it:

```bash
bombadil --version
```

::: {.callout .callout-warning}
Do not download the executable with your web browser. It will be blocked by GateKeeper.
:::

</details>
<details name="install">
<summary>Linux</summary>

Download the `bombadil` binary and make it executable:

```bash
curl -L -o bombadil https://github.com/antithesishq/bombadil/releases/download/v%version%/bombadil-x86_64-linux
chmod +x bombadil
```

Put the binary somewhere on your `PATH`, like in `~/.local/bin` if that is
configured.

```bash
mv ./bombadil ~/.local/bin/bombadil
```

You should now be able to run it:

```bash
bombadil --version
```

</details>
<details name="install">
<summary>Nix (flake)</summary>

```bash
nix run github:antithesishq/bombadil
```

</details>

<details name="install">
<summary>GitHub Actions</summary>

::: browser
```yaml
- uses: antithesishq/bombadil-action@v2
  with:
    driver: browser
    origin: https://your-app.example.com
    specification: ./bombadil/specification.ts
    time-limit: 5m
    exit-on-violation: true
    output-path: bombadil-output
```

See [its README](https://github.com/antithesishq/bombadil-action) for detailed instructions.
:::

::: terminal
```yaml
- uses: antithesishq/bombadil-action@v2
  with:
    driver: terminal
    command: your-cli --arg value
    specification: ./bombadil/specification.ts
    time-limit: 5m
    exit-on-violation: true
    output-path: bombadil-output
```
:::

</details>
</div>

::: {.callout .callout-note}
Docker images are not yet available, but coming soon.
:::

If you want to compile from source, see [Contributing](https://github.com/antithesishq/bombadil/tree/main/docs/development/contributing.md).

## TypeScript support

When writing specifications in TypeScript, you'll want the types available.
If you installed Bombadil via npm, you already have them — skip ahead to
[Your first test](#your-first-test).

Otherwise, install the package with your package manager of choice:


<div class="accordion">
<details name="typescript">
<summary>npm</summary>
```bash
npm install --save-dev @antithesishq/bombadil
```
</details>
<details name="typescript">
<summary>Yarn</summary>
```bash
yarn add --dev @antithesishq/bombadil
```
</details>
<details name="typescript">
<summary>Bun</summary>
```bash
bun add --development @antithesishq/bombadil
```
</details>
</div>

Or use the files provided in [the 
release package](https://github.com/antithesishq/bombadil/releases/v%version%).

## Your first test

::: browser
With the CLI installed, let's run a browser test just to see that things are
working:

```bash
bombadil browser test https://en.wikipedia.org --output-path my-test
```

This will run until you shut it down using <kbd>Ctrl</kbd>+<kbd>C</kbd>. Any
property violations will be logged as errors, and with the `--output-path`
option you will get results to inspect afterwards.

Launch the *Bombadil Inspect* tool to see what happened in the test you
just ran:

```bash
bombadil browser inspect my-test
```

This will open a web application in your browser, which has some features to highlight:

* This interface is focused on *state transitions*, i.e. the state before and
  after each action.
* On the left is the actions list, which you can use to navigate the state
  transitions by clicking the actions. 
* In the bottom you'll see the timeline, which you can scrub (click and drag)
  with your mouse. The timeline also shows the currently selected state
  transition.
* If there were any violations found in the test, they'll be shown as
  exclamation mark icons in the timeline.

No violations? That's fine, Wikipedia is pretty solid! This confirms that
Bombadil runs and produces results.
:::

::: terminal
With the CLI installed, let's run a very simple terminal test just to see that
things are working:

```bash
bombadil terminal test --output-path my-test head -n100
```

It's likely it'll type a <kbd>Ctrl</kbd>+<kbd>D</kbd> and have `head` exit
before a hundred lines have been printed. As we didn't use a custom
specification with the `--specification` option, Bombadil used the default
specification for terminal testing, which enters various Unicode input and some
escape sequences. It also checks a few common properties, like that the program
exits with a zero exit code and that it doesn't print any byte sequences the
terminal can't render.

::: {.callout .callout-note}
There's not yet an equivalent to `bombadil browser inspect` for terminal tests,
so we can't easily inspect the results at this point.
:::
:::

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

Reproductions are not guaranteed to succeed; if they diverge, Bombadil fails
with an error. For reproductions to succeed, it's important to use the same
options as in the original test. After a test run, Bombadil prints both the
`inspect` and `--reproduce` commands you can use.
