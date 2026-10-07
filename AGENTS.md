# AGENTS.md

turnray is a macOS menu bar app that shows the status of AI agents running under herdr.
See README.md for what it does from a user's side; this file is what you need to change it.

## Toolchain

- Rust and prek (the pre-commit runner) are pinned in `mise.toml`
- `resvg` is needed only to re-render icons (`brew install resvg`)

## Commands

```sh
make test     # cargo check --all-targets / test / clippy / fmt --check — run before every commit
make icons    # re-render the menu bar PNGs and AppIcon.icns from crates/tray-app/assets/*.svg
make app      # universal, ad-hoc signed target/turnray.app + target/turnray-<version>.zip
cargo test -p tray-app <name>   # one test, or every test whose name contains <name>
RUST_LOG=debug ./target/release/turnray   # logs go to stderr
```

The pre-commit hooks (`.pre-commit-config.yaml`) run the same checks as `make test`.

## Layout

A Cargo workspace with two crates:

- `crates/agent-core` — logic with no GUI dependency: `Status` and `Agent` (`model.rs`),
  priority order (`priority.rs`), settings parsing (`settings.rs`), herdr's JSON
  (`herdr.rs`) and the `DataSource` trait (`source.rs`)
- `crates/tray-app` — the macOS app:
  - `ui.rs` works out what to show (`TrayView`, `MenuEntry`) from the state alone, so it is
    tested without a menu bar. Put display decisions here, not in `menu.rs`
  - `menu.rs` applies that to the tray item, sending only what changed
  - `images.rs` decodes the embedded PNGs and composes the Full style's image at runtime
  - `herdr.rs` finds and runs the `herdr` executable (login-shell fallback); `process.rs`
    runs a child with a deadline, a cap on its output and a process-group kill
  - `backend.rs` polls the source every second; `store.rs` saves the settings to
    `~/Library/Application Support/turnray/settings`

## Icons

- The menu bar images are `crates/tray-app/assets/*.svg`, rendered to the
  `tray-*-36.png` files next to them, which are committed and embedded with
  `include_bytes!`. After editing an SVG, run `make icons` and commit both
- Adding a status icon means adding its name to `STATUSES` in the `Makefile`
- The tray shows them as template images: macOS keeps only the alpha channel and tints
  it to the menu bar's text colour. Draw with `currentColor`; any other colour is lost,
  and partial alpha shows as a lighter shade
- Each image is 36px tall (18pt at 2x). Status SVGs use a 22×22 viewBox; digits 14×36
- `app-icon.svg` is the app icon: `make icons` turns it into the committed `AppIcon.icns`,
  which `make app` puts in the bundle

## Tests

- Unit tests live in a `#[cfg(test)] mod tests` at the bottom of each file
- herdr is never called from tests: `tray-app/src/herdr.rs` runs `fetch` against a fake
  `herdr` shell script written to a temp dir

## Commits

- Conventional Commits prefixes, as in the history: `feat:`, `fix:`, `test:`, `docs:`,
  `chore:`
