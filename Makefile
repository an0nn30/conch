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
	@echo "  macos-arm64   Build DMG for macOS ARM64 (native)"
	@echo "  macos-x86     Build DMG for macOS x86_64 (cross-compile)"
	@echo "  macos-universal Build universal DMG (ARM64 + x86_64)"
	@echo "  linux-amd64   Build .deb and .rpm for Linux AMD64 (requires cross)"
	@echo "  linux-arm64   Build .deb and .rpm for Linux ARM64 (requires cross)"
	@echo "  windows       Build .zip for Windows x86_64 (requires cross)"
	@echo "  all           Build all targets"
	@echo "  clean         Remove build artifacts"
	@echo ""
	@echo "Version: $(VERSION)"

# ---------------------------------------------------------------------------
# macOS ARM64
# ---------------------------------------------------------------------------
.PHONY: macos-arm64
macos-arm64:
	cargo build --release -p conch_app
	@mkdir -p "$(DIST)"
	rm -rf "$(APP)"
	mkdir -p "$(APP)/Contents/MacOS" "$(APP)/Contents/Resources"
	cp target/release/conch "$(APP)/Contents/MacOS/"
	cp packaging/macos/Info.plist "$(APP)/Contents/"
	cp crates/conch_app/icons/conch.icns "$(APP)/Contents/Resources/"
	codesign --force --deep --sign - "$(APP)"
	hdiutil create -volname "Conch" -srcfolder "$(APP)" -ov -format UDZO \
		"$(DIST)/Conch-$(VERSION)-macos-arm64.dmg"
	rm -rf "$(APP)"
	@echo "Built $(DIST)/Conch-$(VERSION)-macos-arm64.dmg"

# ---------------------------------------------------------------------------
# macOS x86_64
# ---------------------------------------------------------------------------
.PHONY: macos-x86
macos-x86:
	rustup target add x86_64-apple-darwin 2>/dev/null || true
	cargo build --release -p conch_app --target x86_64-apple-darwin
	@mkdir -p "$(DIST)"
	rm -rf "$(APP)"
	mkdir -p "$(APP)/Contents/MacOS" "$(APP)/Contents/Resources"
	cp target/x86_64-apple-darwin/release/conch "$(APP)/Contents/MacOS/"
	cp packaging/macos/Info.plist "$(APP)/Contents/"
	cp crates/conch_app/icons/conch.icns "$(APP)/Contents/Resources/"
	codesign --force --deep --sign - "$(APP)"
	hdiutil create -volname "Conch" -srcfolder "$(APP)" -ov -format UDZO \
		"$(DIST)/Conch-$(VERSION)-macos-x86_64.dmg"
	rm -rf "$(APP)"
	@echo "Built $(DIST)/Conch-$(VERSION)-macos-x86_64.dmg"

# ---------------------------------------------------------------------------
# macOS Universal (fat binary: ARM64 + x86_64)
# ---------------------------------------------------------------------------
.PHONY: macos-universal
macos-universal:
	rustup target add x86_64-apple-darwin 2>/dev/null || true
	cargo build --release -p conch_app
	cargo build --release -p conch_app --target x86_64-apple-darwin
	@mkdir -p "$(DIST)"
	rm -rf "$(APP)"
	mkdir -p "$(APP)/Contents/MacOS" "$(APP)/Contents/Resources"
	lipo -create \
		target/release/conch \
		target/x86_64-apple-darwin/release/conch \
		-output "$(APP)/Contents/MacOS/conch"
	cp packaging/macos/Info.plist "$(APP)/Contents/"
	cp crates/conch_app/icons/conch.icns "$(APP)/Contents/Resources/"
	codesign --force --deep --sign - "$(APP)"
	hdiutil create -volname "Conch" -srcfolder "$(APP)" -ov -format UDZO \
		"$(DIST)/Conch-$(VERSION)-macos-universal.dmg"
	rm -rf "$(APP)"
	@echo "Built $(DIST)/Conch-$(VERSION)-macos-universal.dmg"

# ---------------------------------------------------------------------------
# Linux AMD64 (requires cross: cargo install cross)
# ---------------------------------------------------------------------------
.PHONY: linux-amd64
linux-amd64:
	cross build --release -p conch_app --target x86_64-unknown-linux-gnu
	@mkdir -p "$(DIST)"
	cargo deb -p conch_app --no-build --target x86_64-unknown-linux-gnu
	cargo generate-rpm -p crates/conch_app --target x86_64-unknown-linux-gnu
	cp target/x86_64-unknown-linux-gnu/debian/*.deb "$(DIST)/"
	cp target/x86_64-unknown-linux-gnu/generate-rpm/*.rpm "$(DIST)/"
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
	cp target/aarch64-unknown-linux-gnu/debian/*.deb "$(DIST)/"
	cp target/aarch64-unknown-linux-gnu/generate-rpm/*.rpm "$(DIST)/"
	@echo "Built Linux ARM64 packages in $(DIST)/"

# ---------------------------------------------------------------------------
# Windows x86_64 (requires cross: cargo install cross)
# ---------------------------------------------------------------------------
.PHONY: windows
windows:
	cross build --release -p conch_app --target x86_64-pc-windows-msvc
	@mkdir -p "$(DIST)"
	cp target/x86_64-pc-windows-msvc/release/conch.exe "$(DIST)/Conch-$(VERSION)-windows-x86_64.exe"
	@echo "Built $(DIST)/Conch-$(VERSION)-windows-x86_64.exe"

# ---------------------------------------------------------------------------
# All
# ---------------------------------------------------------------------------
.PHONY: all
all: macos-arm64 macos-x86 linux-amd64 linux-arm64 windows

# ---------------------------------------------------------------------------
# Clean
# ---------------------------------------------------------------------------
.PHONY: clean
clean:
	rm -rf "$(APP)" "$(DIST)"
	cargo clean
