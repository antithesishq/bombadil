import os
import re
import subprocess
from pathlib import Path

from shell import ROOT

CHANGELOG = ROOT / "CHANGELOG.md"


def build_entry(version: str, raw_commits: str) -> str:
    lines = ["## " + version, ""]
    for line in raw_commits.splitlines():
        cleaned = re.sub(r"^[0-9a-f]+ ", "", line).strip()
        if cleaned:
            lines.append(f"* {cleaned}")
    lines.append("")
    return "\n".join(lines)


def prepend(entry: str) -> None:
    existing = CHANGELOG.read_text()
    title_end = existing.find("\n", existing.find("# ")) + 1
    new_text: str = existing[:title_end] + "\n" + entry + existing[title_end:]
    _ = CHANGELOG.write_text(new_text)


def open_in_editor(path: Path) -> None:
    editor = os.environ.get("EDITOR") or os.environ.get("VISUAL") or "vi"
    _ = subprocess.run([editor, str(path)])
