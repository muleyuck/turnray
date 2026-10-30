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
  (`herdr.rs`), Claude Code hook events and session records (`hook.rs`) and the
  `DataSource` trait (`source.rs`)
- `crates/tray-app` — the macOS app:
  - `ui.rs` works out what to show (`TrayView`, `PanelContent`, `SettingsView`) from the
    state alone, so it is tested without a menu bar or a panel. Put display decisions here,
    not in `menu.rs` or `panel.rs`
  - `menu.rs` holds the app's state (`App`): applies it to the tray item and the panel,
    sending only what changed, and applies the settings view's changes
  - `panel.rs` lays out the panel (a borderless `NSPanel` hung under the tray icon, AppKit
    through objc2): the list from `PanelContent` and the settings view from `SettingsView`,
    switched by ⚙, and opens and closes it
  - `images.rs` decodes the embedded PNGs and composes the Full style's image at runtime
  - `herdr.rs` finds and runs the `herdr` executable (login-shell fallback); `process.rs`
    runs a child with a deadline, a cap on its output and a process-group kill
  - `backend.rs` polls the source every second; `store.rs` saves the settings to
    `~/Library/Application Support/turnray/settings`
  - `hooks.rs` runs `turnray hook claude` (writes a session file) and reads those files back
    as a data source, dropping stale sessions; `merged.rs` merges that source with herdr's
  - `login.rs` turns launch at login on and off through `SMAppService` (macOS 13+, and
    only from the `.app`); macOS keeps that setting, not `store.rs`

- `plugin/` is the Claude Code plugin (`.claude-plugin/plugin.json`, `hooks/hooks.json`,
  `scripts/turnray-hook`, which finds `turnray.app` and runs `turnray hook claude`);
  `.claude-plugin/marketplace.json` publishes it as `turnray@turnray`

Data sources: the agent's own hook comes first. Add no per-terminal or per-multiplexer
source besides `HerdrSource`.

The arguments and stdin of `turnray hook claude` are a compatibility promise with the
plugin. A breaking change raises the value in `Contents/Resources/hook-protocol` (set in
the `Makefile`) and updates `plugin/scripts/turnray-hook` to match.

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

- Hook tests use a temp dir and a fake `.app`, never the real `claude` or `~/.claude`.
  Tests that run the real `ps` fail inside a sandbox, so run `make test` outside one

## Commits

- Conventional Commits prefixes, as in the history: `feat:`, `fix:`, `test:`, `docs:`,
  `chore:`
