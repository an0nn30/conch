# =============================================================================
# MobaMacOS Terminal — Makefile
# =============================================================================
#
# Quick reference:
#   make              — build the fat JAR  (default)
#   make deps         — download all Maven dependencies
#   make compile      — compile Java sources
#   make test         — compile + run tests
#   make run          — run the app via Maven exec plugin
#   make jar          — build shaded (fat) JAR — all deps + resources bundled
#   make app          — native app for the current OS
#   make app-mac      — macOS .app bundle   (must run on macOS,  JDK 14+)
#   make dmg          — macOS .dmg installer (must run on macOS)
#   make app-win      — Windows .msi installer (must run on Windows + WiX 3.x)
#   make icons-mac    — generate .icns from Paper 512×512 terminal icon
#   make icons-win    — generate .ico  from Paper 512×512 terminal icon
#   make clean        — remove target/, dist/, and generated icons
#   make help         — show this message
#
# =============================================================================

# ── App metadata ──────────────────────────────────────────────────────────────
APP_NAME    := MobaMacOS Terminal
APP_VENDOR  := MobaMacOS
APP_ID      := com.mobamacos.terminal
APP_VERSION := 1.0.0
MAIN_CLASS  := com.mobamacos.Main
DESCRIPTION := A MobaXterm-inspired SSH client for macOS

# ── Build paths ───────────────────────────────────────────────────────────────
MVN            := mvn
TARGET_DIR     := target
DIST_DIR       := dist
STAGE_DIR      := $(TARGET_DIR)/package-input

# Name that maven-shade-plugin writes (matches pom.xml artifactId + version).
# Extracted dynamically so bumping pom.xml is the only change needed.
POM_VERSION    := $(shell $(MVN) help:evaluate -Dexpression=project.version -q -DforceStdout 2>/dev/null || echo UNKNOWN)
SHADED_JAR     := $(TARGET_DIR)/mobamacos-terminal-$(POM_VERSION).jar

# ── Icon sources (from bundled Paper icon theme) ───────────────────────────────
ICON_SRC_512   := src/main/package/app-icon-512.png
ICON_SRC_1024  := src/main/package/app-icon-1024.png
ICON_MACOS     := src/main/package/mobamacos-terminal.icns
ICON_WINDOWS   := src/main/package/mobamacos-terminal.ico

# ── Detect OS + locate jpackage ───────────────────────────────────────────────
OS := $(shell uname -s 2>/dev/null || echo Windows_NT)

# Prefer JAVA_HOME from the environment; fall back to /usr/libexec/java_home (macOS)
JAVA_HOME ?= $(shell /usr/libexec/java_home 2>/dev/null || \
                     dirname $$(dirname $$(readlink -f $$(which java 2>/dev/null))))
JPACKAGE   := "$(JAVA_HOME)/bin/jpackage"

# =============================================================================
# Default
# =============================================================================

.DEFAULT_GOAL := jar
.PHONY: all deps compile test jar jar-clean run stage app app-mac dmg app-win \
        icons-mac icons-win clean help

all: jar

# =============================================================================
# Maven targets
# =============================================================================

deps:  ## Download / resolve all Maven dependencies
	$(MVN) dependency:resolve

compile:  ## Compile Java sources (skips tests)
	$(MVN) compile -DskipTests

test:  ## Compile and run unit tests
	$(MVN) test

jar:  ## Build the shaded fat JAR — all dependencies and resources bundled
	$(MVN) package -DskipTests
	@echo ""
	@echo "  Built: $(SHADED_JAR)"

jar-clean:  ## Force a clean rebuild of the fat JAR (use when incremental compile misses changes)
	$(MVN) clean package -DskipTests
	@echo ""
	@echo "  Built: $(SHADED_JAR)"

run: compile  ## Run directly via Maven exec plugin (no packaging needed)
	$(MVN) exec:java

# =============================================================================
# Staging — isolates the fat JAR so jpackage doesn't pick up stray files
# =============================================================================

stage:  ## Force-clean build + copy fat JAR into staging directory for jpackage
	$(MVN) clean package -DskipTests
	@mkdir -p "$(STAGE_DIR)"
	@cp "$(SHADED_JAR)" "$(STAGE_DIR)/"

# =============================================================================
# Icon generation
# =============================================================================

icons-mac: $(ICON_MACOS)  ## Generate .icns from the Paper 512×512 terminal icon (macOS only)

# Make will skip this if .icns is already newer than both source PNGs.
$(ICON_MACOS): $(ICON_SRC_512) $(ICON_SRC_1024)
	@echo "  Generating $(ICON_MACOS)…"
	@mkdir -p "$(dir $(ICON_MACOS))"
	@TMPDIR=$$(mktemp -d) && ICONSET="$$TMPDIR/mobamacos.iconset" && mkdir "$$ICONSET" \
	&& sips -z 16   16   "$(ICON_SRC_512)"  --out "$$ICONSET/icon_16x16.png"      >/dev/null \
	&& sips -z 32   32   "$(ICON_SRC_512)"  --out "$$ICONSET/icon_16x16@2x.png"   >/dev/null \
	&& sips -z 32   32   "$(ICON_SRC_512)"  --out "$$ICONSET/icon_32x32.png"       >/dev/null \
	&& sips -z 64   64   "$(ICON_SRC_512)"  --out "$$ICONSET/icon_32x32@2x.png"   >/dev/null \
	&& sips -z 128  128  "$(ICON_SRC_512)"  --out "$$ICONSET/icon_128x128.png"    >/dev/null \
	&& sips -z 256  256  "$(ICON_SRC_512)"  --out "$$ICONSET/icon_128x128@2x.png" >/dev/null \
	&& sips -z 256  256  "$(ICON_SRC_512)"  --out "$$ICONSET/icon_256x256.png"    >/dev/null \
	&& sips -z 512  512  "$(ICON_SRC_512)"  --out "$$ICONSET/icon_256x256@2x.png" >/dev/null \
	&& sips -z 512  512  "$(ICON_SRC_512)"  --out "$$ICONSET/icon_512x512.png"    >/dev/null \
	&& sips -z 1024 1024 "$(ICON_SRC_1024)" --out "$$ICONSET/icon_512x512@2x.png" >/dev/null \
	&& iconutil -c icns "$$ICONSET" -o "$(ICON_MACOS)" \
	&& rm -rf "$$TMPDIR"
	@echo "  Done: $(ICON_MACOS)"

icons-win: $(ICON_WINDOWS)  ## Generate .ico from Paper icon (requires ImageMagick: brew install imagemagick)

$(ICON_WINDOWS): $(ICON_SRC_512)
	@echo "  Generating $(ICON_WINDOWS)…"
	@mkdir -p "$(dir $(ICON_WINDOWS))"
	@if command -v magick >/dev/null 2>&1; then \
	    magick "$(ICON_SRC_512)" \
	        -define icon:auto-resize=256,128,64,48,32,16 \
	        "$(ICON_WINDOWS)" \
	    && echo "  Done: $(ICON_WINDOWS)"; \
	else \
	    echo "  WARNING: ImageMagick not found.  Install with: brew install imagemagick"; \
	    echo "           Falling back to PNG (Windows installer will use a default icon)."; \
	    cp "$(ICON_SRC_512)" "$(ICON_WINDOWS)"; \
	fi

# =============================================================================
# Native app packaging via jpackage (bundled JRE — no Java install required)
#
# jpackage is included with JDK 14+.  The packaged app embeds a private JRE
# so end-users need nothing pre-installed.
#
#   --type app-image  →  self-contained .app directory (macOS) / folder (Windows)
#   --type dmg        →  drag-to-install DMG (macOS only)
#   --type exe        →  installer EXE (Windows only; requires WiX Toolset 3.x)
# =============================================================================

# Common jpackage flags shared across all platform targets
JPACKAGE_COMMON_FLAGS = \
	--input          "$(STAGE_DIR)"                       \
	--main-jar       "$(notdir $(SHADED_JAR))"            \
	--main-class     "$(MAIN_CLASS)"                      \
	--name           "$(APP_NAME)"                        \
	--app-version    "$(APP_VERSION)"                     \
	--vendor         "$(APP_VENDOR)"                      \
	--description    "$(DESCRIPTION)"                     \
	--java-options   "-Xmx512m"                           \
	--java-options   "-Dfile.encoding=UTF-8"

app:  ## Package a native app for the current OS (auto-detects macOS / Windows)
ifeq ($(OS),Darwin)
	$(MAKE) app-mac
else
	$(MAKE) app-win
endif

app-mac: stage icons-mac  ## macOS self-contained .app bundle (run on macOS)
	@mkdir -p "$(DIST_DIR)/mac"
	$(JPACKAGE)                                                \
		$(JPACKAGE_COMMON_FLAGS)                               \
		--type           app-image                             \
		--dest           "$(DIST_DIR)/mac"                     \
		--icon           "$(ICON_MACOS)"                       \
		--mac-package-identifier "$(APP_ID)"                   \
		--java-options   "-Dapple.laf.useScreenMenuBar=false"
	@echo ""
	@echo "  Built: $(DIST_DIR)/mac/$(APP_NAME).app"
	@echo "  Run:   open '$(DIST_DIR)/mac/$(APP_NAME).app'"

dmg: stage icons-mac  ## macOS .dmg drag-to-install package (run on macOS)
	@mkdir -p "$(DIST_DIR)/mac"
	$(JPACKAGE)                                                \
		$(JPACKAGE_COMMON_FLAGS)                               \
		--type           dmg                                   \
		--dest           "$(DIST_DIR)/mac"                     \
		--icon           "$(ICON_MACOS)"                       \
		--mac-package-identifier "$(APP_ID)"                   \
		--java-options   "-Dapple.laf.useScreenMenuBar=false"
	@echo ""
	@echo "  Built: $(DIST_DIR)/mac/$(APP_NAME)-$(APP_VERSION).dmg"

app-win: stage icons-win  ## Windows .msi installer (run on Windows + WiX Toolset 3.x)
	@mkdir -p "$(DIST_DIR)/win"
	$(JPACKAGE)                                                \
		$(JPACKAGE_COMMON_FLAGS)                               \
		--type           msi                                   \
		--dest           "$(DIST_DIR)/win"                     \
		--icon           "$(ICON_WINDOWS)"                     \
		--win-dir-chooser                                      \
		--win-menu                                             \
		--win-shortcut
	@echo ""
	@echo "  Built: $(DIST_DIR)/win/$(APP_NAME)-$(APP_VERSION).msi"

# =============================================================================
# Housekeeping
# =============================================================================

clean:  ## Remove target/, dist/, staging, and generated icon files
	$(MVN) clean
	rm -rf "$(DIST_DIR)" "$(STAGE_DIR)" "$(ICON_MACOS)" "$(ICON_WINDOWS)"

help:  ## Show available targets and usage notes
	@echo ""
	@echo "  MobaMacOS Terminal — Makefile targets"
	@echo ""
	@grep -E '^[a-zA-Z_-]+:.*##' $(MAKEFILE_LIST) | \
	    awk 'BEGIN {FS = ":.*##"}; {printf "  make %-14s %s\n", $$1, $$2}'
	@echo ""
	@echo "  Platform notes:"
	@echo "    make app-mac / make dmg  — requires macOS with JDK 14+"
	@echo "    make app-win             — requires Windows with WiX Toolset 3.x"
	@echo "                               (download from https://wixtoolset.org/releases/)"
	@echo "    make icons-win           — requires ImageMagick (brew install imagemagick)"
	@echo ""
	@echo "  JAVA_HOME is currently: $(JAVA_HOME)"
	@echo "  jpackage:               $(JPACKAGE)"
	@echo ""
