# Bombadil

Property-based testing for web UIs, autonomously exploring and validating
correctness properties, *finding harder bugs earlier*.

Runs in your local developer environment, in CI, and inside Antithesis.

*NOTE: Bombadil is new and experimental. Stuff is going to change in the early
days, and generally stuff will be missing. Even so, we hope you'll try it out!*

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
