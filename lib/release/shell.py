import subprocess
from pathlib import Path

from ui import fail

ROOT = Path.cwd()


def run(cmd: str | list[str], *, cwd: Path | None = None, check: bool = True) -> None:
    """Run a command for its side effects; abort on failure when check=True."""
    result = subprocess.run(
        cmd,
        shell=isinstance(cmd, str),
        cwd=cwd or ROOT,
        check=False,
    )
    if check and result.returncode != 0:
        fail(f"Command failed: {cmd!r}")


def run_result(
    cmd: str | list[str], *, cwd: Path | None = None
) -> subprocess.CompletedProcess[str]:
    """Run a command and return the full CompletedProcess (never raises)."""
    return subprocess.run(
        cmd,
        shell=isinstance(cmd, str),
        capture_output=True,
        text=True,
        cwd=cwd or ROOT,
        check=False,
    )


def capture(
    cmd: str | list[str], *, cwd: Path | None = None, check: bool = True
) -> str:
    """Run a command and return stripped stdout; abort on failure when check=True."""
    result = run_result(cmd, cwd=cwd)
    if check and result.returncode != 0:
        fail(f"Command failed: {cmd!r}\n{result.stderr.strip()}")
    return result.stdout.strip()
