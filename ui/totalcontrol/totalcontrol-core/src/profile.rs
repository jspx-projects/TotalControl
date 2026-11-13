//! Declarative channel topology for a supported RME device — see
//! `crate::profiles` for the per-model instances of [`DeviceProfile`].
//!
//! The profile is shared by every backend that drives the model, so channel
//! names, counts and output display order cannot drift between the hardware
//! path, the simulation and the tests.

use crate::channel::{ChannelType, InputChannel, OutputChannel, PlaybackChannel, Sensitivity};

/// One RME model's channel topology: model name, per-input and
/// per-output-pair naming and typing, and the output display order. Each
/// supported model is one `const DeviceProfile` (see `crate::profiles`).
#[derive(Debug, Clone, Copy)]
pub struct DeviceProfile {
    pub model_name: &'static str,
    /// One entry per physical hardware input, in channel-id order.
    pub inputs: &'static [InputSpec],
    /// One entry per physical stereo output pair, in the hardware output
    /// row's order.
    pub outputs: &'static [OutputPairSpec],
    /// One entry per software-playback stereo pair, in the order the device
    /// exposes them.  Playback channel ids are `2*i` / `2*i+1`, so this list
    /// must be in device order.  The labels can differ from `outputs`: the
    /// UCX software row starts at AN1/2, puts Phones fourth, and calls the
    /// first optical pair AS 1/2 when the optical input is SPDIF.
    pub playback_pairs: &'static [OutputPairSpec],
    /// Hardware-output display order: entry `i` is the output-pair index
    /// (into `outputs`, which stays in device block order) shown at
    /// position `i` in TotalMix's bottom row.  Identity for devices whose
    /// display order matches the block order.
    pub output_order: &'static [usize],
}

#[derive(Debug, Clone, Copy)]
pub struct InputSpec {
    pub name: &'static str,
    pub channel_type: ChannelType,
}

#[derive(Debug, Clone, Copy)]
pub struct OutputPairSpec {
    pub left: &'static str,
    pub right: &'static str,
}

impl DeviceProfile {
    pub fn input_count(&self) -> usize {
        self.inputs.len()
    }

    pub fn output_pair_count(&self) -> usize {
        self.outputs.len()
    }

    /// Instrument inputs default to +4dBu sensitivity; every other
    /// input type has no sensitivity switch at all.
    pub fn build_inputs(&self) -> Vec<InputChannel> {
        self.inputs
            .iter()
            .enumerate()
            .map(|(i, spec)| {
                let mut ch =
                    InputChannel::new(i, spec.name, spec.channel_type, self.output_pair_count());
                ch.sensitivity = if spec.channel_type == ChannelType::Instrument {
                    Some(Sensitivity::Plus4dBu)
                } else {
                    None
                };
                ch
            })
            .collect()
    }

    pub fn build_playbacks(&self) -> Vec<PlaybackChannel> {
        let mut pbs = Vec::new();
        let outputs = self.output_pair_count();
        for (i, pair) in self.playback_pairs.iter().enumerate() {
            // The id is the real device playback channel, so crosspoints
            // keep addressing the right source regardless of the labels.
            let mut left = PlaybackChannel::new(i * 2, &format!("PCM {}", pair.left), outputs);
            let mut right =
                PlaybackChannel::new(i * 2 + 1, &format!("PCM {}", pair.right), outputs);
            // Factory default is straight playback: each software playback
            // pair feeds only its own hardware output pair at unity.  The
            // volume array is in internal output order, so map the display
            // pair through `output_order`.
            let target = self.output_order.get(i).copied().unwrap_or(i);
            if let Some(slot) = left.volumes.get_mut(target) {
                *slot = 1.0;
            }
            if let Some(slot) = right.volumes.get_mut(target) {
                *slot = 1.0;
            }
            pbs.push(left);
            pbs.push(right);
        }
        pbs
    }

    pub fn build_outputs(&self) -> Vec<OutputChannel> {
        let mut outs = Vec::new();
        for (i, pair) in self.outputs.iter().enumerate() {
            outs.push(OutputChannel::new(i * 2, &format!("OUT {}", pair.left)));
            outs.push(OutputChannel::new(
                i * 2 + 1,
                &format!("OUT {}", pair.right),
            ));
        }
        outs
    }
}
