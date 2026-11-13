//! Per-model device profiles — see [`crate::profile::DeviceProfile`].
//!
//! Each supported model has one profile const. The profile is the single
//! source for channel names, topology and output display order. Backends and
//! mocks build their channels from it, so they cannot drift apart.

pub mod babyface_classic;
pub mod uc;
pub mod ucx;
