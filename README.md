# turnray

A tray app that shows, in the macOS menu bar, the status of the AI agents running under
herdr — so you can tell whose turn it is without
changing your terminal layout. The name is *turn* (your turn) + *tray*.

- Each agent is in one of five statuses: **blocked**, **done**, **idle**, **working**, **unknown**.
  The menu bar shows them as badges (a speech bubble with a bang, a check, a clock, a
  sparkle, a question mark) in front of the app's mark
  and each number is the count of the status right next to it
- Two styles, switched from the menu:
  - **Simple**: the most urgent status's icon and its count only
  - **Full**: every status that has an agent, each as icon + count, in priority order
- Click for the list of every agent (status, agent, workspace, title), grouped by status in
  priority order. The list is display-only
- The priority order (default: blocked → done → idle → working → unknown) can be changed from
  the menu, and is shared by the menu bar and the list, so the icon on show is always the
  head of the list
- Polls `herdr agent list` every second. If herdr can't be reached, a warning triangle is
  shown and the menu says why
- With no agents, the app's mark alone is shown and the menu says "No agents"

## Requirements

- macOS
- herdr, running. turnray finds `herdr` in `/opt/homebrew/bin`, `/usr/local/bin`,
  `/opt/local/bin`, `/usr/bin`, `~/.local/bin` and `~/.cargo/bin`, then falls back to
  `command -v herdr` in a zsh login shell (`zsh -l`: `.zprofile` is read, `.zshrc` is not)

## Install

```sh
brew install --cask muleyuck/tap/turnray
```

Or download `turnray-<version>.zip` from
[Releases](https://github.com/muleyuck/turnray/releases), unzip it, move `turnray.app` to
`/Applications`, and clear the quarantine flag (the app is ad-hoc signed, not notarized,
so Gatekeeper blocks it otherwise):

```sh
xattr -dr com.apple.quarantine /Applications/turnray.app
```

To start turnray at login, add it in System Settings > General > Login Items.

## Build

```sh
cargo build --release
./target/release/turnray
```

## Quitting

Quit is in the menu.

## Settings

The style and priority order are saved to `~/Library/Application Support/turnray/settings`.

## Development

```sh
make test   # cargo check / test / clippy / fmt
make icons  # re-render assets/*.svg to the embedded PNGs (needs: brew install resvg)
```

Logs go to stderr when launched from a terminal:

```sh
RUST_LOG=debug ./target/release/turnray
```
