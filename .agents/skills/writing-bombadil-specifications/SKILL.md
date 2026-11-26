---
name: writing-bombadil-specifications
description: Use when writing, changing, or reviewing a Bombadil specification. Covers properties, extractors, action generators, and a review checklist.
metadata:
  bombadil-stub: "1"
---

# Writing Bombadil specifications

The guide ships inside Bombadil, so it matches the installed version. Print it
and follow it:

```sh
bombadil skill show writing-specifications
```

Use the project's installed executable. For an npm development dependency, run
this from the project root instead:

```sh
./node_modules/.bin/bombadil skill show writing-specifications
```

If the guide command is unavailable, use `bombadil --version`, `bombadil --help`,
and subcommand help. Check installed TypeScript definitions before using an API.

To exercise the specification, run `bombadil skill show fuzzing`.
To investigate a violation, run `bombadil skill show investigating-failures`.
