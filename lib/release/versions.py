import re

from shell import ROOT
from ui import fail, prompt, warn

CARGO_TOML = ROOT / "Cargo.toml"
VERSION_RE = re.compile(r'^(version\s*=\s*")(\d+\.\d+\.\d+)(")', re.MULTILINE)


def read_current_version() -> str:
    text = CARGO_TOML.read_text()
    m = VERSION_RE.search(text)
    if not m:
        fail("Could not find version in Cargo.toml [workspace.package]")
    return m.group(2)


def bump(version: str, part: str) -> str:
    major, minor, patch = map(int, version.split("."))
    if part == "major":
        return f"{major + 1}.0.0"
    if part == "minor":
        return f"{major}.{minor + 1}.0"
    return f"{major}.{minor}.{patch + 1}"


def write_version(new_version: str) -> None:
    text = CARGO_TOML.read_text()
    new_text: str = VERSION_RE.sub(
        lambda m: m.group(1) + new_version + m.group(3), text, count=1
    )
    if new_text == text:
        fail("Version replacement had no effect – check Cargo.toml format")
    _ = CARGO_TOML.write_text(new_text)


def choose_version(current: str) -> str:
    from colors import bold, dim  # local import to avoid cycle with ui

    patch_v = bump(current, "patch")
    minor_v = bump(current, "minor")
    major_v = bump(current, "major")

    print(f"\n  Current version: {bold(current)}")
    print(f"  {dim('(1)')} patch  →  {bold(patch_v)}")
    print(f"  {dim('(2)')} minor  →  {bold(minor_v)}")
    print(f"  {dim('(3)')} major  →  {bold(major_v)}")
    print(f"  {dim('(4)')} custom")

    choice = prompt("Choose (1/2/3/4 or type a version directly)", default="1")

    if choice == "1":
        return patch_v
    if choice == "2":
        return minor_v
    if choice == "3":
        return major_v
    if choice == "4":
        return prompt("Enter version (x.y.z)")

    # Maybe they typed a version directly
    if re.match(r"^\d+\.\d+\.\d+$", choice):
        return choice

    warn(f"Unrecognised choice '{choice}', defaulting to patch bump")
    return patch_v
