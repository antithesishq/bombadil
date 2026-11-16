---
name: investigating-bombadil-failures
description: Use when debugging a Bombadil failure or verifying a fix against a recorded run. Inspects traces, reproduces violations, and shrinks failing runs.
metadata:
  bombadil-stub: "1"
---

# Investigating Bombadil failures

The guide ships inside Bombadil, so it matches the installed version. Print it
and follow it:

```sh
bombadil skill show investigating-failures
```

Use the project's installed executable. For an npm development dependency, run
this from the project root instead:

```sh
./node_modules/.bin/bombadil skill show investigating-failures
```

If the guide command is unavailable, use `bombadil --version`, `bombadil --help`,
and subcommand help. Do not assume newer options exist in older versions.

To search for more bugs after a fix, run `bombadil skill show fuzzing`.
