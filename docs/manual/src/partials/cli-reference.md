## Command-line interface

The primary way of using Bombadil, at least locally, is through the
command-line interface (CLI). This section describes in detail what commands
are available, and what options, flags, and arguments you can give them.

The general exit code semantics of the CLI is:

| Code | Meaning |
|-----:|---------|
| 0 | Test completed normally (including time limit) |
| 1 | Other error |
| 2 | Property violation(s) detected |
