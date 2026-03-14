VERSION := $(shell grep '^version' Cargo.toml | head -1 | sed 's/.*"\(.*\)"/\1/')
APP      = Conch.app
DIST     = dist
PLUGINS_DYLIB = libconch_ssh.dylib libconch_files.dylib
PLUGINS_SO    = libconch_ssh.so libconch_files.so
PLUGINS_DLL   = conch_ssh.dll conch_files.dll

# ---------------------------------------------------------------------------
# Default
# ---------------------------------------------------------------------------
.PHONY: help
help:
	@echo "Usage: make <target>"
	@echo ""
	@echo "Local builds (run on the target platform):"
	@echo "  dmg-native     Build DMG for current macOS architecture"
	@echo "  dmg-universal  Build universal DMG (ARM64 + x86_64, macOS only)"
	@echo "  deb            Build .deb package (run on Linux)"
	@echo "  rpm            Build .rpm package (run on Linux)"
	@echo "  msi            Build .msi installer (run on Windows)"
	@echo "  exe            Build portable .exe (run on Windows)"
	@echo ""
	@echo "Cross-compilation (requires cross: cargo install cross):"
	@echo "  linux-amd64    Build .deb and .rpm for Linux AMD64"
	@echo "  linux-arm64    Build .deb and .rpm for Linux ARM64"
	@echo "  windows-cross  Build .exe for Windows x86_64"
	@echo ""
	@echo "Other:"
	@echo "  all            Build all cross targets"
	@echo "  release V=x.y.z  Bump version, tag, and push"
	@echo "  clean          Remove build artifacts"
	@echo "  changelog      Generate release notes locally"
	@echo ""
	@echo "Version: $(VERSION)"

# ===========================================================================
# LOCAL BUILDS — run these on the target platform
# ===========================================================================

# ---------------------------------------------------------------------------
# macOS — DMG (current architecture)
# ---------------------------------------------------------------------------
.PHONY: dmg-native
dmg-native:
	cargo build --release
	@mkdir -p "$(DIST)"
	rm -rf "$(APP)"
	mkdir -p "$(APP)/Contents/MacOS" "$(APP)/Contents/Resources" "$(APP)/Contents/Plugins"
	cp target/release/conch "$(APP)/Contents/MacOS/"
	@for p in $(PLUGINS_DYLIB); do cp "target/release/$$p" "$(APP)/Contents/Plugins/"; done
	cp packaging/macos/Info.plist "$(APP)/Contents/"
	cp crates/conch_app/icons/conch.icns "$(APP)/Contents/Resources/"
	codesign --remove-signature "$(APP)" 2>/dev/null || true
	codesign --force --deep --sign - "$(APP)"
	hdiutil create -volname "Conch" -srcfolder "$(APP)" \
		-fs HFS+ -ov -format UDZO \
		"$(DIST)/Conch-v$(VERSION)-$(shell uname -m).dmg"
	rm -rf "$(APP)"
	@echo "Built $(DIST)/Conch-v$(VERSION)-$(shell uname -m).dmg"

# ---------------------------------------------------------------------------
# macOS — Universal DMG (ARM64 + x86_64)
# ---------------------------------------------------------------------------
.PHONY: dmg-universal
dmg-universal:
	rustup target add aarch64-apple-darwin x86_64-apple-darwin 2>/dev/null || true
	cargo build --release --target=aarch64-apple-darwin
	cargo build --release --target=x86_64-apple-darwin
	@mkdir -p "$(DIST)"
	rm -rf "$(APP)"
	mkdir -p "$(APP)/Contents/MacOS" "$(APP)/Contents/Resources" "$(APP)/Contents/Plugins"
	lipo -create \
		target/aarch64-apple-darwin/release/conch \
		target/x86_64-apple-darwin/release/conch \
		-output "$(APP)/Contents/MacOS/conch"
	@for p in $(PLUGINS_DYLIB); do \
		lipo -create \
			"target/aarch64-apple-darwin/release/$$p" \
			"target/x86_64-apple-darwin/release/$$p" \
			-output "$(APP)/Contents/Plugins/$$p"; \
	done
	cp packaging/macos/Info.plist "$(APP)/Contents/"
	cp crates/conch_app/icons/conch.icns "$(APP)/Contents/Resources/"
	codesign --remove-signature "$(APP)" 2>/dev/null || true
	codesign --force --deep --sign - "$(APP)"
	hdiutil create -volname "Conch" -srcfolder "$(APP)" \
		-fs HFS+ -ov -format UDZO \
		"$(DIST)/Conch-v$(VERSION).dmg"
	rm -rf "$(APP)"
	@echo "Built $(DIST)/Conch-v$(VERSION).dmg"

# ---------------------------------------------------------------------------
# Linux — .deb (run on Linux, builds natively)
# ---------------------------------------------------------------------------
.PHONY: deb
deb:
	cargo build --release
	@mkdir -p "$(DIST)"
	cargo deb -p conch_app --no-build
	cp target/debian/*.deb "$(DIST)/conch-v$(VERSION)-$$(dpkg --print-architecture).deb"
	@echo "Built $(DIST)/conch-v$(VERSION)-$$(dpkg --print-architecture).deb"

# ---------------------------------------------------------------------------
# Linux — .rpm (run on Linux, builds natively)
# ---------------------------------------------------------------------------
.PHONY: rpm
rpm:
	cargo build --release
	@mkdir -p "$(DIST)"
	cargo generate-rpm -p crates/conch_app
	cp target/generate-rpm/*.rpm "$(DIST)/"
	@echo "Built RPM in $(DIST)/"

# ---------------------------------------------------------------------------
# Windows — .msi installer (run on Windows)
# ---------------------------------------------------------------------------
.PHONY: msi
msi:
	cargo build --release
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
exe:
	cargo build --release
	@mkdir -p "$(DIST)"
	cp target/release/conch.exe "$(DIST)/Conch-v$(VERSION)-portable.exe"
	@for p in $(PLUGINS_DLL); do cp "target/release/$$p" "$(DIST)/" 2>/dev/null || true; done
	@echo "Built $(DIST)/Conch-v$(VERSION)-portable.exe"

# ===========================================================================
# CROSS-COMPILATION — build from any platform (requires cross)
# ===========================================================================

# ---------------------------------------------------------------------------
# Linux AMD64 (cross-compile)
# ---------------------------------------------------------------------------
.PHONY: linux-amd64
linux-amd64:
	cross build --release --target x86_64-unknown-linux-gnu
	@mkdir -p "$(DIST)"
	cargo deb -p conch_app --no-build --target x86_64-unknown-linux-gnu
	cargo generate-rpm -p crates/conch_app --target x86_64-unknown-linux-gnu
	cp target/x86_64-unknown-linux-gnu/debian/*.deb "$(DIST)/conch-v$(VERSION)-amd64.deb"
	cp target/x86_64-unknown-linux-gnu/generate-rpm/*.rpm "$(DIST)/conch-v$(VERSION)-1.x86_64.rpm"
	@echo "Built Linux AMD64 packages in $(DIST)/"

# ---------------------------------------------------------------------------
# Linux ARM64 (cross-compile)
# ---------------------------------------------------------------------------
.PHONY: linux-arm64
linux-arm64:
	cross build --release --target aarch64-unknown-linux-gnu
	@mkdir -p "$(DIST)"
	cargo deb -p conch_app --no-build --no-strip --target aarch64-unknown-linux-gnu
	cargo generate-rpm -p crates/conch_app --target aarch64-unknown-linux-gnu
	cp target/aarch64-unknown-linux-gnu/debian/*.deb "$(DIST)/conch-v$(VERSION)-arm64.deb"
	cp target/aarch64-unknown-linux-gnu/generate-rpm/*.rpm "$(DIST)/conch-v$(VERSION)-1.aarch64.rpm"
	@echo "Built Linux ARM64 packages in $(DIST)/"

# ---------------------------------------------------------------------------
# Windows x86_64 (cross-compile)
# ---------------------------------------------------------------------------
.PHONY: windows-cross
windows-cross:
	cross build --release --target x86_64-pc-windows-msvc
	@mkdir -p "$(DIST)"
	cp target/x86_64-pc-windows-msvc/release/conch.exe "$(DIST)/Conch-v$(VERSION)-portable.exe"
	@echo "Built $(DIST)/Conch-v$(VERSION)-portable.exe"

# ---------------------------------------------------------------------------
# All cross targets
# ---------------------------------------------------------------------------
.PHONY: all
all: dmg-universal linux-amd64 linux-arm64 windows-cross

# ===========================================================================
# RELEASE & UTILITIES
# ===========================================================================

# ---------------------------------------------------------------------------
# Release: make release V=0.2.2
# ---------------------------------------------------------------------------
.PHONY: release
release:
ifndef V
	$(error Usage: make release V=x.y.z)
endif
	@echo "Releasing v$(V)..."
	sed -i '' 's/^version = ".*"/version = "$(V)"/' Cargo.toml
	sed -i '' 's|<string>[0-9][0-9]*\.[0-9][0-9]*\.[0-9][0-9]*</string>|<string>$(V)</string>|g' packaging/macos/Info.plist
	cargo check --workspace
	git add Cargo.toml packaging/macos/Info.plist Cargo.lock
	git diff --cached --quiet || git commit -m "release: v$(V)"
	git tag -a "v$(V)" -m "v$(V)" -f
	git push origin main
	git push origin "v$(V)"
	@echo ""
	@echo "Tag v$(V) pushed — GitHub Actions will build artifacts and generate the changelog."
	@echo "To preview the changelog locally, run: make changelog"
	@echo ""

.PHONY: changelog
changelog:
	@./.github/workflows/generate_changelog.sh

# ---------------------------------------------------------------------------
# Clean
# ---------------------------------------------------------------------------
.PHONY: clean
clean:
	rm -rf "$(APP)" "$(DIST)"
	cargo clean
