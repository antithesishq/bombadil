---
name: investigating-bombadil-failures
description: Inspects Bombadil traces, reproduces property violations, and shrinks failing action sequences. Use when debugging a Bombadil failure or verifying a fix against a recorded run.
metadata:
  bombadil-stub: "1"
---

# Investigating Bombadil failures

Run `bombadil skill show investigating-failures` and follow its version-matched guide.

Use the project's installed executable. For an npm development dependency,
run `./node_modules/.bin/bombadil skill show investigating-failures` from the project root.
If the guide command is unavailable, use `bombadil --version`, `bombadil --help`,
and subcommand help. Do not assume newer options exist in older versions.

To search for more bugs after a fix, run `bombadil skill show finding-bugs`.
