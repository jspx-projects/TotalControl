# Fireface UC gated controls — audit resolved

Working note, not part of the specification.

The gate is capability-based: `needs_protocol_features` in
`totalcontrol-gui/src/app.rs` lists the protocol-feature messages, and
`protocol_feature_allowed` excepts the ones the backend's capabilities
declare. The UC profile (`profiles/uc.rs`) declares what the UC backend
implements; the decisions below landed with #190 and the capability
model.

## Decisions

| Message | Decision | Basis |
|---|---|---|
| `LevelChanged` | keep gated | no UC software reference-level switch; §5 references are read-modify-write register fields |
| `Autoset` | keep gated | no decoded AutoSet write |
| `DynamicsEnabled`, `DynamicsChanged` | keep gated | the UC has no DSP (§9) |
| `SpeakerB`, `SpeakerBLinked` | un-gated | host-side monitor operation, implemented (`caps.speaker_b`) |
| `TalkbackChanged`, `TalkbackReceiveChanged` | un-gated | host-side talkback, implemented (`caps.talkback`, output `TalkbackReceive`) |
| `ReverbChanged`, `EchoChanged`, `FxReturnChanged`, `FxSendChanged` | keep gated | no UC FX DSP (§9) |
| `PhaseChanged` | un-gated | matrix sign write (`caps.phase`) |
| `OutputPhaseChanged` | un-gated | per-side output phase words, measured (`caps.output_phase`) |
| `PitchChanged` | un-gated | `0x1b` DDS mapping; works on UC and UCX (`caps.pitch`) |
| `WidthChanged` | un-gated | UC pair-matrix rewrite, unit-tested (`caps.width`) |
| `ChannelWidthChanged` | un-gated | per-channel width on linked pairs (`caps.channel_width`) |
| `MsProcChanged`, `InputMsProcChanged`, `PlaybackMsProcChanged` | un-gated | UC MS matrix rewrite, unit-tested (`caps.ms_proc`) |
| `An12Changed` | unsupported | no AN 1>2 link on the UC (`caps.an12` false) |
| `InputLinkChanged` | unsupported | global input link implemented by no backend (`caps.input_link` false); the per-pair stereo link remains |
| `EqForRecordChanged` | keep gated | no UC EQ |
| `DimToggle`, `Dim` | un-gated | host-side monitor dim with saved levels (`caps.dim`) |
| `EqEnabled`, `EqBand*`, `EqLowCut*` | keep gated | no UC EQ |

The former reverse checks (`Dim`, `SpeakerBLinked`, `TalkbackReceiveChanged`
reaching UC-rejecting setters) are resolved: the setters are implemented.

## Remaining

The open GUI work lives in `UC-OPEN-WORK.md` (channel controls, settings
card). The `0x17` index `0x1000` evidence question is the only gate-adjacent
item still open.