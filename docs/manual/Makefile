.PHONY: all html epub pdf txt clean serve watch dev

SRC_DIR := src
TARGET_DIR := target
# Sort sources to ensure consistent ordering (01-, 02-, etc.)
SOURCES := $(sort $(wildcard $(SRC_DIR)/*.md))
# Extract version from parent Cargo.toml
VERSION := $(shell grep '^version' ../../Cargo.toml | head -1 | sed 's/.*"\(.*\)"/\1/')

# Output files
HTML_DIR := $(TARGET_DIR)/html
EPUB_FILE := $(TARGET_DIR)/bombadil-manual.epub
PDF_FILE := $(TARGET_DIR)/bombadil-manual.pdf
TXT_FILE := $(TARGET_DIR)/bombadil-manual.txt

# Pandoc options
PANDOC := pandoc
PANDOC_HTML_OPTS := --to=chunkedhtml --toc --toc-depth=3 -Vtoc-title="The Bombadil Manual" \
	--split-level=1 --resource-path=$(SRC_DIR) --css=reset.css --css=style.css \
	--template=$(SRC_DIR)/template.html -M version=$(VERSION)
PANDOC_EPUB_OPTS := --toc --toc-depth=3 \
	--epub-metadata=$(SRC_DIR)/metadata.yaml --resource-path=$(SRC_DIR) -M version=$(VERSION)
PANDOC_PDF_OPTS := --toc --toc-depth=3 \
	--pdf-engine=lualatex --resource-path=$(SRC_DIR) \
	--highlight-style=monochrome \
	-V mainfont="Inter" \
	-V monofont="JetBrains Mono" \
	-M version=$(VERSION) \
	-V subtitle="Version $(VERSION)" \
	-V geometry="margin=1in" \
	-V header-includes='\usepackage{fancyhdr}\pagestyle{fancy}\fancyhf{}\fancyhead[L]{The Bombadil Manual v$(VERSION)}\fancyhead[R]{\leftmark}\fancyfoot[C]{\thepage}\usepackage{titling}\pretitle{\begin{center}\LARGE\bfseries}\posttitle{\par\end{center}}'
PANDOC_TXT_OPTS := --to plain --wrap=none

all: html epub pdf txt

html: $(HTML_DIR)/.built

epub: $(EPUB_FILE)

pdf: $(PDF_FILE)

txt: $(TXT_FILE)

$(TARGET_DIR):
	mkdir -p $(TARGET_DIR)

# For chunked HTML, pandoc creates the directory itself
# Remove it first if it exists, then let pandoc create it
$(HTML_DIR)/.built: $(SOURCES) $(SRC_DIR)/template.html $(wildcard $(SRC_DIR)/*.css) $(wildcard $(SRC_DIR)/*.js) | $(TARGET_DIR)
	rm -rf $(HTML_DIR)
	$(PANDOC) $(SOURCES) --output $(HTML_DIR)/ $(PANDOC_HTML_OPTS)
	cp $(SRC_DIR)/*.css $(HTML_DIR)/
	cp $(SRC_DIR)/*.js $(HTML_DIR)/
	touch $(HTML_DIR)/.built

$(EPUB_FILE): $(SOURCES) | $(TARGET_DIR)
	$(PANDOC) $(SOURCES) -o $(EPUB_FILE) $(PANDOC_EPUB_OPTS)

$(PDF_FILE): $(SOURCES) | $(TARGET_DIR)
	$(PANDOC) $(SOURCES) -o $(PDF_FILE) $(PANDOC_PDF_OPTS)

$(TXT_FILE): $(SOURCES) | $(TARGET_DIR)
	$(PANDOC) $(SOURCES) -o $(TXT_FILE) $(PANDOC_TXT_OPTS)

clean:
	rm -rf $(TARGET_DIR)

# Development server with live reload
serve: html
	@browser-sync start --server $(HTML_DIR) --port 8000 --no-open

# Watch for changes and rebuild
watch:
	watchexec -w $(SRC_DIR) -e md,css,html,js,yaml make html

# Development mode: rebuild and serve with live reload
dev: clean all
	@concurrently -n serve,watch \
		"browser-sync start --server $(HTML_DIR) --port 8000 --no-open --files '$(HTML_DIR)/**/*'" \
		"watchexec -w $(SRC_DIR) -e css,html,js,md,yaml make html"
