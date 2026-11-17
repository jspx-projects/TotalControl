# Digiface USB sources and licence

Working note, not part of the specification.

## 1. Vendor documentation

| Source | Use |
|---|---|
| RME, *Digiface USB User's Guide*, 2026, <https://rme-audio.de/downloads/dface_usb_e.pdf> | Modes, clock, optical format, TotalMix, Linux answer, no FX |
| RME, *Fireface UCX II User's Guide*, 2026 | TotalMix behaviour shared across models, where the Digiface manual is brief |
| RME, *TotalMix FX* chapters in the Digiface manual | Groups, snapshots, loopback, control room |

Relevant Digiface sections:

- 4, brief description: channel counts, TotalMix, no FX;
- 7.1, settings: optical output format, clock, sync status;
- 8.5–8.6, clock and synchronization;
- 19, TotalMix: routing, snapshots, groups;
- 21.6, loopback;
- 26, Linux FAQ: CC mode, clock source through the ALSA mixer;
- 28, technical specifications: clock sources.

The PDFs are not committed. `docs/rme/` is local only.

## 2. Mainline Linux

| Source | Use |
|---|---|
| `sound/usb/quirks-table.h` | Product IDs, interface 0 match, audio format sets, channel counts |
| `sound/usb/quirks.c` | `snd_usb_rme_digiface_boot_quirk`, fixed boot state |
| `sound/usb/card.c` | Class-driver binding and rawmidi rules |
| `sound/usb/pcm.c` | Rate and stream behaviour, background only |

Tree: <https://git.kernel.org/pub/scm/linux/kernel/git/torvalds/linux.git/tree/sound/usb>

The quirk table is the authority for the two product IDs. This note does not
repeat them.

## 3. Public prior art

| Source | Licence | Use |
|---|---|---|
| Openface-Mixer, <https://github.com/thomas-nelson23/Openface-Mixer> | MIT | Digiface mixer over libusb, gain law, matrix words, meter decode, ARC module |
| rmectl, <https://github.com/hoshinolina/rmectl> | MIT | Control registers, status decode, clock, rate, output format, loopback, node model |

Both projects document the same protocol and agree on the main points. That
agreement raises confidence but is not a capture.

The two projects disagree on request 22 (loopback). The mainline boot quirk
uses a third form. The conflict is recorded in
[`DIGIFACE_USB_PROTOCOL_SPEC.md`](DIGIFACE_USB_PROTOCOL_SPEC.md) §5.5.

No code is copied into this note. The tables are written from the protocol
facts.

## 4. Licence analysis

### Repository licences

| Part | Licence |
|---|---|
| `kernel/` | GPL-2.0-only |
| `ui/totalcontrol/totalcontrol-core` | MIT |
| `ui/totalcontrol/totalcontrol-gui` | GPL-3.0-only |
| Tools, scripts and documentation | GPL-3.0-or-later |

### Prior-art licences

Openface-Mixer and rmectl are both MIT. MIT permits use, modification,
sublicensing and distribution, provided the copyright notice and permission
notice are retained.

MIT is compatible with GPL-2.0 and GPL-3.0. A combined work can be distributed
under the GPL, and the MIT notice stays on the MIT parts. MIT adds no
copyleft condition to the rest of the work.

### What this means per target

| Target | Allowed | Obligation |
|---|---|---|
| Kernel module, GPL-2.0-only | Yes | Keep both MIT notices in adapted files and in `THIRD-PARTY.md` |
| Core backend, MIT | Yes | Keep both MIT notices |
| GUI, GPL-3.0-only | Yes | Keep both MIT notices |
| Documentation, GPL-3.0-or-later | Yes | Keep the MIT notice for transcribed tables or text |

No licence change is needed. Do not add an MIT licence to the repository as a
whole.

### Recommended practice

1. Reimplement from the protocol tables. Facts are not copyrightable, and an
   independent implementation is the cleanest position.
2. If code is adapted, keep the upstream copyright line and the MIT text.
3. Add one `THIRD-PARTY.md` entry per project, with URL, licence, copyright
   line, upstream commit hash and date.
4. Put the GPL SPDX identifier on the new kernel file, plus a line naming the
   MIT portions.
5. Do not copy vendor manual text, screenshots or PDFs into the repository.
6. Do not copy from sources without a licence. Use them as factual leads only.

### `THIRD-PARTY.md` entries to add when code lands

```markdown
## Openface-Mixer — RME Digiface and ARC USB protocol reference

<https://github.com/thomas-nelson23/Openface-Mixer>

MIT, Copyright (c) 2026 The Openface Mixer contributors.  The Digiface USB
mixer backend and the ARC USB codec follow this project's protocol notes.
The upstream commit is <hash>.
```

```markdown
## rmectl — RME Digiface mixer protocol reference

<https://github.com/hoshinolina/rmectl>

MIT, Copyright (c) rmectl contributors.  The Digiface USB control register,
status and node model follow this project's protocol notes.  The upstream
commit is <hash>.
```

Append the full MIT text below each entry, as `THIRD-PARTY.md` already does for
its other MIT sources.

## 5. MIT licence text, Openface-Mixer

```text
MIT License

Copyright (c) 2026 The Openface Mixer contributors

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

## 6. MIT licence text, rmectl

```text
MIT License

Copyright (c) rmectl contributors

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

## 7. Open source questions

| Status | Item |
|---|---|
| open | Pin both upstream commit hashes before adapting code |
| open | Decide whether the backend is a reimplementation or a port |
| open | Confirm the UAPI header licence matches `kernel/ffuc-uapi.h` |
