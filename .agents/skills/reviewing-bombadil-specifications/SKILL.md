---
name: reviewing-bombadil-specifications
description: Reviews Bombadil specifications for properties that pass without checking anything, such as vacuous implications, unreachable triggers, narrowed actions, and validity-only oracles. Use after writing a Bombadil specification or when asked to review one.
metadata:
  bombadil-stub: "1"
---

# Reviewing Bombadil specifications

Run `bombadil skill show reviewing-specifications` and check the specification against its version-matched checklist.

Use the project's installed executable. For an npm development dependency,
run `./node_modules/.bin/bombadil skill show reviewing-specifications` from the project root.
If the guide command is unavailable, use `bombadil --version`, `bombadil --help`,
and subcommand help. Check installed TypeScript definitions before using an API.

To fix a property, run `bombadil skill show writing-specifications`.
To exercise the result, run `bombadil skill show finding-bugs`.
