VERSION := $(shell grep '^version' Cargo.toml | head -1 | sed 's/.*"\(.*\)"/\1/')
APP      = Conch.app
DIST     = dist

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
	@echo "  dmg-native     Build DMG for current macOS architecture"
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
	cargo build --release -p conch_tauri
	@echo "Binary at target/release/conch"

.PHONY: build-all
build-all: java-sdk build

# ---------------------------------------------------------------------------
# macOS — DMG (current architecture)
# ---------------------------------------------------------------------------
.PHONY: dmg-native
dmg-native: build
	@mkdir -p "$(DIST)"
	rm -rf "$(APP)"
	mkdir -p "$(APP)/Contents/MacOS" "$(APP)/Contents/Resources"
	cp target/release/conch "$(APP)/Contents/MacOS/"
	cp packaging/macos/Info.plist "$(APP)/Contents/"
	@if [ -f crates/conch_tauri/icons/icon.icns ]; then \
		cp crates/conch_tauri/icons/icon.icns "$(APP)/Contents/Resources/conch.icns"; \
	fi
	codesign --remove-signature "$(APP)" 2>/dev/null || true
	codesign --force --deep --sign - "$(APP)"
	mkdir -p dmg-staging && mv "$(APP)" dmg-staging/
	create-dmg \
		--volname "Conch" \
		--window-pos 200 120 \
		--window-size 600 400 \
		--icon-size 80 \
		--icon "Conch.app" 150 200 \
		--hide-extension "Conch.app" \
		--app-drop-link 450 200 \
		--no-internet-enable \
		"$(DIST)/Conch-v$(VERSION)-$(shell uname -m).dmg" \
		"dmg-staging/" || true
	rm -rf dmg-staging
	@echo "Built $(DIST)/Conch-v$(VERSION)-$(shell uname -m).dmg"

# ---------------------------------------------------------------------------
# macOS — Universal DMG (ARM64 + x86_64)
# ---------------------------------------------------------------------------
.PHONY: dmg-universal
dmg-universal: java-sdk
	rustup target add aarch64-apple-darwin x86_64-apple-darwin 2>/dev/null || true
	cargo build --release -p conch_tauri --target=aarch64-apple-darwin
	cargo build --release -p conch_tauri --target=x86_64-apple-darwin
	@mkdir -p "$(DIST)"
	rm -rf "$(APP)"
	mkdir -p "$(APP)/Contents/MacOS" "$(APP)/Contents/Resources"
	lipo -create \
		target/aarch64-apple-darwin/release/conch \
		target/x86_64-apple-darwin/release/conch \
		-output "$(APP)/Contents/MacOS/conch"
	cp packaging/macos/Info.plist "$(APP)/Contents/"
	@if [ -f crates/conch_tauri/icons/icon.icns ]; then \
		cp crates/conch_tauri/icons/icon.icns "$(APP)/Contents/Resources/conch.icns"; \
	fi
	codesign --remove-signature "$(APP)" 2>/dev/null || true
	codesign --force --deep --sign - "$(APP)"
	mkdir -p dmg-staging && mv "$(APP)" dmg-staging/
	create-dmg \
		--volname "Conch" \
		--window-pos 200 120 \
		--window-size 600 400 \
		--icon-size 80 \
		--icon "Conch.app" 150 200 \
		--hide-extension "Conch.app" \
		--app-drop-link 450 200 \
		--no-internet-enable \
		"$(DIST)/Conch-v$(VERSION).dmg" \
		"dmg-staging/" || true
	rm -rf dmg-staging
	@echo "Built $(DIST)/Conch-v$(VERSION).dmg"

# ---------------------------------------------------------------------------
# Linux — .deb (run on Linux, builds natively)
# ---------------------------------------------------------------------------
.PHONY: deb
deb: build
	@mkdir -p "$(DIST)"
	cargo deb -p conch_tauri --no-build
	cp target/debian/*.deb "$(DIST)/conch-v$(VERSION)-$$(dpkg --print-architecture).deb"
	@echo "Built $(DIST)/conch-v$(VERSION)-$$(dpkg --print-architecture).deb"

# ---------------------------------------------------------------------------
# Linux — .rpm (run on Linux, builds natively)
# ---------------------------------------------------------------------------
.PHONY: rpm
rpm: build
	@mkdir -p "$(DIST)"
	cargo generate-rpm -p crates/conch_tauri
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
		-out "$(DIST)/Conch-v$(VERSION)-installer.msi" \
		"packaging/windows/conch.wxs"
	@echo "Built $(DIST)/Conch-v$(VERSION)-installer.msi"

# ---------------------------------------------------------------------------
# Windows — portable .exe (run on Windows)
# ---------------------------------------------------------------------------
.PHONY: exe
exe: build
	@mkdir -p "$(DIST)"
	cp target/release/conch.exe "$(DIST)/Conch-v$(VERSION)-portable.exe"
	@echo "Built $(DIST)/Conch-v$(VERSION)-portable.exe"

# ===========================================================================
# RELEASE & UTILITIES
# ===========================================================================

.PHONY: bump
bump:
ifndef V
	$(error Usage: make bump V=x.y.z)
endif
	@echo "Bumping version to $(V)..."
	sed -i '' 's/^version = ".*"/version = "$(V)"/' Cargo.toml
	sed -i '' 's|<string>[0-9][0-9]*\.[0-9][0-9]*\.[0-9][0-9]*</string>|<string>$(V)</string>|g' packaging/macos/Info.plist
	sed -i '' 's|Version="[0-9][0-9]*\.[0-9][0-9]*\.[0-9][0-9]*"|Version="$(V)"|g' packaging/windows/conch.wxs
	cargo check --workspace
	@echo "Version bumped to $(V). Review with 'git diff', then commit."

.PHONY: release
release:
ifndef V
	$(error Usage: make release V=x.y.z)
endif
	@echo "Releasing v$(V)..."
	$(MAKE) bump V=$(V)
	git add Cargo.toml packaging/macos/Info.plist packaging/windows/conch.wxs Cargo.lock
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
