# Fireface UC, UC_WIN_RME: open questions

Working notes for the PC personality matched as `FFUC_PRODUCT_UC_WIN_RME`
(firmware 138 on the test unit).  Not part of the specification.

## Period signalling on the isochronous transport

PipeWire drops out at quanta of 512 and above when capture drives the graph
and playback follows on the same card: it reads the follower with
`snd_pcm_avail_update()`, which returns the follower's hw_ptr as of its last
period, and the two directions' periods end within one URB of each other.
While capture runs, the interrupt transport signals both directions together
at the capture period boundary, playback first, and playback's own boundary
signals nothing (`ffuc_period_elapsed()`); playback signals itself while
capture is stopped.  Signalling both directions at both boundaries also
cures the dropouts, but on a firmware 133 unit at 48 kHz (48-frame URBs, so
one URB per period at a quantum of 48) it lost whole periods at a quantum of
48: 3 silent periods in the output loopback over four 30 s runs, against none
with the capture boundary signalling both.  The isochronous transport (UCX,
AP personalities) still signals only its own direction; whether it shows the
same dropouts has not been tested.

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
