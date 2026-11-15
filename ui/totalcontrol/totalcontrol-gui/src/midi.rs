//! ALSA rawmidi transport for the Mackie Control surface.
//!
//! One worker owns one remote's port pair: it reads the surface's byte
//! stream and forwards it as [`Message::MackieIn`], and drains the
//! feedback channel handed back through [`Message::MackieReady`].  The
//! codec in [`crate::mackie`] does the parsing and encoding; this module
//! only moves bytes.
//!
//! Port enumeration lists every rawmidi device of every card, so the
//! settings page can offer the unit's own MIDI ports (a surface on the
//! breakout cable) and any USB MIDI surface alike.

use iced::futures::channel::mpsc;
use iced::futures::{SinkExt, Stream};
use iced::stream;

use crate::app::Message;
use crate::mackie::MidiPort;

/// One rawmidi port as offered in the settings picklists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MidiPortInfo {
    pub port: MidiPort,
    pub label: String,
    pub input: bool,
    pub output: bool,
}

/// The worker recipe: which ports to open.  `Hash` because it identifies
/// the `Subscription::run_with` recipe.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MackieWorkerConfig {
    pub input: Option<MidiPort>,
    pub output: Option<MidiPort>,
}

#[cfg(feature = "alsa")]
mod imp {
    use super::*;
    use std::io::{Read, Write};
    use std::time::Duration;

    use alsa::rawmidi::Rawmidi;
    use alsa::Direction;

    /// Every rawmidi port of every card, inputs and outputs merged.
    pub fn enumerate() -> Vec<MidiPortInfo> {
        let mut ports: Vec<MidiPortInfo> = Vec::new();
        for card in alsa::card::Iter::new().flatten() {
            let Ok(ctl) = alsa::Ctl::from_card(&card, false) else {
                continue;
            };
            let card_name = card.get_name().unwrap_or_default();
            for info in alsa::rawmidi::Iter::new(&ctl).flatten() {
                let port = MidiPort {
                    card: card.get_index() as u32,
                    device: info.get_device() as u32,
                };
                let name = info
                    .get_subdevice_name()
                    .ok()
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| format!("MIDI {}", info.get_device() + 1));
                let entry = ports.iter_mut().find(|p| p.port == port);
                let entry = match entry {
                    Some(entry) => entry,
                    None => {
                        ports.push(MidiPortInfo {
                            port,
                            label: format!("{card_name} — {name}"),
                            input: false,
                            output: false,
                        });
                        ports.last_mut().unwrap()
                    }
                };
                match info.get_stream() {
                    Direction::Capture => entry.input = true,
                    Direction::Playback => entry.output = true,
                }
            }
        }
        ports
    }

    fn open(port: Option<MidiPort>, dir: Direction) -> Option<Rawmidi> {
        let port = port?;
        match Rawmidi::new(&port.hw_name(), dir, true) {
            Ok(r) => Some(r),
            Err(err) => {
                log::warn!("MIDI: cannot open {} ({err})", port.hw_name());
                None
            }
        }
    }

    /// The transport worker.  The input runs on its own thread because
    /// rawmidi reads block; a 1 ms poll keeps the latency below one MIDI
    /// byte time while costing nothing when no surface is connected.
    pub fn worker(config: &MackieWorkerConfig) -> impl Stream<Item = Message> {
        let config = config.clone();
        stream::channel(100, async move |mut output| {
            let (tx, mut rx) = mpsc::channel::<Vec<u8>>(4096);
            if output.send(Message::MackieReady(tx)).await.is_err() {
                return;
            }
            let input = open(config.input, Direction::Capture);
            let out_port = open(config.output, Direction::Playback);
            if input.is_none() && out_port.is_none() {
                log::warn!("MIDI: no port could be opened, remote stays down");
                return;
            }

            let (in_tx, in_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(256);
            // With no input port the sender stays here, keeping the channel
            // open so the loop below only ends on shutdown. With an input
            // port it moves into the reader thread.
            let _in_tx_keepalive = if let Some(input) = input {
                std::thread::spawn(move || {
                    let mut io = input.io();
                    let mut buf = [0u8; 256];
                    loop {
                        match io.read(&mut buf) {
                            Ok(0) => break,
                            Ok(n) => {
                                if in_tx.blocking_send(buf[..n].to_vec()).is_err() {
                                    break;
                                }
                            }
                            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                                // The receiver dies with the worker; a silent
                                // device must not keep this thread alive.
                                if in_tx.is_closed() {
                                    break;
                                }
                                std::thread::sleep(Duration::from_millis(1));
                            }
                            Err(err) => {
                                log::warn!("MIDI: input read failed ({err})");
                                break;
                            }
                        }
                    }
                });
                None
            } else {
                Some(in_tx)
            };
            // Without an input port the input branch never fires.
            let mut in_rx = Some(in_rx);

            log::info!(
                "MIDI: remote up, in {:?}, out {:?}",
                config.input,
                config.output
            );
            loop {
                tokio::select! {
                    bytes = async {
                        match &mut in_rx {
                            Some(rx) => rx.recv().await,
                            None => std::future::pending().await,
                        }
                    } => {
                        match bytes {
                            Some(bytes) => {
                                if output.send(Message::MackieIn(bytes)).await.is_err() {
                                    return;
                                }
                            }
                            // The input thread ended (port error or unplug).
                            None => break,
                        }
                    }
                    bytes = rx.recv() => {
                        match bytes {
                            Ok(bytes) => {
                                if let Some(port) = &out_port {
                                    let mut io = port.io();
                                    if let Err(err) = io.write(&bytes) {
                                        log::warn!("MIDI: output write failed ({err})");
                                    }
                                }
                            }
                            // The app dropped the feedback sender: closed.
                            Err(_) => break,
                        }
                    }
                }
            }
        })
    }
}

#[cfg(feature = "alsa")]
pub use imp::{enumerate, worker};

/// Without the ALSA feature there is no rawmidi access; the settings
/// page shows an empty port list.
#[cfg(not(feature = "alsa"))]
pub fn enumerate() -> Vec<MidiPortInfo> {
    Vec::new()
}

#[cfg(not(feature = "alsa"))]
pub fn worker(_config: &MackieWorkerConfig) -> impl Stream<Item = Message> {
    stream::channel(1, async move |_output| {})
}

/// The picklist label for a port choice; `None` reads "None".
pub fn port_label(port: Option<MidiPort>, ports: &[MidiPortInfo]) -> String {
    match port {
        None => "None".into(),
        Some(port) => ports
            .iter()
            .find(|p| p.port == port)
            .map(|p| p.label.clone())
            .unwrap_or_else(|| format!("hw:{},{}", port.card, port.device)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn port_labels_fall_back_to_the_hw_name() {
        let port = MidiPort { card: 2, device: 0 };
        assert_eq!(port_label(None, &[]), "None");
        assert_eq!(port_label(Some(port), &[]), "hw:2,0");
        let info = MidiPortInfo {
            port,
            label: "Fireface UC — MIDI 1".into(),
            input: true,
            output: true,
        };
        assert_eq!(port_label(Some(port), &[info]), "Fireface UC — MIDI 1");
    }

    /// Live check: run with `cargo test -p totalcontrol-gui enumerate -- --ignored`.
    #[test]
    #[ignore]
    fn enumerate_lists_the_live_ports() {
        let ports = enumerate();
        for p in &ports {
            println!("{:?} in={} out={} {}", p.port, p.input, p.output, p.label);
        }
    }
}