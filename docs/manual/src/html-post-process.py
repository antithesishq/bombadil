#!/usr/bin/env python3
"""
Post-processing of HTML output files, adjusting heading levels
so that all files start at h1. Also adds heading anchor links
and word break hints.

Usage: html-post-process.py TARGET_DIR
"""
import sys
import pathlib
import re
from typing import Literal, Union, cast
from bs4 import BeautifulSoup, ResultSet, Tag


type Level = Union[
    Literal[1], Literal[2], Literal[3], Literal[4], Literal[5], Literal[6]
]


def heading_level(name: str) -> Level | None:
    """Return the heading level (1-6) if `name` is h1..h6, else None."""
    if len(name) == 2 and name[0] == "h" and name[1] in "123456":
        return cast(Level, int(name[1]))
    return None


def level_class(cls: str):
    """Return the level (1-6) if `cls` is level1..level6, else None."""
    if cls.startswith("level") and cls[5:] in ("1", "2", "3", "4", "5", "6"):
        return int(cls[5:])
    return None


def shift_headings(headings: list) -> None:
    levels: set[int] = {
        level
        for heading in headings
        for level in [heading_level(heading.name)]
        if level is not None
    }
    shift = min(levels) - 1
    if shift <= 0:
        # Document already starts at h1.
        return

    for heading in headings:
        level = heading_level(heading.name)
        if level is not None:
            heading.name = f"h{level - shift}"


def add_anchor_links(soup: BeautifulSoup, headings: list) -> None:
    for heading in headings:
        if not heading.get("id"):
            continue
        if heading.find("a", class_="header-anchor") is not None:
            # Heading already has an anchor link.
            continue
        anchor = soup.new_tag(
            "a", href=f"#{heading['id']}", attrs={"class": "header-anchor"}
        )
        anchor.string = "#"
        heading.append(" ")
        heading.append(anchor)


def word_break_on_slash(soup: BeautifulSoup, tags: ResultSet[Tag]) -> None:
    for tag in tags:
        text = tag.string
        if not text:
            continue
        tag.clear()
        for i, segment in enumerate(text.split("/")):
            if i > 0:
                tag.append("/")
                tag.append(soup.new_tag("wbr"))
            tag.append(segment)


def fix_file(path: pathlib.Path) -> None:
    text = path.read_text(encoding="utf-8")
    soup = BeautifulSoup(text, "html.parser")

    headings = soup.find_all(re.compile("^h[1-6]$"))
    if headings:
        shift_headings(headings)
        add_anchor_links(soup, headings)

    word_break_on_slash(soup, soup.select("#TOC a"))
    word_break_on_slash(soup, soup.select("h1 .citation, h2 .citation, h3 .citation"))

    path.write_text(str(soup), encoding="utf-8")


def main() -> None:
    if len(sys.argv) != 2:
        sys.exit(f"usage: {sys.argv[0]} OUTPUT_DIR")
    out_dir = pathlib.Path(sys.argv[1])
    for html_file in out_dir.glob("**/*.html"):
        fix_file(html_file)


if __name__ == "__main__":
    main()
