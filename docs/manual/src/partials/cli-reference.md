## Command-line interface

The primary way of using Bombadil, at least locally, is through the
command-line interface (CLI). This section describes in detail what commands
are available, and what options, flags, and arguments you can give them.

The general exit code semantics of the CLI is:

| Code | Meaning |
|-----:|---------|
| 0 | Test or fuzzing campaign completed normally (including time limit) |
| 1 | Other error, including invalid command-line usage |
| 2 | Property violation(s) detected |

Property violations take precedence, so failed fuzz runs only cause the fuzzing
campaign to exit with `1` if there are no violations found.
