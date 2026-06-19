.PHONY: all html epub pdf txt man clean serve watch dev

SRC_DIR := src
TARGET_DIR := target
# Sort sources to ensure consistent ordering (01-, 02-, etc.)
SOURCES := $(sort $(wildcard $(SRC_DIR)/*.md))
# Extract version from parent Cargo.toml
VERSION := $(shell grep '^version' ../../Cargo.toml | head -1 | sed 's/.*"\(.*\)"/\1/')

# Manuals are built once per driver; add a driver here and the build picks it up.
DRIVERS := browser terminal

# Per-driver outputs all live under target/<driver>/ so the manual landing
# page can link to PDF/EPUB/TXT with simple relative paths.
HTML_BUILTS := $(foreach d,$(DRIVERS),$(TARGET_DIR)/$(d)/.built)
EPUB_FILES  := $(foreach d,$(DRIVERS),$(TARGET_DIR)/$(d)/bombadil-manual.epub)
PDF_FILES   := $(foreach d,$(DRIVERS),$(TARGET_DIR)/$(d)/bombadil-manual.pdf)
TXT_FILES   := $(foreach d,$(DRIVERS),$(TARGET_DIR)/$(d)/bombadil-manual.txt)
MAN_FILES   := $(foreach d,$(DRIVERS),$(TARGET_DIR)/$(d)/bombadil.1)
LANDING     := $(TARGET_DIR)/index.html
# Shared HTML assets (one copy at the root; per-driver pages reference ../style.css etc.)
SHARED_ASSETS := $(TARGET_DIR)/style.css $(TARGET_DIR)/reset.css \
	$(TARGET_DIR)/toc-toggle.js $(TARGET_DIR)/copy-code.js \
	$(TARGET_DIR)/driver-switcher.js $(TARGET_DIR)/landing.js

# Pandoc options
PANDOC := pandoc

# Filters used by every output format.
COMMON_FILTERS := \
	--lua-filter=$(SRC_DIR)/driver.lua \
	--lua-filter=$(SRC_DIR)/replace.lua \
	--lua-filter=$(SRC_DIR)/admonitions.lua \
	--lua-filter=$(SRC_DIR)/table-nowrap.lua

PANDOC_HTML_OPTS := --to=chunkedhtml --toc --toc-depth=3 -Vtoc-title="The Bombadil Manual" \
	--split-level=1 --resource-path=$(SRC_DIR) --css=../reset.css --css=../style.css \
	--highlight-style=monochrome \
	$(COMMON_FILTERS) \
	--lua-filter=$(SRC_DIR)/code-nav.lua \
	--template=$(SRC_DIR)/template.html -M version=$(VERSION)
PANDOC_EPUB_OPTS := --toc --toc-depth=3 \
	--epub-metadata=$(SRC_DIR)/metadata.yaml --resource-path=$(SRC_DIR) \
	--highlight-style=monochrome \
	$(COMMON_FILTERS) -M version=$(VERSION)
PANDOC_PDF_OPTS := --toc --toc-depth=3 \
	--pdf-engine=lualatex --resource-path=$(SRC_DIR) \
	$(COMMON_FILTERS) \
	--listings \
	-V mainfont="IBM Plex Sans Condensed" \
	-V mainfontoptions="BoldFont=IBMPlexSansCondensed-SemiBold.otf" \
	-V monofont="IBM Plex Mono" \
	-M version=$(VERSION) \
	-V subtitle="Version $(VERSION)" \
	-V geometry="margin=1in" \
	-V header-includes='\usepackage{fontawesome5}\usepackage{xcolor}\definecolor{admonitionblue}{HTML}{286486}\definecolor{admonitionyellow}{HTML}{944927}\definecolor{admonitioncyan}{HTML}{3B8992}\definecolor{admonitionred}{HTML}{A8334C}\definecolor{listingborder}{RGB}{180,180,180}\definecolor{lightgraybg}{RGB}{248,248,248}\definecolor{lightbluebg}{RGB}{245,250,252}\definecolor{lightyellowbg}{RGB}{252,249,245}\definecolor{lightcyanbg}{RGB}{245,251,252}\definecolor{lightredbg}{RGB}{253,246,248}\usepackage{listings}\lstset{basicstyle=\ttfamily\small,columns=fullflexible,keepspaces=true,showstringspaces=false,aboveskip=1em,belowskip=1em,breaklines=true,breakatwhitespace=false,frame=single,framerule=0.5pt,rulecolor=\color{listingborder},backgroundcolor=\color{lightgraybg},xleftmargin=12pt,xrightmargin=12pt,framexleftmargin=8pt,framexrightmargin=8pt,framextopmargin=6pt,framexbottommargin=6pt,postbreak=\mbox{\textcolor{gray}{\tiny\ensuremath{\hookrightarrow}}\space}}\usepackage{tcolorbox}\newtcolorbox{admonitionblue}{colback=lightbluebg,colframe=admonitionblue,boxrule=0.5pt,leftrule=4pt,arc=0pt,left=8pt,right=8pt,top=6pt,bottom=6pt,before skip=1em,after skip=1em}\newtcolorbox{admonitionyellow}{colback=lightyellowbg,colframe=admonitionyellow,boxrule=0.5pt,leftrule=4pt,arc=0pt,left=8pt,right=8pt,top=6pt,bottom=6pt,before skip=1em,after skip=1em}\newtcolorbox{admonitioncyan}{colback=lightcyanbg,colframe=admonitioncyan,boxrule=0.5pt,leftrule=4pt,arc=0pt,left=8pt,right=8pt,top=6pt,bottom=6pt,before skip=1em,after skip=1em}\newtcolorbox{admonitionred}{colback=lightredbg,colframe=admonitionred,boxrule=0.5pt,leftrule=4pt,arc=0pt,left=8pt,right=8pt,top=6pt,bottom=6pt,before skip=1em,after skip=1em}\usepackage{fancyhdr}\pagestyle{fancy}\fancyhf{}\fancyhead[L]{The Bombadil Manual v$(VERSION)}\fancyhead[R]{\leftmark}\fancyfoot[C]{\thepage}\usepackage{titling}\pretitle{\begin{center}\LARGE\bfseries}\posttitle{\par\end{center}}\let\oldsection\section\renewcommand{\section}{\clearpage\oldsection}'
PANDOC_TXT_OPTS := --to plain --wrap=none \
	--lua-filter=$(SRC_DIR)/driver.lua \
	--lua-filter=$(SRC_DIR)/table-nowrap.lua
PANDOC_MAN_OPTS := --standalone --to man -M section=1 -M title=bombadil \
	--lua-filter=$(SRC_DIR)/driver.lua \
	--lua-filter=$(SRC_DIR)/table-nowrap.lua

all: html epub pdf txt man

html: $(HTML_BUILTS) $(LANDING) $(SHARED_ASSETS)

epub: $(EPUB_FILES)

pdf: $(PDF_FILES)

txt: $(TXT_FILES)

man: $(MAN_FILES)

$(TARGET_DIR):
	mkdir -p $(TARGET_DIR)

# For chunked HTML, pandoc creates the directory itself
# Remove it first if it exists, then let pandoc create it
$(TARGET_DIR)/%/.built: $(SOURCES) $(SRC_DIR)/template.html $(SRC_DIR)/driver.lua $(SRC_DIR)/replace.lua $(SRC_DIR)/admonitions.lua $(SRC_DIR)/code-nav.lua $(SRC_DIR)/table-nowrap.lua | $(TARGET_DIR)
	rm -rf $(TARGET_DIR)/$*
	mkdir -p $(TARGET_DIR)
	$(PANDOC) $(SOURCES) --output $(TARGET_DIR)/$*/ $(PANDOC_HTML_OPTS) -M driver=$*
	# Replace the per-driver index (pandoc's chunkedhtml TOC page) with a
	# redirect so chunked-html "previous" links and any direct visits land
	# on the single top-level manual landing.
	printf '<!DOCTYPE html><meta http-equiv="refresh" content="0; url=../index.html">' > $(TARGET_DIR)/$*/index.html
	touch $@

# Shared HTML assets live once at the root; per-driver pages and the
# landing all reference them with relative paths.
$(TARGET_DIR)/style.css: $(wildcard $(SRC_DIR)/*.css) | $(TARGET_DIR)
	mkdir -p $(TARGET_DIR)
	esbuild $(SRC_DIR)/index.css --bundle --outfile=$@

$(TARGET_DIR)/reset.css: $(SRC_DIR)/reset.css | $(TARGET_DIR)
	mkdir -p $(TARGET_DIR)
	cp $< $@

$(TARGET_DIR)/%.js: $(SRC_DIR)/%.js | $(TARGET_DIR)
	mkdir -p $(TARGET_DIR)
	cp $< $@

$(TARGET_DIR)/%/bombadil-manual.epub: $(SOURCES) $(SRC_DIR)/driver.lua $(SRC_DIR)/replace.lua $(SRC_DIR)/admonitions.lua $(SRC_DIR)/table-nowrap.lua | $(TARGET_DIR)
	mkdir -p $(dir $@)
	$(PANDOC) $(SOURCES) -o $@ $(PANDOC_EPUB_OPTS) -M driver=$*

$(TARGET_DIR)/%/bombadil-manual.pdf: $(SOURCES) $(SRC_DIR)/driver.lua $(SRC_DIR)/replace.lua $(SRC_DIR)/admonitions.lua $(SRC_DIR)/table-nowrap.lua $(SRC_DIR)/inline-code-size.lua | $(TARGET_DIR)
	mkdir -p $(dir $@)
	$(PANDOC) $(SOURCES) -o $@ $(PANDOC_PDF_OPTS) -M driver=$*

$(TARGET_DIR)/%/bombadil-manual.txt: $(SOURCES) $(SRC_DIR)/driver.lua | $(TARGET_DIR)
	mkdir -p $(dir $@)
	$(PANDOC) $(SOURCES) -o $@ $(PANDOC_TXT_OPTS) -M driver=$*

$(TARGET_DIR)/%/bombadil.1: $(SOURCES) $(SRC_DIR)/driver.lua | $(TARGET_DIR)
	mkdir -p $(dir $@)
	$(PANDOC) $(SOURCES) -o $@ $(PANDOC_MAN_OPTS) -M driver=$*

# Top-level manual landing: a static page with title, version, driver dropdown,
# and links to each driver's entry point and downloads. Shares stylesheets and
# scripts with the rest of the manual via SHARED_ASSETS.
$(LANDING): $(SRC_DIR)/landing.html | $(TARGET_DIR)
	mkdir -p $(TARGET_DIR)
	sed 's/__VERSION__/$(VERSION)/g' $(SRC_DIR)/landing.html > $@

clean:
	rm -rf $(TARGET_DIR)

# Development server with live reload
serve: html
	@browser-sync start --server $(TARGET_DIR) --port 8000 --no-open

# Watch for changes and rebuild
watch:
	watchexec -w $(SRC_DIR) -e md,css,html,js,yaml,lua make html

# Development mode: rebuild and serve with live reload
dev: clean all
	@concurrently -n serve,watch \
		"browser-sync start --server $(TARGET_DIR) --port 8000 --no-open --files '$(TARGET_DIR)/**/*'" \
		"watchexec -w $(SRC_DIR) -e css,html,js,md,yaml,lua make html"
