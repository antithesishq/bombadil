# Bombadil

Property-based testing for web UIs, autonomously exploring and validating
correctness properties, *finding harder bugs earlier*.

Runs in your local developer environment, in CI, and inside Antithesis.

> [!NOTE]
> Bombadil is new and experimental. Stuff is going to change in the early days. Even so, we hope you'll try it out!

## How it works 

As a user, you:

* **Write a specification:**

    A specification is a TypeScript module that exports *properties*.

    Properties are linear temporal logic formulas, describing what the system
    under test should and shouldn't do. The `"@antithesishq/bombadil/defaults"` module
    provides a set of reasonable properties for web applications. You may also
    specify your own domain-specific requirements.

* **Run tests:**

    When you have a specification, you run tests against a URL using that
    specification. This can be done locally, or in something like GitHub Actions.

This is unlike Selenium, Cypress, or Playwright, where you write fixed test
cases. Instead, you define actions and properties, and Bombadil explores and
tests your web application for you. This is *property-based testing* or
*fuzzing* for web applications.

## Examples

<details>
<summary>Starter (only using default properties)</summary>

This specification doesn't specify any custom properties at all, it just
reexports the default ones provided by Bombadil:

```typescript
export * from "@antithesishq/bombadil/defaults";
```

</details>

<details>
<summary>Invariant</summary>

An *invariant* is a very common type of property; something that should always
be true. Here's one that checks that there's always an `<h1>` element with some
text in it:


```typescript
import { always, extract } from "@antithesishq/bombadil";

const title = extract((state) => state.document.querySelector("h1")?.textContent ?? "");

export const has_title = always(() => title.current.trim() !== "");
```

</details>

<details>
<summary>Guarantee</summary>

A *guarantee* property is where something _good_ should happen within some
bounded amount of time. Here's one that checks that, when something is loading,
it eventually finishes loading and you see a result:


```typescript
import { now, eventually, extract } from "@antithesishq/bombadil";

const is_loading = extract((state) => !!state.document.querySelector("progress"));

const result = extract((state) =>
  state.document.querySelector(".result")?.textContent ?? null
);

export const finishes_loading = 
    now(() => is_loading.current)
        .implies(
            eventually(() => 
            !is_loading.current && result.current !== null
            ).within(5, "seconds")
        );
```

</details>

## Usage

Start a test:

```bash
$ bombadil test https://example.com
```

Or headless (useful in CI):

```bash
$ bombadil test https://example.com --headless
```

Check custom properties defined in a specification file:

```bash
$ bombadil test https://example.com my-spec.ts
```

These will log any property violations they find. If you want to immediately
exit, for instance when running in CI, run with `--exit-on-violation`:

```bash
$ bombadil test --exit-on-violation https://example.com my-spec.ts
```

You can also store the trace (a JSONL log file) by providing `--output-path`:

```bash
$ bombadil test --exit-on-violation --output-path=/tmp/my-test https://example.com my-spec.ts
$ head -n1 /tmp/my-test/trace.jsonl | jq .
{
  "url": "https://example.com",
  "hash_previous": null,
  "hash_current": 15313187356000757162,
  "action": null,
  "screenshot": "/tmp/my-test/screenshots/1770487569266229.webp",
  "violations": [],
  ...
}
```

> [!NOTE]
> The format of JSONL traces is currently under development and might change.

## Install

So far there's not a lot options for installing Bombadil other than using Nix.
That's going to change though! We want to supply:

* statically linked executables, which you can just download 
* Docker images
* a GitHub Action, ready to be used in your CI configuration

But for now, your best bet is either running it through Nix, like:

```
nix run github:antithesishq/bombadil
```

Or setting up the [developer environment](docs/contributing.md) and compiling
it with Cargo:

```
nix develop
cargo build --release
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
