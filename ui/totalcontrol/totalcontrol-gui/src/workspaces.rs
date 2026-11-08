//! Named TotalControl workspaces.
//!
//! A workspace is the portable host-side configuration: the current mixer
//! scene, the eight snapshots, channel labels, groups, layouts, view state
//! and Control Room preferences.  It is deliberately separate from the
//! mixer's auto-saved scene and from the unit's six device-side setups.  A
//! workspace checks the device model but does not store the serial number.
//!
//! The file is JSON with the `.tcws` extension.  Writes are atomic so a
//! failed save cannot destroy the previous workspace.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use totalcontrol_core::{ChannelId, Scene};

use crate::app::{RowMode, View, OUTPUT_DESTS};
use crate::prefs::ControlRoomSettings;
use crate::sidebar::Group;

pub const EXTENSION: &str = "tcws";
pub const SCHEMA: u32 = 1;

fn default_schema() -> u32 {
    SCHEMA
}

fn default_scale() -> f32 {
    1.0
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WorkspaceView {
    pub view: View,
    pub row_mode: RowMode,
    pub sidebar_open: bool,
    pub sidebar_width: f32,
    pub panels_open: [bool; 4],
    pub fx_panel: bool,
    pub ui_scale: f32,
    pub sel_out: usize,
    pub quick_channel: ChannelId,
    pub show_original_names: bool,
    pub matrix_mono: bool,
    pub active_snapshot: Option<usize>,
    pub active_layout: Option<usize>,
    pub collapsed: HashSet<ChannelId>,
    /// The Talkback button is latched on in this workspace.
    pub talkback_on: bool,
    /// External Input is latched on in this workspace.
    pub external_input_on: bool,
    /// Input channels listening through PFL when the workspace was saved.
    pub pfl_inputs: Vec<ChannelId>,
}

impl Default for WorkspaceView {
    fn default() -> Self {
        Self {
            view: View::Quick,
            row_mode: RowMode::Three,
            sidebar_open: true,
            sidebar_width: crate::sidebar::DEFAULT_WIDTH,
            panels_open: [true; 4],
            fx_panel: true,
            ui_scale: default_scale(),
            sel_out: 0,
            quick_channel: ChannelId::Input(0),
            show_original_names: false,
            matrix_mono: false,
            active_snapshot: None,
            active_layout: None,
            collapsed: HashSet::new(),
            talkback_on: false,
            external_input_on: false,
            pfl_inputs: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct WorkspaceSnapshot {
    /// User-visible snapshot name.  Defaults to `Mix n` when empty.
    pub name: String,
    pub scene: Option<Scene>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Workspace {
    #[serde(default = "default_schema")]
    pub schema: u32,
    pub name: String,
    /// The current mixer scene.
    pub scene: Scene,
    /// The eight Mix slots.
    pub snapshots: [WorkspaceSnapshot; 8],
    pub groups: [Group; 4],
    /// The six Mixer Layout slots.
    pub layouts: [Option<HashSet<ChannelId>>; 6],
    pub labels: Vec<(ChannelId, String)>,
    pub view: WorkspaceView,
    pub control_room: ControlRoomSettings,
    pub assign: [Option<usize>; 7],
    pub surround_51: bool,
    pub surround_61: bool,
    pub surround_71: bool,
    pub window_size: Option<(f32, f32)>,
}

impl Default for Workspace {
    fn default() -> Self {
        let mut scene = Scene::new("Untitled");
        scene.model = String::new();
        Self {
            schema: SCHEMA,
            name: "Untitled".into(),
            scene,
            snapshots: Default::default(),
            groups: Default::default(),
            layouts: Default::default(),
            labels: Vec::new(),
            view: WorkspaceView::default(),
            control_room: ControlRoomSettings::default(),
            assign: [None; 7],
            surround_51: false,
            surround_61: false,
            surround_71: false,
            window_size: None,
        }
    }
}

/// A predefined direct surround layout.  The source channels follow the
/// virtual sink's channel order; each source is routed to one hardware
/// output channel, with FL/FR and SL/SR kept as stereo pairs and C, LFE
/// and BC kept as mono channels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurroundLayout {
    Direct51,
    Direct61,
}

impl SurroundLayout {
    pub const ALL: [Self; 2] = [Self::Direct51, Self::Direct61];

    pub fn menu_label(self) -> &'static str {
        match self {
            Self::Direct51 => "5.1 Direct",
            Self::Direct61 => "6.1 Direct",
        }
    }

    pub fn name(self) -> &'static str {
        self.menu_label()
    }

    fn sources(self) -> &'static [SurroundSourceSpec] {
        match self {
            Self::Direct51 => DIRECT_51_SOURCES,
            Self::Direct61 => DIRECT_61_SOURCES,
        }
    }

    fn outputs(self) -> &'static [SurroundOutputSpec] {
        match self {
            Self::Direct51 => DIRECT_51_OUTPUTS,
            Self::Direct61 => DIRECT_61_OUTPUTS,
        }
    }

    fn routes(self) -> &'static [SurroundRouteSpec] {
        match self {
            Self::Direct51 => DIRECT_51_ROUTES,
            Self::Direct61 => DIRECT_61_ROUTES,
        }
    }

    /// Software-playback pairs that are split into two mono strips.
    fn split_pairs(self) -> &'static [usize] {
        match self {
            Self::Direct51 => &[1],
            Self::Direct61 => &[1, 2],
        }
    }

    fn six_one(self) -> bool {
        self == Self::Direct61
    }
}

struct SurroundSourceSpec {
    channel: usize,
    label: &'static str,
}

struct SurroundOutputSpec {
    /// Display position of the hardware-output pair.
    pair: usize,
    linked: bool,
    left: &'static str,
    right: &'static str,
    left_member: bool,
    right_member: bool,
}

struct SurroundRouteSpec {
    source: usize,
    pair: usize,
    side: usize,
}

const DIRECT_51_SOURCES: &[SurroundSourceSpec] = &[
    SurroundSourceSpec { channel: 0, label: "FL" },
    SurroundSourceSpec { channel: 1, label: "FR" },
    SurroundSourceSpec { channel: 2, label: "C" },
    SurroundSourceSpec { channel: 3, label: "LFE" },
    SurroundSourceSpec { channel: 4, label: "SL" },
    SurroundSourceSpec { channel: 5, label: "SR" },
];

const DIRECT_51_OUTPUTS: &[SurroundOutputSpec] = &[
    SurroundOutputSpec {
        pair: 0,
        linked: true,
        left: "FL",
        right: "FR",
        left_member: true,
        right_member: true,
    },
    SurroundOutputSpec {
        pair: 1,
        linked: false,
        left: "C",
        right: "LFE",
        left_member: true,
        right_member: true,
    },
    SurroundOutputSpec {
        pair: 2,
        linked: true,
        left: "SL",
        right: "SR",
        left_member: true,
        right_member: true,
    },
];

const DIRECT_51_ROUTES: &[SurroundRouteSpec] = &[
    SurroundRouteSpec { source: 0, pair: 0, side: 0 },
    SurroundRouteSpec { source: 1, pair: 0, side: 1 },
    SurroundRouteSpec { source: 2, pair: 1, side: 0 },
    SurroundRouteSpec { source: 3, pair: 1, side: 1 },
    SurroundRouteSpec { source: 4, pair: 2, side: 0 },
    SurroundRouteSpec { source: 5, pair: 2, side: 1 },
];

const DIRECT_61_SOURCES: &[SurroundSourceSpec] = &[
    SurroundSourceSpec { channel: 0, label: "FL" },
    SurroundSourceSpec { channel: 1, label: "FR" },
    SurroundSourceSpec { channel: 2, label: "C" },
    SurroundSourceSpec { channel: 3, label: "LFE" },
    SurroundSourceSpec { channel: 4, label: "BC" },
    SurroundSourceSpec { channel: 5, label: "Unused" },
    SurroundSourceSpec { channel: 6, label: "SL" },
    SurroundSourceSpec { channel: 7, label: "SR" },
];

const DIRECT_61_OUTPUTS: &[SurroundOutputSpec] = &[
    SurroundOutputSpec {
        pair: 0,
        linked: true,
        left: "FL",
        right: "FR",
        left_member: true,
        right_member: true,
    },
    SurroundOutputSpec {
        pair: 1,
        linked: false,
        left: "C",
        right: "LFE",
        left_member: true,
        right_member: true,
    },
    SurroundOutputSpec {
        pair: 2,
        linked: false,
        left: "BC",
        right: "Unused",
        left_member: true,
        right_member: false,
    },
    SurroundOutputSpec {
        pair: 3,
        linked: true,
        left: "SL",
        right: "SR",
        left_member: true,
        right_member: true,
    },
];

const DIRECT_61_ROUTES: &[SurroundRouteSpec] = &[
    SurroundRouteSpec { source: 0, pair: 0, side: 0 },
    SurroundRouteSpec { source: 1, pair: 0, side: 1 },
    SurroundRouteSpec { source: 2, pair: 1, side: 0 },
    SurroundRouteSpec { source: 3, pair: 1, side: 1 },
    SurroundRouteSpec { source: 4, pair: 2, side: 0 },
    SurroundRouteSpec { source: 6, pair: 3, side: 0 },
    SurroundRouteSpec { source: 7, pair: 3, side: 1 },
];

/// How the current device numbers output pairs and their per-pair send
/// arrays.  `output_order` maps a display position to the internal pair
/// index; `matrix_uses_display_order` is true for the UC, whose channel
/// arrays are already in display order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SurroundTopology {
    pub output_order: Vec<usize>,
    pub matrix_uses_display_order: bool,
}

impl SurroundTopology {
    fn internal_pair(&self, display_pair: usize) -> Option<usize> {
        self.output_order.get(display_pair).copied()
    }

    fn volume_pair(&self, display_pair: usize) -> Option<usize> {
        if self.matrix_uses_display_order {
            Some(display_pair)
        } else {
            self.internal_pair(display_pair)
        }
    }
}

fn set_label(workspace: &mut Workspace, channel: ChannelId, label: &str) {
    workspace.labels.retain(|(id, _)| *id != channel);
    workspace.labels.push((channel, label.to_string()));
}

/// Turn the current workspace into a direct 5.1 or 6.1 surround layout.
///
/// Playback routing is cleared first, then each source channel is sent to
/// one physical output side in media-player order.  FL/FR and SL/SR are
/// linked stereo pairs; C, LFE and BC are split mono channels.  The used
/// output channels form group 1.
pub fn configure_direct(
    workspace: &mut Workspace,
    layout: SurroundLayout,
    topology: &SurroundTopology,
) -> Result<(), String> {
    let sources = layout.sources();
    let outputs = layout.outputs();
    let routes = layout.routes();

    for source in sources {
        if workspace.scene.playbacks.get(source.channel).is_none() {
            return Err(format!(
                "This device has no software playback channel {}",
                source.channel + 1
            ));
        }
    }

    let mut internal_pairs = Vec::with_capacity(outputs.len());
    let mut volume_pairs = Vec::with_capacity(outputs.len());
    let mut chosen_blocks = std::collections::HashSet::new();
    for output in outputs {
        let internal = topology.internal_pair(output.pair).ok_or_else(|| {
            format!(
                "This device has no hardware output pair {}",
                output.pair + 1
            )
        })?;
        let volume = topology.volume_pair(output.pair).ok_or_else(|| {
            format!(
                "This device has no hardware output pair {}",
                output.pair + 1
            )
        })?;
        if workspace.scene.outputs.len() < internal * 2 + 2 {
            return Err(format!(
                "This device has no hardware output pair {}",
                output.pair + 1
            ));
        }
        internal_pairs.push(internal);
        volume_pairs.push(volume);
        chosen_blocks.insert(output.pair * 2);
        chosen_blocks.insert(output.pair * 2 + 1);
    }

    // Clear the playback row so no old send leaks into the surround
    // outputs, then clear input sends and mono-matrix cells that address
    // the output pairs chosen above.  Input routing to other outputs is
    // left alone.
    for playback in &mut workspace.scene.playbacks {
        playback.volumes.fill(0.0);
        playback.pans.fill(0);
        playback.mute = false;
        playback.solo = false;
        playback.trim = 0.0;
        playback.width = 1.0;
        playback.ms_proc = false;
        playback.split = false;
    }
    for input in &mut workspace.scene.inputs {
        for &pair in &volume_pairs {
            if let Some(volume) = input.volumes.get_mut(pair) {
                *volume = 0.0;
            }
        }
    }
    workspace.scene.matrix_overrides.retain(|cell| {
        cell.source < 18 && !chosen_blocks.contains(&cell.block)
    });

    for output in outputs {
        let internal = topology.internal_pair(output.pair).ok_or_else(|| {
            format!(
                "This device has no hardware output pair {}",
                output.pair + 1
            )
        })?;
        let link = &mut workspace.scene.settings.output_link;
        if link.len() <= internal {
            link.resize(internal + 1, true);
        }
        link[internal] = output.linked;
        for (side, (label, member)) in [
            (output.left, output.left_member),
            (output.right, output.right_member),
        ]
        .into_iter()
        .enumerate()
        {
            let channel = internal * 2 + side;
            let out = &mut workspace.scene.outputs[channel];
            out.volume = if member { 1.0 } else { 0.0 };
            out.mute = false;
            out.solo = false;
            out.balance = 0;
            out.phase_l = false;
            out.phase_r = false;
            out.no_trim = false;
            out.loopback = false;
            out.cue = false;
            out.talkback = false;
            set_label(workspace, ChannelId::Output(channel), label);
        }
    }

    for &pair in layout.split_pairs() {
        for side in 0..2 {
            let channel = pair * 2 + side;
            let Some(playback) = workspace.scene.playbacks.get_mut(channel) else {
                return Err(format!(
                    "This device has no software playback channel {}",
                    channel + 1
                ));
            };
            playback.split = true;
        }
    }

    for route in routes {
        if route.side != route.source % 2 {
            return Err("Surround route crosses playback sides".into());
        }
        let pair = topology.volume_pair(route.pair).ok_or_else(|| {
            format!(
                "This device has no hardware output pair {}",
                route.pair + 1
            )
        })?;
        let Some(playback) = workspace.scene.playbacks.get_mut(route.source) else {
            return Err(format!(
                "This device has no software playback channel {}",
                route.source + 1
            ));
        };
        if let Some(volume) = playback.volumes.get_mut(pair) {
            *volume = 1.0;
        }
    }

    for source in sources {
        set_label(
            workspace,
            ChannelId::Playback(source.channel),
            source.label,
        );
    }

    let members: Vec<ChannelId> = outputs
        .iter()
        .flat_map(|output| {
            let internal = topology.internal_pair(output.pair).unwrap_or(0);
            [
                (internal * 2, output.left_member),
                (internal * 2 + 1, output.right_member),
            ]
            .into_iter()
            .filter_map(|(channel, member)| member.then_some(ChannelId::Output(channel)))
        })
        .collect();
    workspace.groups[0] = Group {
        members,
        mute_linked: true,
        solo_linked: true,
        fader_linked: true,
    };

    // Control Room pairs are always stereo and cannot be part of the
    // surround layout, so clear any destination that now points at one of
    // the chosen output pairs.
    for slot in workspace.assign[..OUTPUT_DESTS].iter_mut() {
        if slot.is_some_and(|pair| internal_pairs.contains(&pair)) {
            *slot = None;
        }
    }
    workspace.scene.settings.main_out = workspace.assign[0];
    workspace.scene.settings.main_b_out = workspace.assign[1];
    workspace.scene.settings.dim = false;
    workspace.scene.settings.speaker_b = false;
    workspace.scene.settings.mono = false;
    workspace.scene.settings.mute_fx = false;
    workspace.scene.settings.external_input = false;
    workspace.view.talkback_on = false;
    workspace.view.external_input_on = false;
    workspace.view.pfl_inputs.clear();
    workspace.view.active_snapshot = None;
    workspace.view.sel_out = internal_pairs[0];

    workspace.surround_51 = !layout.six_one();
    workspace.surround_61 = layout.six_one();
    workspace.surround_71 = false;
    workspace.name = layout.name().to_string();
    workspace.scene.name = workspace.name.clone();
    Ok(())
}

/// Default folder for the file dialogs.  `Documents` when it exists,
/// otherwise the user's home directory.
pub fn default_dir() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let documents = home.join("Documents");
    if documents.is_dir() {
        documents
    } else {
        home
    }
}

/// Read and validate a workspace file.
pub fn load(path: &Path) -> Result<Workspace, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("Could not read {}: {e}", path.display()))?;
    let workspace: Workspace = serde_json::from_str(&text)
        .map_err(|e| format!("Could not parse {}: {e}", path.display()))?;
    if workspace.schema > SCHEMA {
        return Err(format!(
            "{} was written by a newer TotalControl (workspace schema {}).",
            path.display(),
            workspace.schema
        ));
    }
    Ok(workspace)
}

/// Write a workspace atomically.
pub fn save(path: &Path, workspace: &Workspace) -> Result<(), String> {
    let json = serde_json::to_string_pretty(workspace)
        .map_err(|e| format!("Could not serialise workspace: {e}"))?;
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)
        .map_err(|e| format!("Could not create {}: {e}", parent.display()))?;
    let tmp = path.with_extension(format!("{EXTENSION}.tmp"));
    std::fs::write(&tmp, json)
        .map_err(|e| format!("Could not write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("Could not replace {}: {e}", path.display())
    })
}

/// Ensure a save dialog name ends in the workspace extension.
pub fn with_extension(path: PathBuf) -> PathBuf {
    if path.extension().and_then(|s| s.to_str()) == Some(EXTENSION) {
        path
    } else {
        path.with_extension(EXTENSION)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use totalcontrol_core::{OutputChannel, PlaybackChannel};

    fn channel_label(workspace: &Workspace, channel: ChannelId) -> Option<&str> {
        workspace
            .labels
            .iter()
            .find(|(id, _)| *id == channel)
            .map(|(_, label)| label.as_str())
    }

    fn test_workspace(playbacks: usize, outputs: usize) -> Workspace {
        let mut workspace = Workspace::default();
        for i in 0..playbacks {
            workspace.scene.playbacks.push(PlaybackChannel::new(
                i,
                &format!("PB {}", i + 1),
                outputs / 2,
            ));
        }
        for i in 0..outputs {
            workspace
                .scene
                .outputs
                .push(OutputChannel::new(i, &format!("OUT {}", i + 1)));
        }
        workspace.scene.settings.output_link = vec![true; outputs / 2];
        workspace
    }

    fn ucx_topology() -> SurroundTopology {
        SurroundTopology {
            output_order: vec![1, 2, 3, 0, 4, 5, 6, 7, 8],
            matrix_uses_display_order: false,
        }
    }

    fn uc_topology() -> SurroundTopology {
        SurroundTopology {
            output_order: vec![1, 2, 3, 0, 4, 5, 6, 7, 8],
            matrix_uses_display_order: true,
        }
    }

    #[test]
    fn direct_51_keeps_stereo_pairs_and_mono_centre_and_lfe() {
        let mut workspace = test_workspace(18, 18);
        workspace.assign[0] = Some(1);
        workspace.assign[2] = Some(4);
        configure_direct(&mut workspace, SurroundLayout::Direct51, &ucx_topology()).unwrap();

        assert_eq!(workspace.name, "5.1 Direct");
        assert!(workspace.surround_51);
        assert!(!workspace.surround_61);
        assert!(!workspace.surround_71);

        // UCX volume arrays are in internal (Phones-first) pair order.
        for (source, pair) in [(0, 1), (1, 1), (2, 2), (3, 2), (4, 3), (5, 3)] {
            assert_eq!(workspace.scene.playbacks[source].volumes[pair], 1.0);
        }
        for source in 0..6 {
            for pair in 0..9 {
                if ![(0, 1), (1, 1), (2, 2), (3, 2), (4, 3), (5, 3)].contains(&(source, pair)) {
                    assert_eq!(
                        workspace.scene.playbacks[source].volumes[pair], 0.0,
                        "unexpected send from playback {source} to pair {pair}"
                    );
                }
            }
        }
        assert!(workspace.scene.playbacks[2].split);
        assert!(workspace.scene.playbacks[3].split);
        assert!(!workspace.scene.playbacks[0].split);
        assert!(!workspace.scene.playbacks[1].split);
        assert!(!workspace.scene.playbacks[4].split);
        assert!(!workspace.scene.playbacks[5].split);

        assert!(workspace.scene.settings.output_link[1]);
        assert!(!workspace.scene.settings.output_link[2]);
        assert!(workspace.scene.settings.output_link[3]);

        assert_eq!(channel_label(&workspace, ChannelId::Playback(0)), Some("FL"));
        assert_eq!(channel_label(&workspace, ChannelId::Playback(1)), Some("FR"));
        assert_eq!(channel_label(&workspace, ChannelId::Playback(2)), Some("C"));
        assert_eq!(channel_label(&workspace, ChannelId::Playback(3)), Some("LFE"));
        assert_eq!(channel_label(&workspace, ChannelId::Playback(4)), Some("SL"));
        assert_eq!(channel_label(&workspace, ChannelId::Playback(5)), Some("SR"));
        assert_eq!(channel_label(&workspace, ChannelId::Output(2)), Some("FL"));
        assert_eq!(channel_label(&workspace, ChannelId::Output(3)), Some("FR"));
        assert_eq!(channel_label(&workspace, ChannelId::Output(4)), Some("C"));
        assert_eq!(channel_label(&workspace, ChannelId::Output(5)), Some("LFE"));
        assert_eq!(channel_label(&workspace, ChannelId::Output(6)), Some("SL"));
        assert_eq!(channel_label(&workspace, ChannelId::Output(7)), Some("SR"));

        assert_eq!(
            workspace.groups[0].members,
            vec![
                ChannelId::Output(2),
                ChannelId::Output(3),
                ChannelId::Output(4),
                ChannelId::Output(5),
                ChannelId::Output(6),
                ChannelId::Output(7),
            ]
        );
        assert!(workspace.groups[0].mute_linked);
        assert!(workspace.groups[0].solo_linked);
        assert!(workspace.groups[0].fader_linked);

        // The two Control Room assignments that pointed at surround outputs
        // are cleared; the third, on an unrelated pair, survives.
        assert_eq!(workspace.assign[0], None);
        assert_eq!(workspace.assign[2], Some(4));
        assert!(workspace.scene.matrix_overrides.is_empty());
    }

    #[test]
    fn direct_51_uses_display_order_volume_indices_on_the_uc() {
        let mut workspace = test_workspace(18, 18);
        configure_direct(&mut workspace, SurroundLayout::Direct51, &uc_topology()).unwrap();

        for (source, pair) in [(0, 0), (1, 0), (2, 1), (3, 1), (4, 2), (5, 2)] {
            assert_eq!(workspace.scene.playbacks[source].volumes[pair], 1.0);
        }
        // Output channels stay in internal Phones-first order.
        assert_eq!(channel_label(&workspace, ChannelId::Output(2)), Some("FL"));
        assert_eq!(channel_label(&workspace, ChannelId::Output(3)), Some("FR"));
        assert_eq!(channel_label(&workspace, ChannelId::Output(7)), Some("SR"));
        assert_eq!(workspace.view.sel_out, 1);
    }

    #[test]
    fn direct_61_preserves_player_order_and_leaves_channel_6_unused() {
        let mut workspace = test_workspace(18, 18);
        configure_direct(&mut workspace, SurroundLayout::Direct61, &ucx_topology()).unwrap();

        assert_eq!(workspace.name, "6.1 Direct");
        assert!(!workspace.surround_51);
        assert!(workspace.surround_61);
        assert!(!workspace.surround_71);

        for (source, pair) in [(0, 1), (1, 1), (2, 2), (3, 2), (4, 3), (6, 0), (7, 0)] {
            assert_eq!(workspace.scene.playbacks[source].volumes[pair], 1.0);
        }
        assert!(workspace.scene.playbacks[5].volumes.iter().all(|&v| v == 0.0));
        for pair in 0..9 {
            assert_eq!(workspace.scene.playbacks[5].volumes[pair], 0.0);
        }

        assert!(workspace.scene.playbacks[2].split);
        assert!(workspace.scene.playbacks[3].split);
        assert!(workspace.scene.playbacks[4].split);
        assert!(workspace.scene.playbacks[5].split);
        assert!(!workspace.scene.playbacks[0].split);
        assert!(!workspace.scene.playbacks[1].split);
        assert!(!workspace.scene.playbacks[6].split);
        assert!(!workspace.scene.playbacks[7].split);

        assert!(workspace.scene.settings.output_link[1]);
        assert!(!workspace.scene.settings.output_link[2]);
        assert!(!workspace.scene.settings.output_link[3]);
        assert!(workspace.scene.settings.output_link[0]);

        assert_eq!(channel_label(&workspace, ChannelId::Playback(4)), Some("BC"));
        assert_eq!(channel_label(&workspace, ChannelId::Playback(5)), Some("Unused"));
        assert_eq!(channel_label(&workspace, ChannelId::Playback(6)), Some("SL"));
        assert_eq!(channel_label(&workspace, ChannelId::Playback(7)), Some("SR"));
        assert_eq!(channel_label(&workspace, ChannelId::Output(6)), Some("BC"));
        assert_eq!(channel_label(&workspace, ChannelId::Output(7)), Some("Unused"));
        assert_eq!(channel_label(&workspace, ChannelId::Output(0)), Some("SL"));
        assert_eq!(channel_label(&workspace, ChannelId::Output(1)), Some("SR"));

        assert_eq!(
            workspace.groups[0].members,
            vec![
                ChannelId::Output(2),
                ChannelId::Output(3),
                ChannelId::Output(4),
                ChannelId::Output(5),
                ChannelId::Output(6),
                ChannelId::Output(0),
                ChannelId::Output(1),
            ]
        );
        assert!(workspace.scene.matrix_overrides.is_empty());
    }

    #[test]
    fn direct_surround_workspaces_round_trip_through_tcws() {
        let dir = std::env::temp_dir().join(format!(
            "totalcontrol-surround-workspace-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("six-one.tcws");

        let mut workspace = test_workspace(18, 18);
        configure_direct(&mut workspace, SurroundLayout::Direct61, &ucx_topology()).unwrap();
        save(&path, &workspace).unwrap();

        let loaded = load(&path).unwrap();
        assert_eq!(loaded.name, "6.1 Direct");
        assert!(loaded.surround_61);
        assert_eq!(loaded.scene.playbacks[4].volumes[3], 1.0);
        assert_eq!(channel_label(&loaded, ChannelId::Playback(4)), Some("BC"));
        assert_eq!(
            loaded.groups[0].members,
            workspace.groups[0].members
        );
        assert!(loaded.scene.matrix_overrides.is_empty());

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn direct_surround_does_not_add_bass_management() {
        for layout in SurroundLayout::ALL {
            let mut workspace = test_workspace(18, 18);
            configure_direct(&mut workspace, layout, &ucx_topology()).unwrap();
            let lfe = &workspace.scene.playbacks[3];
            assert_eq!(lfe.volumes[2], 1.0);
            assert_eq!(lfe.volumes.iter().filter(|&&v| v > 0.0).count(), 1);
            assert!(workspace.scene.matrix_overrides.is_empty());
        }
    }

    #[test]
    fn workspace_round_trips_through_json() {
        let mut workspace = Workspace::default();
        workspace.name = "5.1".into();
        workspace.labels.push((ChannelId::Playback(0), "FL".into()));
        workspace.groups[0].members.push(ChannelId::Output(0));
        workspace.groups[0].fader_linked = true;
        workspace.view.view = View::Mixer;
        workspace.snapshots[0].name = "Surround".into();

        let json = serde_json::to_string(&workspace).unwrap();
        let back: Workspace = serde_json::from_str(&json).unwrap();
        assert_eq!(back.name, "5.1");
        assert_eq!(back.labels[0].1, "FL");
        assert!(back.groups[0].fader_linked);
        assert_eq!(back.snapshots[0].name, "Surround");
        assert_eq!(back.view.view, View::Mixer);
    }

    #[test]
    fn save_and_load_round_trips_atomically() {
        let dir = std::env::temp_dir().join(format!(
            "totalcontrol-workspace-{}-{}",
            std::process::id(),
            SCHEMA
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test.tcws");
        let mut workspace = Workspace::default();
        workspace.name = "Test".into();
        save(&path, &workspace).unwrap();
        let loaded = load(&path).unwrap();
        assert_eq!(loaded.name, "Test");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn newer_schema_is_rejected() {
        let mut workspace = Workspace::default();
        workspace.schema = SCHEMA + 1;
        let json = serde_json::to_string(&workspace).unwrap();
        let parse: Workspace = serde_json::from_str(&json).unwrap();
        assert!(parse.schema > SCHEMA);
    }

    #[test]
    fn extension_is_added_once() {
        assert_eq!(with_extension(PathBuf::from("a")), PathBuf::from("a.tcws"));
        assert_eq!(
            with_extension(PathBuf::from("a.tcws")),
            PathBuf::from("a.tcws")
        );
    }
}
