# rex-kiosk

Fuzzy-find Git repositories (or any folder) and branches, then open them in
[Rex](https://www.superlogical.com/rex/). Each repository gets one Rex session with a
tab for each branch you open, and a branch without a checkout gets a Git worktree. The
picker floats over your current tab and closes once something opens.

## Install

rex-kiosk is a single binary. Build and install it with Cargo:

```sh
cargo install --locked --git https://github.com/thomasschafer/rex-kiosk
```

It needs `git` on the `PATH`. Install it on the machine that runs your Rex server: the
picker runs in a Rex terminal, which runs there.

## Set up in Rex

rex-kiosk opens from a Rex custom action, defined in [`rex/kiosk.lua`](rex/kiosk.lua).
Put that file beside your `init.lua`:

```sh
curl -fsSL https://raw.githubusercontent.com/thomasschafer/rex-kiosk/main/rex/kiosk.lua \
  -o ~/.config/rex/kiosk.lua
```

Then load it in `~/.config/rex/init.lua` and bind a key to the `kiosk` action (here
`cmd+p`, which replaces any other binding of that key):

```lua
require("kiosk").setup()
rex.bind("cmd+p", "kiosk")
```

and reload the configuration:

```sh
rex config reload
```

The action is also in the command palette as "Open Repository or Branch…". The pane
runs `rex-kiosk` through your login shell, so it finds it on the `PATH` your shell's
startup files set. If it isn't there, give its path:

```lua
require("kiosk").setup{ command = { "/path/to/rex-kiosk" } }
```

`setup` also takes `bounds`, where the pane floats as `{ x, y, w, h }` fractions of the
window, which defaults to `{ 0.1, 0.1, 0.8, 0.8 }`.

### Turn on Remote Control

rex-kiosk switches the app to what it opens, which Rex only allows when Remote Control
is on: open Rex's Settings, and under Rex Server turn on Remote Control. Without it,
kiosk still opens the session and tab, but the app stays where it was, and the picker
says how to turn it on.

On first use, a setup wizard asks which directories to search and writes
`~/.config/rex-kiosk/config.toml`.

## Usage

Type to fuzzy-search repositories and branches (and plain folders, if you enable
`include_non_git`). `enter` opens the selected checkout or folder, `tab` opens a
repository's branch view, `ctrl+o` there creates a branch, `ctrl+x` deletes a branch's
checkout, and `ctrl+h` shows all active bindings for the current view.

Opening a repository or branch switches to the tab already showing it, if there is one.
Otherwise kiosk adds a tab to the repository's session, or creates the session with that
tab in it. Sessions are named after the repository and tabs after the branch, and
`[on_open]` panes are split into each new tab with their commands. A plain folder gets a
session of its own with one tab.

Sessions and tabs only have names, which you can change, so kiosk goes by the folders
your terminals are in instead:

- A tab shows a checkout when its first terminal is inside that checkout. kiosk looks at
  where each terminal's foreground program is, so it follows `cd`, and falls back to the
  folder the shell last reported. Floating panes don't count.
- A session belongs to the repository its first such tab is in. New tabs for a
  repository go in that session, wherever its other tabs are.
- Only a session with no terminal kiosk can place falls back to its name.
- If more than one session belongs to a repository, kiosk prefers the one named after
  it, and otherwise refuses to guess and asks you to name exactly one of them so.

Deleting a checkout removes the worktree with Git, then closes its tab, and the session
too when that was its last tab. When the picker is floating over the tab being closed,
the tab's panes close at once and the tab itself closes with the picker. If that tab is
the only one in its session, which would leave the session empty, rex-kiosk closes the
whole session, picker included, and sends a notification saying what it removed. A
checkout with uncommitted changes needs a second confirmation.

If rex-kiosk itself fails, for example on a mistake in its config, its pane stays open
with the error until you press Enter.

## Configuration

<!-- CONFIG:START -->
rex-kiosk reads `$XDG_CONFIG_HOME/rex-kiosk/config.toml`, which is
`~/.config/rex-kiosk/config.toml` when `XDG_CONFIG_HOME` is unset. All
sections are optional, and the defaults are shown below alongside curated
examples for settings that benefit from one.

#### `search_dirs`

Directories searched recursively for Git repositories and, when enabled,
plain folders. Entries can be simple strings such as `"~/Code"` or inline
tables such as `{ path = "~/Work", depth = 3 }`, and both forms can be mixed.
Missing directories are reported as warnings unless marked `optional`.

Example:

```toml
include_non_git = false
search_dirs = [
  "~/Code",
  { path = "~/Work", depth = 2, include_non_git = true },
  { path = "~/Clients", optional = true },
]
```

A repository search root, written either as a path string or an inline table.

Accepted forms:

- String form.
  - Directory to scan with the default depth of 1. `~` and paths beginning with `~/` expand from the user's home directory.
- A path with optional per-directory scan depth, plain-folder override, and missing-directory tolerance.
  - `path` — Directory to scan. `~` and paths beginning with `~/` expand from the user's home directory; other relative paths are accepted as written.
  - `depth` — Maximum directory depth to scan. The value must be at least 1 and defaults to 1 when omitted.
  - `include_non_git` — Whether to include plain folders for this search root. When omitted, the global `include_non_git` value is used.
  - `optional` — Whether this directory may be absent. A missing optional directory is skipped without a warning; a path that exists but is not a readable directory still warns. Defaults to `false`.

#### `include_non_git`

Include plain folders in search results globally. Rich search-directory
entries can override this value with their own `include_non_git` setting.

Default: `false`

### `[worktrees]`

Where kiosk creates checkouts for branches that do not have one.

Example:

```toml
[worktrees]
directory = "~/Code/worktrees"
```

#### `directory`

Directory holding new checkouts, each at `<directory>/<repository>/<branch>`.
`~` and paths beginning with `~/` expand from the user's home directory; the path
must be absolute after expansion.

Default: `"~/.kiosk/worktrees"`

### `[theme]`

Customize terminal-palette colors used by the picker. Light-terminal users
can set `muted`, `border`, and other colors explicitly.

Colors use the terminal's ANSI palette and can be `black`, `red`, `green`, `yellow`, `blue`, `magenta`, `cyan`, `white`, `gray`, `dark_gray`, `reset`.

Defaults:

```toml
[theme]
accent = "magenta"
secondary = "cyan"
tertiary = "green"
error = "red"
warning = "yellow"
muted = "dark_gray"
border = "dark_gray"
hint = "blue"
highlight_fg = "black"
open = "green"
```

### `[on_open]`

Configure command panes created in each new checkout window.

The section is optional and contains no pane definitions by default.

Example:

```toml
[on_open]
panes = [
  { command = "hx", direction = "right", ratio = 0.6 },
]
```

#### `panes`

Pane definitions, each splitting the window's first pane in order. Focus
stays on the first pane. Panes are created only with a new window, not
when kiosk switches to a window that is already open, and never for plain
folders.

A command pane created in a new window.

Each entry is an inline table with:

- `command` — Shell command typed into the pane's shell as it starts, in the opened checkout. The shell stays open when the command ends. The command must not be empty.
- `direction` — Split direction: `right` or `down`.
- `ratio` — Fraction of the resulting split occupied by the new command pane. The value must be greater than 0 and less than 1, and defaults to 0.5 when omitted.

### `[keys]`

Customize key bindings grouped by where they are active.

Assign a key to `"noop"` to unbind an inherited mapping.

Write chords with lowercase `ctrl+`, `alt+`, and `shift+` modifiers followed by a character or a named key such as `enter`, `esc`, `tab`, `backspace`, `delete`, an arrow, `home`, `end`, `pageup`, `pagedown`, or `space`.

Defaults:

```toml
[keys.general]
"ctrl+c" = "quit"
"ctrl+h" = "help"
"ctrl+x" = "dismiss_toast"

[keys.text_edit]
"alt+backspace" = "delete_word"
"backspace" = "backspace"
"ctrl+w" = "delete_word"
"esc" = "clear"
"left" = "cursor_left"
"right" = "cursor_right"

[keys.list_navigation]
"ctrl+n" = "move_down"
"ctrl+p" = "move_up"
"down" = "move_down"
"up" = "move_up"

[keys.modal]
"enter" = "open"
"esc" = "back"

[keys.repo_select]
"enter" = "open"
"q" = "quit"
"tab" = "branches_view"

[keys.branch_select]
"ctrl+o" = "new_branch"
"ctrl+x" = "delete"
"enter" = "open"
"esc" = "back"

```

<!-- CONFIG:END -->

## Platforms

rex-kiosk is tested on macOS with Rex Beta 0.1.0 (build 1642), against the Rex app and
a local server. The Rex app is macOS-only today, but its server also runs on Linux and
FreeBSD, and rex-kiosk should work there too when installed on the server's machine. It
builds and passes its tests on Linux; it hasn't been tried against a Linux or FreeBSD
Rex server.

## Related projects

[kiosk](https://github.com/thomasschafer/kiosk) is the same picker for tmux, and
[herdr-kiosk](https://github.com/thomasschafer/herdr-kiosk) for herdr and the Herdr
app.

## Development

```sh
cargo build
just lint
just test
just readme
```

The configuration reference above is generated from the config types' doc comments:
`just readme` regenerates it, and `just readme-check` (run in CI) fails if it is out of
date.
