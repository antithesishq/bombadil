.PHONY: all html epub pdf txt man clean serve watch dev

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
MAN_FILE := $(TARGET_DIR)/bombadil.1

# Pandoc options
PANDOC := pandoc
PANDOC_HTML_OPTS := --to=chunkedhtml --toc --toc-depth=3 -Vtoc-title="The Bombadil Manual" \
	--split-level=1 --resource-path=$(SRC_DIR) --css=reset.css --css=style.css \
	--highlight-style=monochrome \
	--lua-filter=$(SRC_DIR)/replace.lua \
	--lua-filter=$(SRC_DIR)/admonitions.lua \
	--lua-filter=$(SRC_DIR)/code-nav.lua \
	--lua-filter=$(SRC_DIR)/table-nowrap.lua \
	--template=$(SRC_DIR)/template.html -M version=$(VERSION)
PANDOC_EPUB_OPTS := --toc --toc-depth=3 \
	--epub-metadata=$(SRC_DIR)/metadata.yaml --resource-path=$(SRC_DIR) \
	--highlight-style=monochrome \
	--lua-filter=$(SRC_DIR)/replace.lua \
	--lua-filter=$(SRC_DIR)/admonitions.lua \
	--lua-filter=$(SRC_DIR)/table-nowrap.lua -M version=$(VERSION)
PANDOC_PDF_OPTS := --toc --toc-depth=3 \
	--pdf-engine=lualatex --resource-path=$(SRC_DIR) \
	--lua-filter=$(SRC_DIR)/replace.lua \
	--lua-filter=$(SRC_DIR)/admonitions.lua \
	--lua-filter=$(SRC_DIR)/table-nowrap.lua \
	--listings \
	-V mainfont="IBM Plex Sans Condensed" \
	-V mainfontoptions="BoldFont=IBMPlexSansCondensed-SemiBold.otf" \
	-V monofont="IBM Plex Mono" \
	-M version=$(VERSION) \
	-V subtitle="Version $(VERSION)" \
	-V geometry="margin=1in" \
	-V header-includes='\usepackage{fontawesome5}\usepackage{xcolor}\definecolor{admonitionblue}{HTML}{286486}\definecolor{admonitionyellow}{HTML}{944927}\definecolor{admonitioncyan}{HTML}{3B8992}\definecolor{admonitionred}{HTML}{A8334C}\definecolor{listingborder}{RGB}{180,180,180}\definecolor{lightgraybg}{RGB}{248,248,248}\definecolor{lightbluebg}{RGB}{245,250,252}\definecolor{lightyellowbg}{RGB}{252,249,245}\definecolor{lightcyanbg}{RGB}{245,251,252}\definecolor{lightredbg}{RGB}{253,246,248}\usepackage{listings}\lstset{basicstyle=\ttfamily\small,columns=fullflexible,keepspaces=true,showstringspaces=false,aboveskip=1em,belowskip=1em,breaklines=true,breakatwhitespace=false,frame=single,framerule=0.5pt,rulecolor=\color{listingborder},backgroundcolor=\color{lightgraybg},xleftmargin=12pt,xrightmargin=12pt,framexleftmargin=8pt,framexrightmargin=8pt,framextopmargin=6pt,framexbottommargin=6pt,postbreak=\mbox{\textcolor{gray}{\tiny\ensuremath{\hookrightarrow}}\space}}\usepackage{tcolorbox}\newtcolorbox{admonitionblue}{colback=lightbluebg,colframe=admonitionblue,boxrule=0.5pt,leftrule=4pt,arc=0pt,left=8pt,right=8pt,top=6pt,bottom=6pt,before skip=1em,after skip=1em}\newtcolorbox{admonitionyellow}{colback=lightyellowbg,colframe=admonitionyellow,boxrule=0.5pt,leftrule=4pt,arc=0pt,left=8pt,right=8pt,top=6pt,bottom=6pt,before skip=1em,after skip=1em}\newtcolorbox{admonitioncyan}{colback=lightcyanbg,colframe=admonitioncyan,boxrule=0.5pt,leftrule=4pt,arc=0pt,left=8pt,right=8pt,top=6pt,bottom=6pt,before skip=1em,after skip=1em}\newtcolorbox{admonitionred}{colback=lightredbg,colframe=admonitionred,boxrule=0.5pt,leftrule=4pt,arc=0pt,left=8pt,right=8pt,top=6pt,bottom=6pt,before skip=1em,after skip=1em}\usepackage{fancyhdr}\pagestyle{fancy}\fancyhf{}\fancyhead[L]{The Bombadil Manual v$(VERSION)}\fancyhead[R]{\leftmark}\fancyfoot[C]{\thepage}\usepackage{titling}\pretitle{\begin{center}\LARGE\bfseries}\posttitle{\par\end{center}}\let\oldsection\section\renewcommand{\section}{\clearpage\oldsection}'
PANDOC_TXT_OPTS := --to plain --wrap=none --lua-filter=$(SRC_DIR)/table-nowrap.lua
PANDOC_MAN_OPTS := --standalone --to man -M section=1 -M title=bombadil --lua-filter=$(SRC_DIR)/table-nowrap.lua

all: html epub pdf txt man

html: $(HTML_DIR)/.built

epub: $(EPUB_FILE)

pdf: $(PDF_FILE)

txt: $(TXT_FILE)

man: $(MAN_FILE)

$(TARGET_DIR):
	mkdir -p $(TARGET_DIR)

# For chunked HTML, pandoc creates the directory itself
# Remove it first if it exists, then let pandoc create it
$(HTML_DIR)/.built: $(SOURCES) $(SRC_DIR)/template.html $(SRC_DIR)/replace.lua $(SRC_DIR)/admonitions.lua $(SRC_DIR)/code-nav.lua $(SRC_DIR)/table-nowrap.lua $(wildcard $(SRC_DIR)/*.css) $(wildcard $(SRC_DIR)/*.js) | $(TARGET_DIR)
	rm -rf $(HTML_DIR)
	$(PANDOC) $(SOURCES) --output $(HTML_DIR)/ $(PANDOC_HTML_OPTS)
	esbuild $(SRC_DIR)/index.css --bundle --outfile=$(HTML_DIR)/style.css
	cp $(SRC_DIR)/reset.css $(HTML_DIR)/
	cp $(SRC_DIR)/*.js $(HTML_DIR)/
	touch $(HTML_DIR)/.built

$(EPUB_FILE): $(SOURCES) $(SRC_DIR)/replace.lua $(SRC_DIR)/admonitions.lua $(SRC_DIR)/table-nowrap.lua | $(TARGET_DIR)
	$(PANDOC) $(SOURCES) -o $(EPUB_FILE) $(PANDOC_EPUB_OPTS)

$(PDF_FILE): $(SOURCES) $(SRC_DIR)/replace.lua $(SRC_DIR)/admonitions.lua $(SRC_DIR)/table-nowrap.lua $(SRC_DIR)/inline-code-size.lua | $(TARGET_DIR)
	$(PANDOC) $(SOURCES) -o $(PDF_FILE) $(PANDOC_PDF_OPTS)

$(TXT_FILE): $(SOURCES) | $(TARGET_DIR)
	$(PANDOC) $(SOURCES) -o $(TXT_FILE) $(PANDOC_TXT_OPTS)

$(MAN_FILE): $(SOURCES) | $(TARGET_DIR)
	$(PANDOC) $(SOURCES) -o $(MAN_FILE) $(PANDOC_MAN_OPTS)

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
