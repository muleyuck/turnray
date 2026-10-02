.PHONY: test
test:
	cargo check --all-targets
	cargo test --verbose
	cargo clippy --all-targets --all-features
	cargo fmt --all --check

ASSETS := crates/tray-app/assets
STATUSES := blocked done idle working unknown error
DIGITS := 0 1 2 3 4 5 6 7 8 9

# Rasterises $(ASSETS)/*.svg into the PNGs the app embeds. 36px tall because tray-icon
# scales the image to 18pt, so 36px is the Retina (2x) size.
# Needs resvg (brew install resvg).
.PHONY: icons
icons:
	for s in $(STATUSES); do resvg -w 36 -h 36 $(ASSETS)/$$s.svg $(ASSETS)/tray-$$s-36.png || exit 1; done
	for d in $(DIGITS); do resvg -w 14 -h 36 $(ASSETS)/digit-$$d.svg $(ASSETS)/tray-digit-$$d-36.png || exit 1; done
