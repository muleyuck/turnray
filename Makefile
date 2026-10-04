.PHONY: test
test:
	cargo check --all-targets
	cargo test --verbose
	cargo clippy --all-targets --all-features
	cargo fmt --all --check

ASSETS := crates/tray-app/assets
STATUSES := blocked done idle working unknown standby error
DIGITS := 0 1 2 3 4 5 6 7 8 9
ICONSET := target/AppIcon.iconset

# Rasterises $(ASSETS)/*.svg into the PNGs the app embeds, and app-icon.svg into
# AppIcon.icns for the .app bundle. The PNGs are 36px tall because tray-icon scales the
# image to 18pt, so 36px is the Retina (2x) size.
# Needs resvg (brew install resvg).
.PHONY: icons
icons:
	for s in $(STATUSES); do resvg -w 36 -h 36 $(ASSETS)/$$s.svg $(ASSETS)/tray-$$s-36.png || exit 1; done
	for d in $(DIGITS); do resvg -w 14 -h 36 $(ASSETS)/digit-$$d.svg $(ASSETS)/tray-digit-$$d-36.png || exit 1; done
	rm -rf $(ICONSET) && mkdir -p $(ICONSET)
	for s in 16 32 128 256 512; do \
		resvg -w $$s -h $$s $(ASSETS)/app-icon.svg $(ICONSET)/icon_$${s}x$${s}.png || exit 1; \
		resvg -w $$((s * 2)) -h $$((s * 2)) $(ASSETS)/app-icon.svg $(ICONSET)/icon_$${s}x$${s}@2x.png || exit 1; \
	done
	iconutil -c icns $(ICONSET) -o $(ASSETS)/AppIcon.icns

VERSION := $(shell sed -n 's/^version = "\(.*\)"$$/\1/p' crates/tray-app/Cargo.toml)
APP := target/turnray.app
BIN := $(APP)/Contents/MacOS/turnray

# Builds a universal, ad-hoc signed target/turnray.app and zips it for release.
.PHONY: app
app:
	MACOSX_DEPLOYMENT_TARGET=11.0 cargo build --release --target aarch64-apple-darwin
	MACOSX_DEPLOYMENT_TARGET=11.0 cargo build --release --target x86_64-apple-darwin
	rm -rf $(APP) && mkdir -p $(APP)/Contents/MacOS $(APP)/Contents/Resources
	lipo -create -output $(BIN) \
		target/aarch64-apple-darwin/release/turnray target/x86_64-apple-darwin/release/turnray
	sed 's/@VERSION@/$(VERSION)/g' packaging/Info.plist.in > $(APP)/Contents/Info.plist
	cp $(ASSETS)/AppIcon.icns $(APP)/Contents/Resources/AppIcon.icns
	codesign --force -s - $(APP)
	lipo -archs $(BIN) | grep -qw arm64
	lipo -archs $(BIN) | grep -qw x86_64
	codesign --verify --strict $(APP)
	plutil -lint $(APP)/Contents/Info.plist
	rm -f target/turnray-$(VERSION).zip
	ditto -c -k --keepParent $(APP) target/turnray-$(VERSION).zip
