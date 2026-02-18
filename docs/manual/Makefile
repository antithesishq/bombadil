.PHONY: all html epub pdf txt clean

SRC_DIR := src
TARGET_DIR := target
# Sort sources to ensure consistent ordering (01-, 02-, etc.)
SOURCES := $(sort $(wildcard $(SRC_DIR)/*.md))

# Output files
HTML_DIR := $(TARGET_DIR)/html
EPUB_FILE := $(TARGET_DIR)/bombadil-manual.epub
PDF_FILE := $(TARGET_DIR)/bombadil-manual.pdf
TXT_FILE := $(TARGET_DIR)/bombadil-manual.txt

# Pandoc options
PANDOC := pandoc
PANDOC_HTML_OPTS := --to=chunkedhtml --toc --toc-depth=3 --number-sections \
	--split-level=2 --resource-path=$(SRC_DIR)
PANDOC_EPUB_OPTS := --toc --toc-depth=3 --number-sections \
	--epub-metadata=$(SRC_DIR)/metadata.yaml --resource-path=$(SRC_DIR)
PANDOC_PDF_OPTS := --toc --toc-depth=3 --number-sections \
	--pdf-engine=pdflatex --resource-path=$(SRC_DIR)
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
$(HTML_DIR)/.built: $(SOURCES) | $(TARGET_DIR)
	rm -rf $(HTML_DIR)
	$(PANDOC) $(SOURCES) --output $(HTML_DIR)/ $(PANDOC_HTML_OPTS)
	touch $(HTML_DIR)/.built

$(EPUB_FILE): $(SOURCES) | $(TARGET_DIR)
	$(PANDOC) $(SOURCES) -o $(EPUB_FILE) $(PANDOC_EPUB_OPTS)

$(PDF_FILE): $(SOURCES) | $(TARGET_DIR)
	$(PANDOC) $(SOURCES) -o $(PDF_FILE) $(PANDOC_PDF_OPTS)

$(TXT_FILE): $(SOURCES) | $(TARGET_DIR)
	$(PANDOC) $(SOURCES) -o $(TXT_FILE) $(PANDOC_TXT_OPTS)

clean:
	rm -rf $(TARGET_DIR)

# Development server (if python is available)
serve: html
	@cd $(HTML_DIR) && python3 -m http.server 8000 || echo "Python not available for serving"
