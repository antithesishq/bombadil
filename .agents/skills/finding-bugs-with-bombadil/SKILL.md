---
name: finding-bugs-with-bombadil
description: Finds bugs in web and terminal UIs through bounded Bombadil fuzzing campaigns. Use when asked to run Bombadil, fuzz a UI, or search for unexpected failures.
metadata:
  bombadil-stub: "1"
---

# Finding bugs with Bombadil

Run `bombadil skill show finding-bugs` and follow its version-matched guide.
Prefer `fuzz` for finding bugs; use `test` for quick checks and reproduction.

Use the project's installed executable. For an npm development dependency,
run `./node_modules/.bin/bombadil skill show finding-bugs` from the project root.
If the guide command is unavailable, use `bombadil --version`, `bombadil --help`,
and subcommand help. Do not assume newer options exist in older versions.

To write properties, run `bombadil skill show writing-specifications`.
To investigate a failure, run `bombadil skill show investigating-failures`.
