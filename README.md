# Bombadil

Property-based testing for web UIs, autonomously exploring and validating
correctness properties, *finding harder bugs earlier*.

Runs in your local developer environment, in CI, and inside Antithesis.

> [!NOTE]
> Bombadil is new and experimental. Stuff is going to change in the early days. Even so, we hope you'll try it out!

## How it works 

As a user, you:

* **Write a specification:**

    A specification is a TypeScript module that exports *properties* and *action generators*.

    Properties are linear temporal logic formulas, describing what the system
    under test should and shouldn't do. The `"@antithesishq/bombadil/defaults/properties"` 
    module provides a set of reasonable properties for web applications. You may also
    specify your own domain-specific requirements.

    Action generators produce actions in every state, which contribute to a set of actions
    that Bombadil picks the *next* action from. The `"@antithesishq/bombadil/defaults/actions"` 
    module provides a set of reasonable generators for most web applications. In addition to
    reexporting those, you can constructor your own actions. For instance, you might have a 
    clickable `<div>` that Bombadil doesn't know can be clicked.

* **Run tests:**

    When you have a specification, you run tests against a URL using that
    specification. This can be done locally, or in something like GitHub Actions.

This is unlike Selenium, Cypress, or Playwright, where you write fixed test
cases. Instead, you define actions and properties, and Bombadil explores and
tests your web application for you. This is *property-based testing* or
*fuzzing* for web applications.

## Examples

<details>
<summary>Starter (only using default properties and actions)</summary>

This specification doesn't specify any custom properties or actions at all, it
just reexports the default ones provided by Bombadil:

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
export { clicks } from "@antithesishq/bombadil/defaults/actions";

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
export { clicks, inputs } from "@antithesishq/bombadil/defaults/actions";

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

Check with a custom specification file:

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

The most straightforward way for you to get start is downloading [the latest
executable](https://github.com/antithesishq/bombadil/releases/latest) for your
platform:

```bash
$ wget https://github.com/antithesishq/bombadil/releases/latest/download/bombadil-x86_64-linux
$ chmod +x bombadil-x86_64-linux
$ ./bombadil-x86_64-linux --version
bombadil 0.2.0
```

If you're a Nix and flakes user, you can run it with:

```
$ nix run github:antithesishq/bombadil
```

Not yet available, but coming soon:

* Docker images
* a GitHub Action, ready to be used in your CI configuration

If you want to compile from source, see [Contributing](docs/contributing.md).

### TypeScript Support

When writing specifications in TypeScript, you'll want the types available.
Get them from [NPM](https://www.npmjs.com/package/@antithesishq/bombadil)
with your package manager of choice:

```bash
$ npm install @antithesishq/bombadil
```

Or use the files provided in the [the 
release package](https://github.com/antithesishq/bombadil/releases/latest).

## More Resources

* [Contributing](docs/development/contributing.md): if you want to hack on it
* [Quickstrom](https://quickstrom.io/): a predecessor to Bombadil

<hr>

<img alt="Tom Bombadil" src="docs/development/tom.png" width=360 />

> Old Tom Bombadil is a merry fellow,<br>
> Bright blue his jacket is, and his boots are yellow.<br>
> Bugs have never fooled him yet, for Tom, he is the Master:<br>
> His specs are stronger specs, and his fuzzer is faster.

Built by [Antithesis](https://antithesis.com).
