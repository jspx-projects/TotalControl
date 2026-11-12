//! Stamps the binary with a build identifier: the short git hash, a `dirty`
//! marker when the working tree has uncommitted changes, and the UTC time of
//! the build.  The About dialog shows it and `--build-id` prints it, which is
//! how a user, a bug report or a shell check tells one build from the next
//! without relying on file timestamps or checksums.

use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn main() {
    let hash = git(&["rev-parse", "--short=7", "HEAD"])
        .filter(|hash| !hash.is_empty())
        .unwrap_or_else(|| "unknown".to_string());

    // `..` is the crate's parent (`ui/totalcontrol`), so a change in the core
    // or the layout crate marks the build as dirty as well.
    let dirty = git(&["status", "--porcelain", "--", ".."]).map_or(false, |s| !s.is_empty());

    let stamp = Command::new("date")
        .args(["-u", "+%Y%m%dT%H%M%SZ"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|stamp| !stamp.is_empty())
        .unwrap_or_else(|| {
            let secs = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            secs.to_string()
        });

    let id = if dirty {
        format!("{hash}-dirty-{stamp}")
    } else {
        format!("{hash}-{stamp}")
    };
    println!("cargo:rustc-env=TC_BUILD_ID={id}");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src");
}
