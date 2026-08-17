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
	@echo "  build          Build release binary"
	@echo "  build-all      Build release binary + Java Plugin SDK"
	@echo "  app            Build TermLab.app for current macOS architecture"
	@echo "  dmg-native     Build DMG + .app for current macOS architecture"
	@echo "  dmg-universal  Build universal DMG (ARM64 + x86_64, macOS only)"
	@echo "  deb            Build .deb package (run on Linux)"
	@echo "  rpm            Build .rpm package (run on Linux)"
	@echo "  msi            Build .msi installer (run on Windows)"
	@echo "  exe            Build portable .exe (run on Windows)"
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

.PHONY: build
build:
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
define assemble_app
	rm -rf "$(APP)"
	mkdir -p "$(APP)/Contents/MacOS" "$(APP)/Contents/Resources"
	cp $(1) "$(APP)/Contents/MacOS/termlab"
	cp packaging/macos/Info.plist "$(APP)/Contents/"
	cp "$(ICNS)" "$(APP)/Contents/Resources/termlab.icns"
	codesign --remove-signature "$(APP)" 2>/dev/null || true
	codesign --force --deep --sign - "$(APP)"
	@[ -x "$(APP)/Contents/MacOS/termlab" ] || { echo "error: app bundle has no executable"; exit 1; }
	@[ -f "$(APP)/Contents/Resources/termlab.icns" ] || { echo "error: app bundle has no icon"; exit 1; }
endef

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
dmg-universal: check-macos-tools java-sdk
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

# ---------------------------------------------------------------------------
# Windows — .msi installer (run on Windows)
# ---------------------------------------------------------------------------
.PHONY: msi
msi: build
	@mkdir -p "$(DIST)"
	wix extension add WixToolset.UI.wixext/4.0.5 WixToolset.Util.wixext/4.0.5 2>/dev/null || true
	wix build -arch "x64" -ext WixToolset.UI.wixext -ext WixToolset.Util.wixext \
		-out "$(DIST)/TermLab-v$(VERSION)-installer.msi" \
		"packaging/windows/termlab.wxs"
	@echo "Built $(DIST)/TermLab-v$(VERSION)-installer.msi"

# ---------------------------------------------------------------------------
# Windows — portable .exe (run on Windows)
# ---------------------------------------------------------------------------
.PHONY: exe
exe: build
	@mkdir -p "$(DIST)"
	cp target/release/termlab.exe "$(DIST)/TermLab-v$(VERSION)-portable.exe"
	@echo "Built $(DIST)/TermLab-v$(VERSION)-portable.exe"

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
	python3 scripts/set_msi_version.py "$(V_NUMERIC)"
	cargo check --workspace
	@# Fail loudly rather than shipping a bundle that disagrees with the binary.
	@grep -q "<string>$(V_NUMERIC)</string>" packaging/macos/Info.plist \
		|| { echo "error: Info.plist was not updated to $(V_NUMERIC)"; exit 1; }
	@grep -q 'Codepage="1252" Version="$(V_NUMERIC)"' packaging/windows/termlab.wxs \
		|| { echo "error: termlab.wxs Package Version was not updated to $(V_NUMERIC)"; exit 1; }
	@grep -q 'InstallerVersion="200"' packaging/windows/termlab.wxs \
		|| { echo "error: InstallerVersion was clobbered — it declares the minimum Windows Installer version and must stay 200"; exit 1; }
	@echo "Version bumped: Cargo=$(V), bundle short=$(V_NUMERIC). Review with 'git diff', then commit."

.PHONY: release
release:
ifndef V
	$(error Usage: make release V=x.y.z)
endif
	@echo "Releasing v$(V)..."
	$(MAKE) bump V=$(V)
	git add Cargo.toml packaging/macos/Info.plist packaging/windows/termlab.wxs Cargo.lock
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
