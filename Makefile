VERSION := $(shell grep '^version' Cargo.toml | head -1 | sed 's/.*"\(.*\)"/\1/')
APP      = TermLab.app
DIST     = dist
ICNS     = crates/termlab_tauri/icons/icon.icns

# ---------------------------------------------------------------------------
# Default
# ---------------------------------------------------------------------------
.PHONY: help
help:
	@echo "Usage: make <target>"
	@echo ""
	@echo "Local builds (run on the target platform):"
	@echo "  run            Build frontend bundles, then cargo run the app"
	@echo "  build          Build release binary"
	@echo "  build-all      Build release binary + Java Plugin SDK"
	@echo "  app            Build TermLab.app for current macOS architecture"
	@echo "  dmg-native     Build DMG + .app for current macOS architecture"
	@echo "  dmg-universal  Build universal DMG (ARM64 + x86_64, macOS only)"
	@echo "  package-macos  Build .app and .dmg automatically (shortcut)"
	@echo "  deb            Build .deb package (run on Linux)"
	@echo "  rpm            Build .rpm package (run on Linux)"
	@echo "  (Windows)      Run scripts/build-windows.ps1 on Windows"
	@echo ""
	@echo "SDKs:"
	@echo "  java-sdk       Build Java Plugin SDK (JAR + sources + javadoc)"
	@echo ""
	@echo "Other:"
	@echo "  bump V=x.y.z   Bump version everywhere (no tag, no push)"
	@echo "  release V=x.y.z  Bump version, commit, tag, and push"
	@echo "                    Also supports prereleases like V=x.y.z-rc.1"
	@echo "  clean          Remove build artifacts"
	@echo "  changelog      Generate release notes locally"
	@echo ""
	@echo "Version: $(VERSION)"

# ===========================================================================
# SDKs
# ===========================================================================

.PHONY: java-sdk
java-sdk:
	$(MAKE) -C java-sdk build
	@echo "Java SDK JARs in java-sdk/build/"

# ===========================================================================
# LOCAL BUILDS
# ===========================================================================

.PHONY: build frontend-vendor dev
# The CodeMirror bundle is generated, git-ignored, and only built by Tauri's
# beforeBuildCommand — which plain `cargo build` never fires. Any target that
# packages the app must depend on this or it ships an editor that shows the
# "bundle missing" toast. Same gap the linux release jobs patch in release.yml.
frontend-vendor:
	npm --prefix crates/termlab_tauri/frontend ci
	npm --prefix crates/termlab_tauri/frontend run build:vendor

# Bundle + build + run in one step. Plain `cargo run` now also self-heals the
# bundle via build.rs, so this is convenience, not a requirement.
dev:
	cargo run -p termlab_tauri

# Build the frontend vendor bundles explicitly, then build and run the app.
.PHONY: run
run: frontend-vendor
	cargo run -p termlab_tauri

build: frontend-vendor
	cargo build --release -p termlab_tauri
	@echo "Binary at target/release/termlab"

.PHONY: build-all
build-all: java-sdk build

# ---------------------------------------------------------------------------
# macOS packaging helpers
#
# These targets used to end their create-dmg call with `|| true`, so a missing
# tool or a failed image still printed "Built ..." and exited 0 with no DMG on
# disk. Everything below fails loudly instead: the prerequisites are checked
# up front with actionable messages, and each target verifies its own output
# exists before claiming success.
# ---------------------------------------------------------------------------
.PHONY: check-macos-tools
check-macos-tools:
	@[ "$$(uname)" = "Darwin" ] || { echo "error: macOS-only target (this is $$(uname))"; exit 1; }
	@command -v create-dmg >/dev/null 2>&1 || \
		echo "note: create-dmg not found, falling back to hdiutil (plain window layout). brew install create-dmg for the styled one."
	@[ -f packaging/macos/Info.plist ] || { echo "error: packaging/macos/Info.plist is missing"; exit 1; }
	@[ -f "$(ICNS)" ] || { echo "error: app icon missing at $(ICNS)"; exit 1; }

# Assemble $(APP) from an already-built binary at $(1).
#
# The bundled LSP resources are staged and signed BEFORE the outer signature:
# codesign seals Contents/Resources, so anything copied in afterwards
# invalidates the bundle. lsp-resources-arm64 signs each nested Mach-O first and
# this macro signs the app last.
define assemble_app
	rm -rf "$(APP)"
	mkdir -p "$(APP)/Contents/MacOS" "$(APP)/Contents/Resources"
	cp $(1) "$(APP)/Contents/MacOS/termlab"
	cp packaging/macos/Info.plist "$(APP)/Contents/"
	cp "$(ICNS)" "$(APP)/Contents/Resources/termlab.icns"
	$(MAKE) --no-print-directory stage-lsp-resources-if-available
	codesign --remove-signature "$(APP)" 2>/dev/null || true
	codesign --force --deep --sign - "$(APP)"
	@[ -x "$(APP)/Contents/MacOS/termlab" ] || { echo "error: app bundle has no executable"; exit 1; }
	@[ -f "$(APP)/Contents/Resources/termlab.icns" ] || { echo "error: app bundle has no icon"; exit 1; }
	@# Re-verify AFTER the outer signature. Today codesign seals loose Mach-O
	@# files in Resources by hash without rewriting them, so the receipt still
	@# matches; if that ever changes, the shipped app would fail its own runtime
	@# hash check. Catch that here rather than only in the smoke test.
	@if [ -d "$(APP)/Contents/Resources/lsp/arm64" ]; then \
		scripts/lsp/fetch-macos-arm64.sh --verify-only "$(APP)/Contents/Resources/lsp/arm64" \
			|| { echo "error: signing the app invalidated the bundled LSP receipt"; exit 1; }; \
	fi
endef

# ---------------------------------------------------------------------------
# Bundled arm64 language servers
#
# packaging/lsp/dist is git-ignored and produced only by an explicit
# `scripts/lsp/fetch-macos-arm64.sh` run — never by cargo build, build.rs, or
# app startup. `app` and `dmg-native` therefore stage it when it is present and
# say so loudly when it is not, while `lsp-resources-arm64` (asked for
# directly) fails hard on a missing or unverified tree.
# ---------------------------------------------------------------------------
LSP_DIST = packaging/lsp/dist

.PHONY: lsp-resources-arm64
lsp-resources-arm64:
	@[ "$$(uname)" = "Darwin" ] || { echo "error: macOS-only target (this is $$(uname))"; exit 1; }
	@[ "$$(uname -m)" = "arm64" ] || { echo "error: lsp-resources-arm64 stages an arm64-only resource tree, but this host is $$(uname -m)"; exit 1; }
	@[ -d "$(APP)" ] || { echo "error: no assembled $(APP) to stage into — run 'make app' or 'make dmg-native'"; exit 1; }
	@if lipo -archs "$(APP)/Contents/MacOS/termlab" 2>/dev/null | grep -q " "; then \
		echo "error: $(APP) holds a universal binary ($$(lipo -archs "$(APP)/Contents/MacOS/termlab"))."; \
		echo "       lsp-resources-arm64 ships only the arm64 server tree, so a universal artifact"; \
		echo "       would run x86_64 with arm64-only language servers. Universal packaging needs an"; \
		echo "       x86_64 resource tree first — see docs/superpowers/specs/2026-08-24-light-editor-lsp-design.md."; \
		exit 1; \
	fi
	@[ -d "$(LSP_DIST)/arm64" ] || { echo "error: $(LSP_DIST)/arm64 is missing — run scripts/lsp/fetch-macos-arm64.sh first"; exit 1; }
	scripts/lsp/fetch-macos-arm64.sh --verify-only "$(LSP_DIST)/arm64"
	rm -rf "$(APP)/Contents/Resources/lsp"
	mkdir -p "$(APP)/Contents/Resources/lsp"
	cp -R "$(LSP_DIST)/." "$(APP)/Contents/Resources/lsp/"
	@# .gitkeep exists only so a clean checkout has the directory tauri.conf.json
	@# declares as a resource. It has no business inside a shipped bundle.
	rm -f "$(APP)/Contents/Resources/lsp/.gitkeep"
	@# Nested executables are signed by scripts/lsp/fetch-macos-arm64.sh while
	@# staging, before the receipt records their bytes — re-signing them here
	@# would rewrite each Mach-O and invalidate the SHA-256 the runtime checks
	@# (and would downgrade the upstream Developer ID signatures on node and the
	@# TypeScript native compiler to ad-hoc). --verify-only re-checks every
	@# nested signature, so this still proves nested code is signed before the
	@# outer app signature is applied by assemble_app.
	scripts/lsp/fetch-macos-arm64.sh --verify-only "$(APP)/Contents/Resources/lsp/arm64"
	@echo "Staged bundled arm64 language servers into $(APP)/Contents/Resources/lsp"

# Internal helper: stage when a verified tree exists, otherwise leave the app
# LSP-less rather than breaking builds on machines that never ran the fetch.
.PHONY: stage-lsp-resources-if-available
stage-lsp-resources-if-available:
	@if [ "$$(uname -m)" != "arm64" ]; then \
		echo "note: skipping bundled LSP resources (host is $$(uname -m), tree is arm64-only)"; \
	elif [ ! -d "$(LSP_DIST)/arm64" ]; then \
		echo "note: skipping bundled LSP resources ($(LSP_DIST)/arm64 not staged; run scripts/lsp/fetch-macos-arm64.sh)"; \
	else \
		$(MAKE) --no-print-directory lsp-resources-arm64; \
	fi

# Build a DMG named $(1) from the assembled $(APP).
define make_dmg
	@mkdir -p "$(DIST)"
	rm -rf dmg-staging
	mkdir -p dmg-staging
	cp -R "$(APP)" dmg-staging/
	ln -s /Applications dmg-staging/Applications
	rm -f "$(1)"
	@if command -v create-dmg >/dev/null 2>&1; then \
		rm -f dmg-staging/Applications; \
		create-dmg \
			--volname "TermLab" \
			--window-pos 200 120 \
			--window-size 600 400 \
			--icon-size 80 \
			--icon "TermLab.app" 150 200 \
			--hide-extension "TermLab.app" \
			--app-drop-link 450 200 \
			--no-internet-enable \
			"$(1)" \
			"dmg-staging/"; \
	else \
		hdiutil create -volname "TermLab" -srcfolder dmg-staging -ov -format UDZO "$(1)"; \
	fi
	rm -rf dmg-staging
	@[ -f "$(1)" ] || { echo "error: DMG creation reported success but $(1) does not exist"; exit 1; }
	@hdiutil verify "$(1)" >/dev/null 2>&1 || { echo "error: $(1) failed hdiutil verify"; exit 1; }
	@echo "Built $(1) ($$(du -h "$(1)" | cut -f1))"
endef

# ---------------------------------------------------------------------------
# macOS — .app only (current architecture)
# ---------------------------------------------------------------------------
.PHONY: app
app: java-sdk build
	@[ "$$(uname)" = "Darwin" ] || { echo "error: macOS-only target (this is $$(uname))"; exit 1; }
	@[ -f packaging/macos/Info.plist ] || { echo "error: packaging/macos/Info.plist is missing"; exit 1; }
	@[ -f "$(ICNS)" ] || { echo "error: app icon missing at $(ICNS)"; exit 1; }
	@mkdir -p "$(DIST)"
	$(call assemble_app,target/release/termlab)
	rm -rf "$(DIST)/$(APP)"
	mv "$(APP)" "$(DIST)/"
	@echo "Built $(DIST)/$(APP)"

# ---------------------------------------------------------------------------
# macOS — DMG (current architecture)
# ---------------------------------------------------------------------------
.PHONY: dmg-native
dmg-native: check-macos-tools java-sdk build
	$(call assemble_app,target/release/termlab)
	$(call make_dmg,$(DIST)/TermLab-v$(VERSION)-$(shell uname -m).dmg)
	rm -rf "$(DIST)/$(APP)"
	mv "$(APP)" "$(DIST)/"
	@echo "Also kept $(DIST)/$(APP)"

# ---------------------------------------------------------------------------
# macOS — Universal DMG (ARM64 + x86_64)
# ---------------------------------------------------------------------------
.PHONY: dmg-universal
dmg-universal: check-macos-tools java-sdk frontend-vendor
	rustup target add aarch64-apple-darwin x86_64-apple-darwin
	cargo build --release -p termlab_tauri --target=aarch64-apple-darwin
	cargo build --release -p termlab_tauri --target=x86_64-apple-darwin
	@mkdir -p "$(DIST)"
	rm -rf "$(APP)"
	mkdir -p "$(APP)/Contents/MacOS" "$(APP)/Contents/Resources"
	lipo -create \
		target/aarch64-apple-darwin/release/termlab \
		target/x86_64-apple-darwin/release/termlab \
		-output "$(APP)/Contents/MacOS/termlab"
	@lipo -archs "$(APP)/Contents/MacOS/termlab" | grep -q "arm64" || { echo "error: universal binary missing arm64"; exit 1; }
	@lipo -archs "$(APP)/Contents/MacOS/termlab" | grep -q "x86_64" || { echo "error: universal binary missing x86_64"; exit 1; }
	cp packaging/macos/Info.plist "$(APP)/Contents/"
	cp "$(ICNS)" "$(APP)/Contents/Resources/termlab.icns"
	codesign --remove-signature "$(APP)" 2>/dev/null || true
	codesign --force --deep --sign - "$(APP)"
	$(call make_dmg,$(DIST)/TermLab-v$(VERSION).dmg)
	rm -rf "$(DIST)/$(APP)"
	mv "$(APP)" "$(DIST)/"
	@echo "Also kept $(DIST)/$(APP)"

# ---------------------------------------------------------------------------
# Linux — .deb (run on Linux, builds natively)
# ---------------------------------------------------------------------------
.PHONY: deb
deb: build
	@mkdir -p "$(DIST)"
	cargo deb -p termlab_tauri --no-build
	cp target/debian/*.deb "$(DIST)/termlab-v$(VERSION)-$$(dpkg --print-architecture).deb"
	@echo "Built $(DIST)/termlab-v$(VERSION)-$$(dpkg --print-architecture).deb"

# ---------------------------------------------------------------------------
# Linux — .rpm (run on Linux, builds natively)
# ---------------------------------------------------------------------------
.PHONY: rpm
rpm: build
	@mkdir -p "$(DIST)"
	cargo generate-rpm -p crates/termlab_tauri
	cp target/generate-rpm/*.rpm "$(DIST)/"
	@echo "Built RPM in $(DIST)/"

# ===========================================================================
# RELEASE & UTILITIES
# ===========================================================================

.PHONY: bump
bump:
ifndef V
	$(error Usage: make bump V=x.y.z)
endif
	@echo "Bumping version to $(V)..."
	@# CFBundleShortVersionString must be at most three period-separated
	@# non-negative integers — Apple rejects a pre-release suffix, and Finder
	@# renders it wrongly. So the marketing version is $(V) with any
	@# -rc.N/-beta.N stripped, while CFBundleVersion keeps the full string as
	@# the build identifier. The previous sed only matched purely numeric
	@# versions, so once the plist held "3.0.0-rc.1" it silently stopped
	@# matching and every later bump left the bundle stale.
	$(eval V_NUMERIC := $(shell echo "$(V)" | sed 's/-.*//'))
	sed -i '' 's/^version = ".*"/version = "$(V)"/' Cargo.toml
	python3 scripts/set_bundle_version.py "$(V)" "$(V_NUMERIC)"
	cargo check --workspace
	@# Fail loudly rather than shipping a bundle that disagrees with the binary.
	@grep -q "<string>$(V_NUMERIC)</string>" packaging/macos/Info.plist \
		|| { echo "error: Info.plist was not updated to $(V_NUMERIC)"; exit 1; }
	@echo "Version bumped: Cargo=$(V), bundle short=$(V_NUMERIC). Review with 'git diff', then commit."

.PHONY: release
release:
ifndef V
	$(error Usage: make release V=x.y.z)
endif
	@echo "Releasing v$(V)..."
	$(MAKE) bump V=$(V)
	git add Cargo.toml packaging/macos/Info.plist Cargo.lock
	git diff --cached --quiet || git commit -m "release: v$(V)"
	git tag -a "v$(V)" -m "v$(V)" -f
	git push origin main
	git push origin "v$(V)"
	@echo "Tag v$(V) pushed — GitHub Actions will build artifacts."

.PHONY: changelog
changelog:
	@./.github/workflows/generate_changelog.sh

.PHONY: clean
clean:
	rm -rf "$(APP)" "$(DIST)"
	$(MAKE) -C java-sdk clean
	cargo clean

# ---------------------------------------------------------------------------
# Convenience package target — builds .app and .dmg automatically
# ---------------------------------------------------------------------------
.PHONY: package-macos
package-macos: dmg-native
	@echo "Package complete: $(DIST)/TermLab.app and $(DIST)/*.dmg"
