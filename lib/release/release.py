#!/usr/bin/env python3
"""
Bombadil release script.

Guides you through the full release process, automating the mechanical steps
and pausing for human review where needed.
"""

import json
import sys
import time
from pathlib import Path
from typing import TypedDict

# Allow importing sibling modules when run directly as a script
sys.path.insert(0, str(Path(__file__).parent))

from changelog import CHANGELOG, build_entry, open_in_editor, prepend
from colors import bold, cyan, dim
from git import commits_since, current_branch, gh_logged_in, is_clean, is_repo_root, last_tag
from shell import capture, run, run_result
from ui import confirm, fail, header, info, ok, pause, prompt, step, warn
from versions import CARGO_TOML, choose_version, read_current_version, write_version

TOTAL_STEPS = 8


class ReleaseJson(TypedDict):
    isDraft: bool
    name: str
    body: str
    url: str


def main() -> None:
    header("Bombadil Release Script")

    # ── Step 1: Pre-flight checks ──────────────────────────────────────────

    step(1, TOTAL_STEPS, "Pre-flight checks")

    if not is_repo_root():
        fail("Must be run from the repository root")
    else:
        ok("Running from repository root")

    branch = current_branch()
    if branch != "main":
        if not confirm(f"You are on branch '{branch}', not 'main'. Continue anyway?", default=False):
            sys.exit(0)
    else:
        ok("On branch main")

    if not is_clean():
        print()
        run("git status --short")
        if not confirm("Working tree is not clean. Continue anyway?", default=False):
            sys.exit(0)
    else:
        ok("Working tree is clean")

    if not gh_logged_in():
        fail("Not signed in to GitHub CLI. Run: gh auth login")
    else:
        ok("gh: authenticated")

    current = read_current_version()
    ok(f"Current version: {current}")

    # ── Step 2: Choose new version ─────────────────────────────────────────

    step(2, TOTAL_STEPS, "Choose new version")
    new_version = choose_version(current)
    ok(f"Releasing: {bold(new_version)}")

    branch_name = f"release/{new_version}"

    # ── Step 3: Create release branch ─────────────────────────────────────

    step(3, TOTAL_STEPS, f"Create branch {branch_name}")

    existing = capture(f"git branch --list {branch_name}")
    if existing:
        if not confirm(f"Branch '{branch_name}' already exists. Delete and recreate?", default=False):
            sys.exit(0)
        run(f"git branch -D {branch_name}")

    run(f"git checkout -b {branch_name}")
    ok(f"Switched to {branch_name}")

    # ── Step 4: Bump version & cargo check ────────────────────────────────

    step(4, TOTAL_STEPS, "Bump version in Cargo.toml")
    write_version(new_version)
    ok(f'Set version = "{new_version}" in Cargo.toml')

    info("Running cargo check (regenerates Cargo.lock)…")
    run("cargo check --quiet")
    ok("cargo check passed")

    # ── Step 5: Update CHANGELOG ───────────────────────────────────────────

    step(5, TOTAL_STEPS, "Update CHANGELOG.md")

    prev_tag = last_tag()
    if prev_tag:
        info(f"Collecting commits since {prev_tag}")
    else:
        info("No previous tag found – collecting all commits")

    raw_commits = commits_since(prev_tag)
    if not raw_commits:
        warn("No commits found since last tag")
        raw_commits = "* (no commits)"

    entry = build_entry(new_version, raw_commits)
    prepend(entry)
    ok("Prepended auto-generated entry to CHANGELOG.md")

    print(f"\n  {dim('Generated entry:')}")
    for line in entry.splitlines():
        print(f"    {dim(line)}")

    if confirm("Open CHANGELOG.md in $EDITOR to rewrite the entry?", default=True):
        open_in_editor(CHANGELOG)
        ok("CHANGELOG.md saved")
    else:
        warn("Skipping editor – remember to clean up the CHANGELOG later")

    # ── Step 6: Commit & push ──────────────────────────────────────────────

    step(6, TOTAL_STEPS, "Commit and push release branch")

    run(f"git add {CARGO_TOML} {CARGO_TOML.parent / 'Cargo.lock'} {CHANGELOG}")
    run(f'git commit -m "release v{new_version}"')
    ok(f"Committed: release v{new_version}")

    if confirm("Push branch and open a pull request?", default=True):
        run(f"git push -u origin {branch_name}")
        ok("Pushed to origin")

        pr_cmd = (
            f'gh pr create --title "release v{new_version}"'
            f' --body "Release v{new_version}\\n\\nSee CHANGELOG.md for details."'
            f" --base main"
        )
        result = run_result(pr_cmd)
        if result.returncode == 0:
            pr_url = result.stdout.strip()
            ok(f"Pull request created: {cyan(pr_url)}")
        else:
            warn("gh pr create failed – create the PR manually")
            info(result.stderr.strip())
    else:
        info("Skipped push/PR – run manually when ready")

    # ── Step 7: Wait for merge, then tag ──────────────────────────────────

    step(7, TOTAL_STEPS, "Merge PR and create tag")

    print(f"""
  {bold('What to do next:')}
  1. Review the PR on GitHub and let CI pass.
  2. Merge the PR (squash merge).
  3. Come back here and press Enter.
""")
    pause("Press Enter after the PR has been merged…")

    run("git fetch")
    ok("Fetched from origin")

    squash = capture(
        f'git log origin/main --oneline --grep="release v{new_version}" -1'
    )
    if squash:
        squash_sha = squash.split()[0]
        info(f"Found squash commit: {dim(squash)}")
    else:
        warn("Could not auto-detect squash commit.")
        squash_sha = prompt("Paste the commit SHA from GitHub to tag")

    tag = f"v{new_version}"
    run(f'git tag -a "{tag}" -m "{tag}" {squash_sha}')
    ok(f"Tagged {squash_sha[:8]} as {tag}")

    if confirm(f"Push tag {tag}?", default=True):
        run(f'git push origin "{tag}"')
        ok(f"Pushed {tag} to origin")
    else:
        info(f'Run manually: git push origin "{tag}"')

    # ── Step 8: Wait for draft release, review, publish ───────────────────

    step(8, TOTAL_STEPS, "Publish GitHub release")

    info("The release workflow will build binaries and create a draft GitHub release.")
    info(f"Polling for draft release {tag}…")

    poll_interval = 15  # seconds
    max_attempts = 40   # ~10 minutes
    release_data: ReleaseJson | None = None

    for attempt in range(1, max_attempts + 1):
        result = run_result(f"gh release view {tag} --json isDraft,name,body,url")
        if result.returncode == 0:
            parsed: ReleaseJson = json.loads(result.stdout)
            if parsed["isDraft"]:
                release_data = parsed
                break
            else:
                ok(f"Release {tag} is already published: {parsed['url']}")
                return
        print(
            f"  {dim(f'  attempt {attempt}/{max_attempts} – not ready yet, retrying in {poll_interval}s…')}",
            end="\r",
        )
        time.sleep(poll_interval)

    print()  # clear the \r line

    if release_data is None:
        warn("Timed out waiting for draft release. Check GitHub manually.")
        info("https://github.com/antithesishq/bombadil/releases")
        return

    ok(f"Draft release found: {release_data['url']}")

    print(f"\n{dim('─' * 60)}")
    run(f"gh release view {tag}")
    print(f"{dim('─' * 60)}")

    if not confirm("Publish this release?", default=True):
        info(f"Skipped. Publish manually: gh release edit {tag} --draft=false")
        return

    run(f"gh release edit {tag} --draft=false")
    ok(f"Released v{new_version}!")
    info(release_data["url"])


if __name__ == "__main__":
    main()
