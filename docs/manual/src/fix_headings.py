#!/usr/bin/env python3
"""
Post-process pandoc --to=chunkedhtml output so that within each chunk file,
the top-level heading is always <h1> (with descendants shifted to match),
regardless of that heading's level in the source document.

Usage: fix_headings.py OUTPUT_DIR
"""
import re
import sys
import pathlib

HEADING_OPEN_RE = re.compile(r"<h([1-6])(?=[ >])")
HEADING_CLOSE_RE = re.compile(r"</h([1-6])>")
LEVEL_CLASS_RE = re.compile(r'(?<=["\s])level([1-6])(?=["\s])')


def fix_file(path: pathlib.Path) -> None:
    text = path.read_text(encoding="utf-8")

    levels = {int(n) for n in HEADING_OPEN_RE.findall(text)}
    if not levels:
        return  # no headings in this file (e.g. index.html) - nothing to do

    shift = min(levels) - 1
    if shift <= 0:
        return  # already starts at h1 - nothing to do

    text = HEADING_OPEN_RE.sub(lambda m: f"<h{int(m.group(1)) - shift}", text)
    text = HEADING_CLOSE_RE.sub(lambda m: f"</h{int(m.group(1)) - shift}>", text)
    text = LEVEL_CLASS_RE.sub(lambda m: f"level{int(m.group(1)) - shift}", text)

    path.write_text(text, encoding="utf-8")


def main() -> None:
    if len(sys.argv) != 2:
        sys.exit(f"usage: {sys.argv[0]} OUTPUT_DIR")

    out_dir = pathlib.Path(sys.argv[1])
    for html_file in out_dir.glob("**/*.html"):
        fix_file(html_file)


if __name__ == "__main__":
    main()
