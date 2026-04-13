from pathlib import Path

from shell import capture, run_result


def is_repo_root() -> bool:
    result = run_result("git rev-parse --show-toplevel")
    if result.returncode != 0:
        return False
    return Path(result.stdout.strip()).resolve() == Path.cwd().resolve()


def current_branch() -> str:
    return capture("git rev-parse --abbrev-ref HEAD")


def is_clean() -> bool:
    return capture("git status --porcelain") == ""


def gh_logged_in() -> bool:
    return run_result("gh auth status").returncode == 0


def last_tag() -> str | None:
    result = run_result("git tag --sort=-v:refname -l 'v*'")
    tags = [t for t in result.stdout.strip().splitlines() if t]
    return tags[0] if tags else None


def commits_since(tag: str | None) -> str:
    if tag:
        return capture(f"git log {tag}..HEAD --oneline")
    return capture("git log --oneline")
