# Fireface UC, UC_WIN_RME: open questions

Working notes for the PC personality matched as `FFUC_PRODUCT_UC_WIN_RME`
(firmware 138 on the test unit).  Not part of the specification.

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
