# Lab helpers

Source copies of the root-owned bench helpers installed in `/usr/local/sbin`.
They run password-free through scoped `NOPASSWD` sudoers rules. See
`docs/DEVELOPMENT-TOOLS.md` ("Lab helpers") for usage and the recovery
sequence.

| Script | Installed as | Purpose |
|---|---|---|
| `reset-fireface-usb` | `/usr/local/sbin/reset-fireface-usb` | USB port reset without unplugging; waits for the card to reappear. |
| `reload-fireface-uc` | `/usr/local/sbin/reload-fireface-uc` | Discover the newest module build, unbind, `rmmod` + `insmod`. |
| `ffuc-lab` | `/usr/local/sbin/ffuc-lab` | Driver debugfs surface: read-only `stats`/`ringtrace`, meter probes `pause`/`double`/`kill`. |

## Install

Install root-owned and not group/world-writable, then add the sudoers rules:

```sh
sudo install -m 0755 -o root -g root reset-fireface-usb reload-fireface-uc ffuc-lab /usr/local/sbin/
sudo visudo -f /etc/sudoers.d/ffuc-lab-helpers
```

with:

```
<user> ALL=(root) NOPASSWD: /usr/local/sbin/reset-fireface-usb, \
    /usr/local/sbin/reload-fireface-uc, /usr/local/sbin/ffuc-lab
```

The raw bench commands used by `scripts/bench-reload.sh` are covered by a
separate `/etc/sudoers.d/ffuc-bench` rule (documented in
`docs/DEVELOPMENT-TOOLS.md`); its `insmod` line is pinned to the absolute
checkout module path. Substitute `<user>` and the checkout path locally;
do not commit personal paths or device serials.