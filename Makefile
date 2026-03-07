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
	@echo "Targets:"
	@echo "  dmg-universal  Build universal DMG (ARM64 + x86_64)"
	@echo "  dmg-native     Build DMG for current architecture"
	@echo "  linux-amd64    Build .deb and .rpm for Linux AMD64 (requires cross)"
	@echo "  linux-arm64    Build .deb and .rpm for Linux ARM64 (requires cross)"
	@echo "  windows        Build .exe for Windows x86_64 (requires cross)"
	@echo "  all            Build all targets"
	@echo "  release V=x.y.z  Bump version, tag, and push"
	@echo "  clean          Remove build artifacts"
	@echo ""
	@echo "Version: $(VERSION)"

# ---------------------------------------------------------------------------
# macOS Universal (fat binary: ARM64 + x86_64)
# ---------------------------------------------------------------------------
.PHONY: dmg-universal
dmg-universal:
	rustup target add aarch64-apple-darwin x86_64-apple-darwin 2>/dev/null || true
	cargo build --release -p conch_app --target=aarch64-apple-darwin
	cargo build --release -p conch_app --target=x86_64-apple-darwin
	@mkdir -p "$(DIST)"
	rm -rf "$(APP)"
	mkdir -p "$(APP)/Contents/MacOS" "$(APP)/Contents/Resources"
	lipo -create \
		target/aarch64-apple-darwin/release/conch \
		target/x86_64-apple-darwin/release/conch \
		-output "$(APP)/Contents/MacOS/conch"
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
# macOS Native (current architecture only)
# ---------------------------------------------------------------------------
.PHONY: dmg-native
dmg-native:
	cargo build --release -p conch_app
	@mkdir -p "$(DIST)"
	rm -rf "$(APP)"
	mkdir -p "$(APP)/Contents/MacOS" "$(APP)/Contents/Resources"
	cp target/release/conch "$(APP)/Contents/MacOS/"
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
# Linux AMD64 (requires cross: cargo install cross)
# ---------------------------------------------------------------------------
.PHONY: linux-amd64
linux-amd64:
	cross build --release -p conch_app --target x86_64-unknown-linux-gnu
	@mkdir -p "$(DIST)"
	cargo deb -p conch_app --no-build --target x86_64-unknown-linux-gnu
	cargo generate-rpm -p crates/conch_app --target x86_64-unknown-linux-gnu
	cp target/x86_64-unknown-linux-gnu/debian/*.deb "$(DIST)/conch-v$(VERSION)-amd64.deb"
	cp target/x86_64-unknown-linux-gnu/generate-rpm/*.rpm "$(DIST)/conch-v$(VERSION)-1.x86_64.rpm"
	@echo "Built Linux AMD64 packages in $(DIST)/"

# ---------------------------------------------------------------------------
# Linux ARM64 (requires cross: cargo install cross)
# ---------------------------------------------------------------------------
.PHONY: linux-arm64
linux-arm64:
	cross build --release -p conch_app --target aarch64-unknown-linux-gnu
	@mkdir -p "$(DIST)"
	cargo deb -p conch_app --no-build --no-strip --target aarch64-unknown-linux-gnu
	cargo generate-rpm -p crates/conch_app --target aarch64-unknown-linux-gnu
	cp target/aarch64-unknown-linux-gnu/debian/*.deb "$(DIST)/conch-v$(VERSION)-arm64.deb"
	cp target/aarch64-unknown-linux-gnu/generate-rpm/*.rpm "$(DIST)/conch-v$(VERSION)-1.aarch64.rpm"
	@echo "Built Linux ARM64 packages in $(DIST)/"

# ---------------------------------------------------------------------------
# Windows x86_64 (requires cross: cargo install cross)
# ---------------------------------------------------------------------------
.PHONY: windows
windows:
	cross build --release -p conch_app --target x86_64-pc-windows-msvc
	@mkdir -p "$(DIST)"
	cp target/x86_64-pc-windows-msvc/release/conch.exe "$(DIST)/Conch-v$(VERSION)-portable.exe"
	@echo "Built $(DIST)/Conch-v$(VERSION)-portable.exe"

# ---------------------------------------------------------------------------
# All
# ---------------------------------------------------------------------------
.PHONY: all
all: dmg-universal linux-amd64 linux-arm64 windows

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
	git commit -m "release: v$(V)"
	git tag -a "v$(V)" -m "v$(V)"
	git push origin main
	git push origin "v$(V)"
	@echo "Released v$(V)"

# ---------------------------------------------------------------------------
# Clean
# ---------------------------------------------------------------------------
.PHONY: clean
clean:
	rm -rf "$(APP)" "$(DIST)"
	cargo clean
