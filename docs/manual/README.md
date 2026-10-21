# Bombadil Manual

This directory contains the Bombadil user manual source files.

## Building the Documentation

The manual can be built in multiple formats using the provided Makefile:

```bash
# Build all formats (HTML, EPUB, PDF, TXT)
make all

# Build specific formats
make html     # Chunked HTML site
make epub     # EPUB e-book
make pdf      # PDF document
make txt      # Plain text (for LLMs)

# Clean build artifacts
make clean

# Serve HTML locally (requires Python)
make serve
```

## Output

All built documentation will be placed in `target/`:
- `target/index.html` - Landing page with driver selection
- `target/<driver>/` - Chunked HTML site, one per driver (`browser`, `terminal`)
- `target/<driver>/bombadil-manual.epub` - EPUB e-book
- `target/<driver>/bombadil-manual.pdf` - PDF document
- `target/<driver>/bombadil-manual.txt` - Plain text
- `target/<driver>/bombadil.1` - Man page

## Structure

- `src/` - Source files (markdown, assets, styles)
  - `01-introduction.md` - Introduction chapter
  - `02-getting-started.md` - Getting started guide
  - `03-specification-language.md` - Specification language docs
  - `04-workflows.md` - Workflows: reproducing, shrinking, inspecting
  - `partials/` - Hand-written parts of the reference chapter, combined with
    generated CLI and TypeScript reference into `target/generated/<driver>/05-reference.md`
  - `metadata.yaml` - EPUB metadata
  - `index.css` - Entry point for the HTML styles
- `target/` - Build output (gitignored)
- `Makefile` - Build system

## Adding Content

Add markdown files in the `src/` directory. Use numeric prefixes (01-, 02-, etc.) to control the order:

```bash
cd src/
# Add a new chapter
echo "# My Chapter" > 06-my-chapter.md
```

Files are automatically sorted and combined during the build. The HTML output will be split into multiple pages down to level-3 headers (`###`) due to the `--split-level=3` option.
The generated reference chapter is numbered `05` so it always sorts last; number new
chapters accordingly.
