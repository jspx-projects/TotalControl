//! The host "audio performance mode".
//!
//! The audio thread wakes once per graph cycle (about a millisecond at the
//! smallest quanta).  A CPU that enters a deep idle state between those wakes
//! pays the exit latency, and a stall beyond the queued buffer margin starves
//! the device.  The kernel's PM-QoS CPU_DMA_LATENCY interface forbids those
//! deep states for as long as a process holds the device open: writing the
//! allowed exit latency in microseconds to `/dev/cpu_dma_latency` is enough,
//! and the constraint is released when the last fd closes (quit or crash).
//!
//! The packaged udev rule grants the logged-in user access through uaccess,
//! the same mechanism as the sound devices, and falls back to the `audio`
//! group for sessions without a seat.
//!
//! The mode is opt-in and is held only while the card streams, so the idle
//! power cost is limited to active sessions.

use std::fs::File;
use std::io::{self, Write};
use std::path::PathBuf;

/// The allowed CPU wake latency choices in microseconds, exposed as the
/// "Max sleep time" setting. `0` forbids every idle state above C0: the
/// strongest effect, the highest idle power. `10` allows the shallowest
/// idle states.
pub const LATENCY_CHOICES: [i32; 2] = [0, 10];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// The mode is off; no constraint is held.
    Released,
    /// The constraint is held.
    Held,
    /// The interface is absent on this kernel.
    Unavailable,
    /// The interface exists but this user may not write it.
    Denied,
}

/// Holds the optional latency constraint.  One instance lives in the app
/// state; it owns the open device while held.
pub struct Holder {
    path: PathBuf,
    file: Option<File>,
    held_us: Option<i32>,
}

impl Default for Holder {
    fn default() -> Self {
        Self {
            path: PathBuf::from("/dev/cpu_dma_latency"),
            file: None,
            held_us: None,
        }
    }
}

impl Holder {
    /// A holder on another path, for tests.
    #[cfg(test)]
    pub fn with_path(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            file: None,
            held_us: None,
        }
    }

    /// Keep the constraint at `us` while `want`, release it otherwise.
    /// Re-applying with another value rewrites the open interface.
    pub fn apply(&mut self, want: bool, us: i32) -> Status {
        if !want {
            self.file = None;
            self.held_us = None;
            return Status::Released;
        }
        if let Some(file) = self.file.as_mut() {
            if self.held_us == Some(us) {
                return Status::Held;
            }
            if let Err(error) = file.write_all(&us.to_ne_bytes()) {
                return failure(error);
            }
            self.held_us = Some(us);
            return Status::Held;
        }
        let mut file = match File::options().write(true).open(&self.path) {
            Ok(file) => file,
            Err(error) => return failure(error),
        };
        if let Err(error) = file.write_all(&us.to_ne_bytes()) {
            return failure(error);
        }
        self.file = Some(file);
        self.held_us = Some(us);
        Status::Held
    }
}

fn failure(error: io::Error) -> Status {
    match error.kind() {
        io::ErrorKind::PermissionDenied => Status::Denied,
        _ => Status::Unavailable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("tc-audio-perf-{}-{name}", std::process::id()))
    }

    #[test]
    fn releases_by_default_and_is_idempotent() {
        let mut holder = Holder::with_path(temp_path("idle"));
        assert_eq!(holder.apply(false, 0), Status::Released);
        assert_eq!(holder.apply(false, 0), Status::Released);
    }

    #[test]
    fn holds_while_wanted_and_writes_the_latency() {
        let path = temp_path("held");
        std::fs::write(&path, b"").expect("create");
        let mut holder = Holder::with_path(&path);
        assert_eq!(holder.apply(true, 0), Status::Held);
        assert_eq!(holder.apply(true, 0), Status::Held);
        let mut written = Vec::new();
        File::open(&path)
            .expect("the holder created the file")
            .read_to_end(&mut written)
            .expect("read back");
        assert!(written.starts_with(&0i32.to_ne_bytes()));
        assert_eq!(holder.apply(false, 0), Status::Released);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn reports_a_missing_interface() {
        let mut holder = Holder::with_path(temp_path("missing"));
        assert_eq!(holder.apply(true, 0), Status::Unavailable);
    }

    #[test]
    fn reports_a_directory_as_unavailable() {
        let dir = std::env::temp_dir();
        let mut holder = Holder::with_path(&dir);
        assert_eq!(holder.apply(true, 0), Status::Unavailable);
    }

    #[test]
    fn reports_a_read_only_file_as_denied() {
        use std::os::unix::fs::PermissionsExt;

        let path = temp_path("denied");
        std::fs::write(&path, b"x").expect("create");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o444))
            .expect("chmod");
        // Root ignores the mode; the mapping is asserted for a normal user.
        if File::options().write(true).open(&path).is_err() {
            let mut holder = Holder::with_path(&path);
            assert_eq!(holder.apply(true, 0), Status::Denied);
        }
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644));
        let _ = std::fs::remove_file(&path);
    }
}
