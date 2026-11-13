//! `totalcontrol-core` — Abstraction layer for RME audio interfaces on Linux.
//!
//! This crate provides a hardware-agnostic interface for controlling
//! RME audio interfaces through the kernel hwdep control node.
//!
//! # Design
//!
//! The main entrypoint is the [`RmeDevice`] trait, implemented by each
//! supported RME interface (Fireface UC/UCX and Babyface-classic).
//!
//! A device exposes:
//! - [`InputChannel`]s (physical hardware inputs)
//! - [`PlaybackChannel`]s (software playback streams)
//! - Global [`DeviceSettings`] (clock, SPDIF, etc.)

#[cfg(feature = "alsa")]
pub mod babyface_classic;
pub mod capabilities;
pub mod channel;
pub mod device;
pub mod error;
#[cfg(feature = "alsa")]
pub mod fireface;
pub mod mock;
pub mod panel;
pub mod profile;
pub mod profiles;
pub mod pw_clock;
pub mod pw_follow;
pub mod pw_meter;
pub mod pw_xrun;
pub mod scene;
pub mod workspace;

#[cfg(feature = "alsa")]
pub use babyface_classic::BabyfaceClassic;
pub use capabilities::DeviceCapabilities;
pub use channel::{
    ChannelId, ChannelType, Dynamics, Echo, InputChannel, InputLevel, OutputChannel,
    PlaybackChannel, Reverb, Sensitivity,
};
pub use device::{DeviceSettings, RmeDevice, TalkbackConfig};
pub use error::Error;
#[cfg(feature = "alsa")]
pub use fireface::{
    DeviceIdentity as FirefaceIdentity, DeviceInfo as FirefaceInfo, Fireface,
    Model as FirefaceModel,
};
pub use mock::MockDevice;
pub use scene::Scene;
