# silent and file managers

silent is one resident process. Nautilus starts it through D-Bus and keeps
it while Nautilus runs; any other file manager hands it a file with
`silent show FILE` and its kin, which start it the same way when it is not
running, so every preview is as quick as Nautilus's:

| Command | What it does |
| --- | --- |
| `silent show FILE` | Shows FILE, opening the preview if it is not open |
| `silent toggle FILE` | Closes the caller's preview if it is open, else shows FILE |
| `silent follow FILE` | Shows FILE only while the caller's preview is open, as a cursor moves |
| `silent close` | Closes the caller's preview |
| `silent warm` | Starts silent ahead of the first preview |

Each prints `shown` or `closed`, the state of the caller's preview after
the call. A preview belongs to a process, the client's parent unless
`--owner PID` names another: silent stays while that process runs, and
closes its preview when it ends. Without `--focus`, a preview leaves the
keyboard where it was, so a terminal file manager keeps its keys and the
preview follows it; a click in the preview gives it the keyboard, and Esc
closes it. With `--focus` it takes the keyboard as Nautilus's preview does.
The arrows, F2 and Delete belong to Nautilus and do nothing in other
previews. Two file managers, or one beside Nautilus, share the one window:
a preview belongs to whoever asked last.

A shell command's parent is the shell, which ends at once, so a file
manager that runs commands through a shell should pass its own process as
`--owner $PPID`. Beware of managers that fork twice before the shell, as
ranger's `shell -f` does: `$PPID` is then a process that has ended.

## Nautilus

Built in: silent owns `org.gnome.NautilusPreviewer`, and Space previews.
Nothing to set up.

## yazi

The plugin in [integrations/yazi/silent.yazi](../integrations/yazi/silent.yazi/main.lua)
warms silent as yazi starts. Its key shows the hovered file, and while the
preview is open it follows yazi's cursor. The keys stay in yazi, and the
same key closes the preview.

```sh
cp -r integrations/yazi/silent.yazi ~/.config/yazi/plugins/
echo 'require("silent"):setup()' >> ~/.config/yazi/init.lua
```

and in `~/.config/yazi/keymap.toml`:

```toml
[mgr]
prepend_keymap = [
	{ on = "i", run = "plugin silent", desc = "Preview in silent, following the cursor" },
]
```

`i` is free in yazi's default keymap; Space selects there. The plugin runs
`silent` from PATH, else `~/.local/bin/silent`; `setup({ bin = "/path/to/silent" })`
names another. Tested with yazi 26.9.1.

## Nemo

Nemo calls nemo-preview as `org.nemo.Preview` (ShowFile with the file, the
window as a number and whether to close a preview of the same file, and
Close), and silent answers that interface too. For Nemo to start silent,
put [data/org.nemo.Preview.service](../data/org.nemo.Preview.service) in
`~/.local/share/dbus-1/services/`, with silent's path on its `Exec` line,
in place of nemo-preview's, which owns the same name. Space then previews.
Tested with Nemo's calls made by `gdbus`; not with Nemo itself.

## Thunar

Edit, Configure custom actions, add one with the command
`silent show --focus %f` for every file type, and give it a key in
Thunar's keyboard shortcuts. Not tested.

## PCManFM and PCManFM-Qt

Both read file-manager actions; save this as
`~/.local/share/file-manager/actions/silent.desktop`:

```ini
[Desktop Entry]
Type=Action
Name=Preview in silent
Profiles=all;

[X-Action-Profile all]
MimeTypes=all/all;
Exec=silent show --focus %f
```

The action is in the context menu. Not tested.

## Dolphin

A service menu, `~/.local/share/kio/servicemenus/silent.desktop`, made
executable (`chmod +x`), as KDE Frameworks 6 requires:

```ini
[Desktop Entry]
Type=Service
MimeType=all/all;
Actions=preview

[Desktop Action preview]
Name=Preview in silent
Icon=document-preview
Exec=silent show --focus %f
```

The action is in the context menu; Dolphin does not bind keys to service
menus. Not tested.

## ranger

In `~/.config/ranger/rc.conf`:

```
map i shell -s silent show --focus --owner $PPID %f
```

`-s` runs the command from ranger itself with its output dropped, so
`$PPID` is ranger; `-f` would fork twice and lose it. Tested with ranger
1.9.4 (both the preview and silent leaving when ranger quits).

## lf

lf's `on-select` hook lets the preview follow the cursor, as yazi's
plugin does. In `~/.config/lf/lfrc`:

```
map i $silent toggle --owner $PPID "$f" >/dev/null
cmd on-select &silent follow --owner $PPID "$f" >/dev/null
```

Not tested.

## nnn

nnn's plugin keys run a command with the hovered file as `$nnn`:

```sh
export NNN_PLUG='i:!silent show --focus --owner $PPID "$nnn"*'
```

`*` skips nnn's prompt after the command. Not tested.
