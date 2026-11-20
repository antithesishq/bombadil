---
name: fuzzing-with-bombadil
description: Use when asked to run Bombadil, fuzz a web or terminal UI, or search for unexpected failures. Runs bounded fuzzing campaigns and keeps failing traces.
metadata:
  bombadil-stub: "1"
  author: Antithesis
  tags: "fuzzing, ui-testing, bombadil"
---

# Fuzzing with Bombadil

The guide ships inside Bombadil, so it matches the installed version. Print it
and follow it:

```sh
bombadil skill show fuzzing
```

Use the project's installed executable. For an npm development dependency, run
this from the project root instead:

```sh
./node_modules/.bin/bombadil skill show fuzzing
```

If the guide command is unavailable, use `bombadil --version`, `bombadil --help`,
and subcommand help. Do not assume newer options exist in older versions.

To investigate a failure, run `bombadil skill show investigating-failures`.
