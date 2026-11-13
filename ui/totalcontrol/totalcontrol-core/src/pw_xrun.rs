//! PipeWire graph xrun counter for the RME output node.
//!
//! `pw-top` exposes one cumulative `ERR` count per node.  The audio graph
//! xrun is deliberately separate from the USB transport counters: it
//! counts PipeWire graph scheduling errors, not driver/USB faults.

use std::process::Command;

/// Read the current `ERR` count of the RME output node, if `pw-top` and
/// the node are available.
pub fn read() -> Option<u64> {
    let sink = crate::pw_clock::fireface_sink_name()?;
    let out = Command::new("pw-top")
        .args(["-b", "-n", "1"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    parse(&String::from_utf8_lossy(&out.stdout), &sink)
}

/// Parse one `pw-top -b -n 1` table for the named node.
///
/// The data columns are `S ID QUANT RATE WAIT BUSY W/Q B/Q ERR FORMAT
/// NAME`; the node name is the last token and `ERR` is column 8.  The
/// FORMAT columns may be empty on an idle node, so the name is matched
/// rather than assuming a fixed token count.
pub fn parse(text: &str, sink: &str) -> Option<u64> {
    for line in text.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 9 || !parts.iter().any(|part| *part == sink) {
            continue;
        }
        return parts.get(8).and_then(|value| value.parse().ok());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::parse;

    #[test]
    fn parses_the_rme_output_row() {
        let text = "\
S   ID  QUANT   RATE    WAIT    BUSY   W/Q   B/Q  ERR FORMAT           NAME
C  132      0      0    ---     ---   ---   ---     7                  alsa_output.usb-RME_Fireface_UC_Win__x.pro-output-0
C  131      0      0    ---     ---   ---   ---     3                  alsa_input.usb-RME_Fireface_UC_Win__x.pro-input-0
";
        assert_eq!(
            parse(text, "alsa_output.usb-RME_Fireface_UC_Win__x.pro-output-0"),
            Some(7)
        );
        assert_eq!(
            parse(text, "alsa_input.usb-RME_Fireface_UC_Win__x.pro-input-0"),
            Some(3)
        );
        assert_eq!(parse(text, "missing"), None);
    }
}
