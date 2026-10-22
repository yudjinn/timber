# Opening sources in your editor

In the desktop app, **Open in editor** (source viewer, node panel) runs the **Editor command** from Settings. The command is a template:

| Placeholder | Becomes |
|---|---|
| `{file}` | absolute path of the source file |
| `{line}` | line number (1-based) |
| `{col}` | column (1-based) |

The command runs directly, **not through a shell**. Arguments are split on spaces, and `"double quotes"` keep spaces together. If you need shell features, wrap the command, e.g. `sh -c "…"`.

With no command set, Timber opens the file with your OS default application (`xdg-open`, `open`, or `start`). That opens the file but can't jump to the line.

## GUI editors

| Editor | Command |
|---|---|
| VS Code | `code -g {file}:{line}:{col}` |
| VSCodium | `codium -g {file}:{line}:{col}` |
| Cursor | `cursor -g {file}:{line}:{col}` |
| Zed | `zed {file}:{line}:{col}` |
| Sublime Text | `subl {file}:{line}:{col}` |
| Kate | `kate --line {line} --column {col} {file}` |
| gedit | `gedit +{line} {file}` |
| CLion / IntelliJ IDEA | `clion --line {line} --column {col} {file}` (or `idea …`) |
| Emacs (GUI) | `emacs +{line}:{col} {file}` |
| Emacs (running server) | `emacsclient -n +{line}:{col} {file}` |
| Notepad++ (Windows) | `"C:\Program Files\Notepad++\notepad++.exe" -n{line} -c{col} {file}` |

## Terminal editors

A terminal editor needs a terminal window to run in. Start your terminal and tell it which command to run:

| Editor | Line syntax | Example |
|---|---|---|
| Neovim / Vim | `+{line} {file}` | `kitty -e nvim +{line} {file}` |
| Helix | `{file}:{line}:{col}` | `alacritty -e hx {file}:{line}:{col}` |
| nano | `+{line},{col} {file}` | `foot nano +{line},{col} {file}` |
| micro | `{file}:{line}:{col}` | `wezterm start -- micro {file}:{line}:{col}` |
| Kakoune | `+{line}:{col} {file}` | `kitty -e kak +{line}:{col} {file}` |
| Emacs (terminal) | `+{line}:{col} {file}` | `gnome-terminal -- emacs -nw +{line}:{col} {file}` |

How common terminals take a command to run:

| Terminal | Form |
|---|---|
| kitty | `kitty -e <command…>` |
| Alacritty | `alacritty -e <command…>` |
| foot | `foot <command…>` |
| WezTerm | `wezterm start -- <command…>` |
| GNOME Terminal | `gnome-terminal -- <command…>` |
| Konsole | `konsole -e <command…>` |
| xterm | `xterm -e <command…>` |
| Windows Terminal | `wt <command…>` |
| macOS Terminal | `open -a Terminal <file>` (no line jump; use a GUI editor or iTerm's CLI) |

## Already running an editor?

Many editors can open a file in an existing window. Use their remote command so files open as tabs instead of new windows:
- VS Code reuses its window by default.
- Neovim: `nvim --server ~/.cache/nvim/server.pipe --remote {file}`, then `+{line}` from inside (or use a GUI client).
- Emacs: `emacsclient`.
- Kakoune: `kak -c <session>`.
