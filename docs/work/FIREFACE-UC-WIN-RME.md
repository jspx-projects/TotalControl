# Fireface UC, UC_WIN_RME: open questions

Working notes for the PC personality matched as `FFUC_PRODUCT_UC_WIN_RME`
(firmware 133 and 138 on the test units).  Not part of the specification.

## Sample format and hardware revision

The RME Flash Update Tool maps the two UC hardware revisions to firmware
127 (Hw Rev 3) and 138 (Hw Rev 6).  Firmware 133 on `UC_WIN_RME` carries
float32; firmware 138 carries S32.  Firmware 134 to 137 is unmeasured and
treated as float32.

The `UC_WIN_RME` descriptor therefore carries the S32 format from firmware
138 upward and float32 below it.  Other UC descriptors keep their declared
float32 format; the firmware revision does not override them.

## Period signalling on the isochronous transport

PipeWire drops out at quanta of 512 and above when capture drives the graph
and playback follows on the same card: it reads the follower with
`snd_pcm_avail_update()`, which returns the follower's hw_ptr as of its last
period, and the two directions' periods end within one URB of each other.
The interrupt transport now updates the other direction's hw_ptr before it
signals a period (`ffuc_period_elapsed()`).  The isochronous transport
(UCX, AP personalities) still signals only its own direction; whether it shows
the same dropouts has not been tested.

## Endpoints that differ from the original UC

The descriptor is copied from the original UC.  On the test unit interface 1
has only bulk IN `0x85`; there is no `0x0c` OUT or `0x8d` IN.  Interface 3
has bulk `0x88` IN and `0x09` OUT, interface 4 bulk `0x8a` IN.  Audio,
status (`0x8b`) and EP0 control work; the state and meter paths are
untested.

## AP personality

The unit's AP personality enumerates under a product ID the driver does not
match.  Its interface 0 is isochronous, with alternate settings 1–3.  Not
supported yet; a capture is needed.
