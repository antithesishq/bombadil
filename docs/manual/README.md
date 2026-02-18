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
- `target/html/` - Chunked HTML site
- `target/bombadil-manual.epub` - EPUB e-book
- `target/bombadil-manual.pdf` - PDF document
- `target/bombadil-manual.txt` - Plain text

## Structure

- `src/` - Source files (markdown, assets, styles)
  - `01-introduction.md` - Introduction chapter
  - `02-getting-started.md` - Getting started guide
  - `03-specification-language.md` - Specification language docs
  - `04-reference.md` - API and CLI reference
  - `metadata.yaml` - EPUB metadata
  - `style.css` - Styles for HTML output
- `target/` - Build output (gitignored)
- `Makefile` - Build system

## Adding Content

Add markdown files in the `src/` directory. Use numeric prefixes (01-, 02-, etc.) to control the order:

```bash
cd src/
# Add a new chapter
echo "# My Chapter" > 05-my-chapter.md
```

Files are automatically sorted and combined during the build. The HTML output will be split into multiple pages at level-2 headers (`##`) due to the `--split-level=2` option.
