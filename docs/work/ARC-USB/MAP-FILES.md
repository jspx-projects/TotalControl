# Map files

Working note. This is the short answer to "where do the map files live and how
does that normally work on Linux". The normative version is
[`CONTROLLER_MODEL.md`](CONTROLLER_MODEL.md) §5–6.

## Shipped files

| Path | Meaning |
|---|---|
| `/usr/share/totalcontrol/midi-controllers/*.json` | Files installed by a distribution package or a `/usr` prefix build. |
| `/usr/local/share/totalcontrol/midi-controllers/*.json` | Files installed by a default source build. |

The directory comes from the build's `datadir`. The program compiles it in, so
it finds its files wherever the prefix put them.

This is the standard Linux layout for read-only program data. Examples in the
same place on a normal system: `/usr/share/alsa`, `/usr/share/pipewire`,
`/usr/share/applications`, `/usr/share/icons`.

## User files

| Path | Meaning |
|---|---|
| `$XDG_DATA_HOME/totalcontrol/midi-controllers/*.json` | A user copy, which overrides a shipped file of the same name. |
| `~/.local/share/totalcontrol/midi-controllers/*.json` | The same, with `XDG_DATA_HOME` unset. |

This is the XDG Base Directory user data location. TotalControl already uses
`~/.local/share/totalcontrol` for its workspaces.

## What is not used

| Path | Why not |
|---|---|
| `/etc/totalcontrol` | `/etc` holds administrator configuration, not shipped data. |
| `~/.config/totalcontrol` | Settings the program writes live there. The maps are data, and they are edited copies of shipped files. |
| Next to the binary | Breaks when the binary is moved or installed into a read-only prefix. |

## How a file is found

1. `TOTALCONTROL_CONTROLLERS`, if set, for tests and development.
2. The user directory.
3. The installed data directory.
4. The definitions embedded in the binary, for a relocated binary without a
   data directory.

First match by file name wins. A malformed user file is reported and skipped.

## How to install by hand

```sh
mkdir -p ~/.local/share/totalcontrol/midi-controllers
cp RME_ARC_USB_MIDI.json ~/.local/share/totalcontrol/midi-controllers/
```

Edit the copy with a text editor, then restart TotalControl. The settings line
shows which file is in use.

## What the files must not contain

| Item | Reason |
|---|---|
| Local paths, host names, user names | Personal identifying information. |
| Serial numbers | Personal identifying information. |
| Captured device state | Not a protocol definition. |
| A channel number chosen by the user | Settings, not definition. |

The file name and the `id` are stable. `version` changes when the content
changes.

## Assignments

Key and button assignments are host settings, not map file content
(`CONTROLLER_MODEL.md` §10). A user map file overrides the protocol; the
host settings override the assignment.
