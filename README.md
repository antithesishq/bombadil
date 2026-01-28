# Bombadil

Property-based testing for web UIs, autonomously exploring and validating
correctness properties, *finding harder bugs earlier*.

Runs in your local developer environment, in CI, and inside Antithesis.

*NOTE: Bombadil is new and experimental. Stuff is going to change in the early
days, and generally stuff will be missing. Even so, we hope you'll try it out!*

## How it works (or, will work!)

As a user, you:

* write a specification:

    *NOTE: this is not ready yet!*

    A specification is a TypeScript module that exports *actions* and *properties*.

    Actions are what drives a test forward. You reexport the actions from the
    `bombadil` framework you want and, optionally, you define and export custom
    actions (e.g. "triple-click this div"). The bulk of actions should be
    provided by Bombadil itself.

    Properties are linear temporal logic formulas, describing what the system under
    test should and shouldn't do. Like the actions, the `bombadil` framework provides
    a set of reasonable properties for web applications. You may also specify your
    own domain-specific requirements.

* run tests:

    When you have a specification, you run tests against a URL using that
    specification. This can be done locally, or in something like GitHub Actions.

This is not like Selenium, Cypress, or Playwright, where you write fixed test
cases. Instead, you define actions and properties, and Bombadil explores and
tests your web application for you. This is *property-based testing* or
*fuzzing* for web applications.

Again, the description above is aspirational. Right now there are only a few
hardcoded properties in lieu of a specification language and related machinery.
We're building in the open, so stay tuned!

## Usage

Start a test:

```bash
bombadil test https://example.com
```

Or headless (useful in CI):

```bash
bombadil test https://example.com --headless
```

These will log any property violations they find. If you want to immediately
exit, for instance when running in CI, run with `--exit-on-violation`:

```bash
bombadil test --exit-on-violation https://example.com
```

## More Resources

* [Contributing](docs/contributing.md): if you want to hack on it
* [Quickstrom](https://quickstrom.io/): a predecessor to Bombadil

<hr>

<img alt="Tom Bombadil" src="docs/tom.png" width=360 />

> Old Tom Bombadil is a merry fellow,<br>
> Bright blue his jacket is, and his boots are yellow.<br>
> Bugs have never fooled him yet, for Tom, he is the Master:<br>
> His specs are stronger specs, and his fuzzer is faster.

Built by [Antithesis](https://antithesis.com).
