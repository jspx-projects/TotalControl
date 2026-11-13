# Fireface UC, UC_WIN_RME: open questions

Working notes for the PC personality matched as `FFUC_PRODUCT_UC_WIN_RME`
(firmware 138 and 133 on the test units).  Not part of the specification.

## Sample format by firmware

Firmware 138 carries S32 sample words in both directions, firmware 133
carries float32.  Firmware 134 to 137 has not been measured; the driver
treats it as float32.  A capture from any of those versions settles where
the format changes.

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
