# State, workspace and persistence

This specification defines where TotalControl keeps its state, which file is
authoritative, how the state is written and restored, and the boundary
between workspace state, host settings and the GUI layout document.

Visible behaviour is in
[`TOTALCONTROL-GUI-SPEC.md`](TOTALCONTROL-GUI-SPEC.md); the mixer model is in
[`../TOTALCONTROL-ARCHITECTURE.md`](../TOTALCONTROL-ARCHITECTURE.md) §9.

## 1. Terms

| Term | Meaning |
|---|---|
| Current workspace | The complete live workspace for one unit: mixer scene, GUI state, snapshots, layouts, groups and masters. It is the single source of truth for workspace state. |
| Explicit workspace | A `.tcws` file the user saves or loads. It has the same schema as the current workspace. |
| Host settings | Installation and per-unit operational settings that are not workspace state. |
| GUI layout | The presentation definition: geometry, colours and strip templates, with a global default and optional per-device overrides. |
| Device state | The unit's own mixer/setup state and its six device setups. It is not a host workspace file. |
| Scene | The serialised mixer model (`inputs`, `playbacks`, `outputs`, `settings`, matrix overrides). |

## 2. Single source of truth

The current workspace is the only source of workspace state. It holds the
mixer scene and every durable GUI field. The host settings file and the GUI
layout document do not duplicate workspace state.

The former `auto.json`, `gui.json`, `Mix <n>.json` and `Layout <n>.json`
files are migration inputs only. They are not read or written once the
current workspace exists.

An explicit workspace is a saved copy of the current workspace. Saving does
not change which state is live; loading replaces the current workspace with
the explicit file.

## 3. Current workspace file

There is one current workspace per unit:

`<data>/workspaces/current-<unit-key>.tcws`

`unit-key` is derived from the unit identity (canonical model and serial when
available). The active `unit-key` is recorded in the host settings.

The file uses the `.tcws` workspace schema. It contains:

| Field group | Contents |
|---|---|
| Scene | The current mixer model and device settings |
| Snapshots | The eight Mix slots and their names |
| Groups | The four mute/solo/fader groups |
| Layout slots | The six layout presets and the active slot |
| Labels | User channel names and "show original names" |
| Masters | The mute and solo master sets, including suspended state |
| Temporary group | The `F` membership and its ganged flag |
| View | View, row mode, sidebar, panels, FX panel, zoom, quick channel, matrix mode |
| Window | Size, position, fullscreen/maximised state and scroll positions |
| Panels | Open channel panels/flyouts and collapsed strips |
| Selection | Selected output and split side, focused strip and temporary selection |
| Monitoring | Control Room settings and assignments |
| Surround | Virtual-surround state |
| Name | The workspace display name |

The file does not store the source workspace path. After a recovered start,
Save uses Save As until the user chooses a file.

The file does not store active Talkback or Listenback, audio meters, fader
animations, pointer hover/drag state, modal dialogs, notices or undo/redo
history.

## 4. Autosave

The application marks the current workspace dirty on every user change to
workspace state. Meter updates and other transient readouts do not dirty it.

A 60-second periodic check writes the current workspace when it is dirty.
There is no user setting for this interval; a change can wait up to 60 seconds
for the periodic write. The check runs while the application is running,
including while its window is hidden in the tray.

The write is also forced immediately on:

- quit;
- window close;
- hide to the system tray;
- device disconnect, before the device handle is released;
- device switch, for the outgoing unit;
- workspace load, for the outgoing unit and for the loaded state;
- explicit workspace Save;
- system session end when the process receives it.

A clean workspace is not rewritten. At most one write is in flight; a write
failure leaves the workspace dirty and is retried at the next interval or
event. The write is atomic and does not run on the audio path.

Mock and development sessions use a temporary workspace location and never
overwrite a real unit's current workspace.

## 5. Restore

### 5.1 Application start

The application loads the current workspace for the active unit. The scene
is applied to the device under the existing startup sync rules (readback,
mismatch prompt and "Always init DSP devices with TotalMix FX settings").
The GUI fields are restored as saved. If no unit is connected at start, the
last active unit's workspace is loaded and is applied when that unit appears;
a different unit switches to its own current workspace.

If no current workspace exists, the application builds one from the old
`auto.json`, `gui.json`, `Mix <n>.json` and `Layout <n>.json` state when
present, then writes it. If none exists, it starts from a new workspace.

If the current workspace is missing or invalid, the migration/default path is
used and the file is replaced on the next write. If the scene model does not
match the connected unit, the workspace is not applied to that unit; a new
workspace is started for the connected unit.

### 5.2 Device reconnect

A reconnect while the application is running keeps the live in-memory
workspace. The scene is reapplied to the device under the startup sync rules.
The current workspace file is not reread, so a file from an earlier save
cannot replace newer in-memory changes.

### 5.3 Device switch

The outgoing unit's workspace is saved first. The incoming unit's current
workspace is loaded if it exists; otherwise a new workspace is started for
that unit. Device-specific host settings follow the incoming unit.

### 5.4 Workspace load and Save

Loading an explicit workspace saves the outgoing current workspace, applies
the explicit scene, replaces the GUI state and writes the loaded state to the
current workspace.

Save writes the current workspace to the chosen `.tcws` file and to the
current workspace file.

## 6. Complete GUI state

The current workspace restores the durable GUI state:

- window size, position, fullscreen/maximised state;
- scroll positions;
- view, row mode, sidebar state and width;
- open sidebar panels, FX panel and channel panels/flyouts;
- collapsed strips;
- zoom/UI scale;
- selected output and split side, quick channel and matrix mono mode;
- active snapshot and active layout;
- channel labels and name display;
- all groups, master sets and the temporary fader group;
- Control Room and surround state.

The following is transient and is not restored:

- audio meters and peak holds;
- fader and panel animations;
- pointer hover, drag and pending click state;
- modal dialogs and notices;
- active Talkback and Listenback;
- the actual hidden/tray state (a normal launch opens the window; the session
  autostart entry starts hidden);
- undo and redo history.

## 7. Host settings

Host settings are stored in:

`$XDG_CONFIG_HOME/totalcontrol/host-settings.json`

They are saved atomically on change and at application exit. They contain no
mixer scene, labels, groups, snapshots, layouts or window layout.

### 7.1 Global

| Setting | Contents |
|---|---|
| Active unit | Last selected unit key |
| OSC | Enabled, dialect and ports |
| Engine | Forced rate, quantum and rate mode |
| Startup | Session autostart (starts hidden in the tray) and keep in tray |
| Files | Last workspace folder |

### 7.2 Per unit

| Setting | Contents |
|---|---|
| Current workspace | The unit's current workspace file |
| Startup sync | Sync direction and "Always init DSP devices with TotalMix FX settings" |

Host settings may carry other unit-specific operational values. They must not
duplicate the mixer scene or a GUI field listed in §6.

### 7.3 Startup visibility

A normal launch opens the window. The session autostart setting installs an
autostart entry that launches the application with `--minimized`, which starts
it hidden in the tray at login. An explicit `--minimized` also starts hidden.
With no tray the window is always shown.

The actual window-hidden state is runtime state. It is not saved in the
current workspace; the next normal launch opens the window.

## 8. GUI layout document

The layout definition is a separate file:

`$XDG_CONFIG_HOME/totalcontrol/gui-layout.json`

It defines geometry, visual tokens, colours and strip templates. It is not
workspace state and is not copied into workspace files.

The document has a global default and optional per-device overrides:

| Level | Contents |
|---|---|
| Default | The layout for any unit without an override |
| Device | An optional partial override keyed by canonical model or device family |

The effective layout is the default merged with the matching device override.
The built-in layout applies when the file is missing or invalid.

Device capabilities are authoritative. A template may contain only elements
the connection can actually use: a unit without the FX bus, EQ or Dynamics
uses a template without those elements, and a layout override cannot add them
back. Unsupported elements are not drawn and have no action. A device template may
arrange the supported elements; it cannot add an unsupported one.

The workspace stores layout presets, the active preset and the channel
visibility/collapse state. The layout document supplies the templates those
presets render with; it is not another workspace.

## 9. Migration

The migration is performed when the current workspace does not yet exist:

1. Read the old auto scene, GUI preferences, snapshot slots and layout slots.
2. Build the current workspace for the active unit.
3. Write the current workspace and host settings.
4. Keep the old files untouched as a fallback for that release.

After migration the application reads and writes only the current workspace,
the host settings file and the optional GUI layout document. The old files
remain on disk as backups and are not a source. The TUI reads the mixer scene
from the current workspace; it has no separate auto scene.

Explicit workspaces remain as user-saved copies.

## 10. File summary

| File | Role | Required |
|---|---|---|
| `<data>/workspaces/current-<unit-key>.tcws` | Live workspace for one unit; autosaved | yes |
| `$XDG_CONFIG_HOME/totalcontrol/host-settings.json` | Global and per-unit operational settings | yes |
| `$XDG_CONFIG_HOME/totalcontrol/gui-layout.json` | Presentation/layout definitions | no |
| User `.tcws` files | Explicit workspace saves | no |

The unit's six device setups and its own mixer state remain device state.
