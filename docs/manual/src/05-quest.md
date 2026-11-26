::: browser

# Guiding tests with Quest

::: {.callout .callout-warning}
This is experimental.
:::

By default, Bombadil picks actions at random. Quest is a service that looks at
the test runs you have already made and works out which actions are worth
trying more often. You tell it what you are after, such as a bug you want to
see again or code that rarely runs, and Bombadil's next runs head in that
direction.

It takes two steps, which you then repeat: upload some ordinary runs, then
run again with a goal.

## Uploading runs

Run your tests as usual, and add the address of the Quest service, the name
of your app in Quest, written as `organisation/app`, and `--quest-upload`:

```bash
bombadil browser fuzz https://example.com spec.ts --quest-url https://quest.example.com --quest-app acme/shop --quest-upload
```

The app is created in Quest the first time you use its name. When the tests
are done, Bombadil uploads the runs and waits while Quest studies them. It
then prints a line after the test results:

```
Quest (acme/shop): uploaded 12 runs and trained model 1
```

If Quest has something to say about what it learned, for example that it
could not use the code coverage, Bombadil prints it as a note under that
line. Quest needs at least two runs before it can do anything with them.

The same options work with `bombadil browser test` and `test-external`, which
upload their single run. With `--quest-app`, Bombadil records which functions
of your scripts ran, as `--coverage-components` does, because Quest needs
that. To record only some scripts, pass `--coverage-components` yourself with
their prefixes.

If the Quest service asks for a token, pass it with `--quest-auth-token`, or
put it in the `BOMBADIL_QUEST_AUTH_TOKEN` environment variable.

## Running with a goal

Add `--goal` to say what you are after:

```bash
bombadil browser fuzz https://example.com spec.ts --quest-url https://quest.example.com --quest-app acme/shop --quest-upload --goal 'bug(noUncaughtExceptions)'
```

Bombadil asks Quest how to reach the goal before it starts, and uploads these
runs too when it is done, so Quest knows more the next time.

Keep uploading ordinary runs as well. Quest compares the runs that had a goal
with the runs that did not, so it needs new ordinary runs now and then. If
you only ever upload runs with a goal, it will eventually stop learning from
them and tell you to upload ordinary runs.

## Using a goal without uploading

To use what Quest has learned without adding to it, leave out
`--quest-upload`:

```bash
bombadil browser fuzz https://example.com spec.ts --quest-url https://quest.example.com --quest-app acme/shop --goal 'bug(noUncaughtExceptions)'
```

This suits tests of a pull request. The code under test there may never be
merged, and its runs should not shape what Quest learns about your app.
Upload from the runs of your main branch instead.

## Goals

A goal is written the way you would write it in JavaScript, using the names
from your specification. Bombadil passes it to Quest as you wrote it.

| Goal | Reward |
| --- | --- |
| `explore` | pages and states that few runs have been in |
| `frontier` | code that rarely runs, in functions that have only partly run |
| `'bug(noUncaughtExceptions)'` | the places where this property was violated before |
| `'state(cart.items > 0)'` | being in a state where the condition is true |
| `'transition(cart.items == 0, cart.items > 0)'` | the step from a state where the first condition is true to one where the second is |
| `'maximize(score)'` | being in a state where the number is high; `minimize` for low |
| `'increase(score)'` | an action that makes the number go up; `decrease` for down |
| `'code(checkout)'` | the function with this name |

Put single quotes around a goal that has parentheses, or your shell will try
to interpret them.

The names in the conditions and numbers are the extractors of your
specification. In the examples above, the specification would contain
`const cart = extract(...)` and `const score = extract(...)`. You can also use
`url`, the address of the page.

If two scripts have a function with the same name, say which script you mean:
`'code("/app.js#checkout")'`. Quest can only head for a function that ran in
at least one uploaded run. For code that has not run yet, use `frontier`.

A few more examples:

- `'state(cart.items > 0 && route.view == "checkout")'` rewards a state where
  both comparisons are true.
- `'transition(route.view == "cart", route.view == "checkout")'` rewards
  going from the cart to the checkout, and nothing for staying in either.
- `'increase(cart.items)'` rewards adding an item, where
  `'maximize(cart.items)'` rewards having a full cart.

Quest decides which goals, conditions and settings it accepts, and its
documentation lists them.

## When something goes wrong

If Quest does not accept a goal, or cannot help with it, Bombadil stops
before testing and prints Quest's explanation. The most common reason is that
you have not uploaded any runs yet.

If the upload fails after the tests, Bombadil prints the error. The exit code
is still the one the tests produced, so a failed upload does not make a
passing test fail.

:::
