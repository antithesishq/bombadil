import sys
from typing import NoReturn

from colors import BOLD, CYAN, RESET, bold, cyan, dim, green, red, yellow


def header(msg: str) -> None:
    print(f"\n{BOLD}{CYAN}{'─' * 60}{RESET}")
    print(f"{BOLD}{CYAN}  {msg}{RESET}")
    print(f"{BOLD}{CYAN}{'─' * 60}{RESET}")


def step(n: int, total: int, msg: str) -> None:
    print(f"\n{bold(f'[{n}/{total}] {msg}')}")


def info(msg: str) -> None:
    print(f"  {cyan('→')} {msg}")


def ok(msg: str) -> None:
    print(f"  {green('✓')} {msg}")


def warn(msg: str) -> None:
    print(f"  {yellow('!')} {msg}")


def fail(msg: str) -> NoReturn:
    print(f"\n{red('✗ Error:')} {msg}", file=sys.stderr)
    sys.exit(1)


def prompt(msg: str, default: str | None = None) -> str:
    hint = f" [{default}]" if default else ""
    try:
        val = input(f"\n  {BOLD}{msg}{hint}: {RESET}").strip()
    except (KeyboardInterrupt, EOFError):
        print()
        sys.exit(0)
    return val or default or ""


def confirm(msg: str, default: bool = True) -> bool:
    hint = "Y/n" if default else "y/N"
    try:
        val = input(f"\n  {BOLD}{msg} ({hint}): {RESET}").strip().lower()
    except (KeyboardInterrupt, EOFError):
        print()
        sys.exit(0)
    if not val:
        return default
    return val in ("y", "yes")


def pause(msg: str = "Press Enter to continue…") -> None:
    try:
        _ = input(f"\n  {dim(msg)}")
    except (KeyboardInterrupt, EOFError):
        print()
        sys.exit(0)
