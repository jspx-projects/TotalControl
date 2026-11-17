//! ALSA rawmidi transport for the MIDI controllers.
//!
//! One worker owns one controller instance's port pair: it reads the
//! controller's byte stream and forwards it as [`Message::ControllerIn`],
//! and drains the feedback channel handed back through
//! [`Message::ControllerReady`].  The engine in [`crate::controllers`] does
//! the parsing and encoding; this module only moves bytes.

use iced::futures::channel::mpsc;
use iced::futures::{SinkExt, Stream};
use iced::stream;

use crate::app::Message;
use crate::mackie::MidiPort;

/// The worker recipe: which instance, which ports to open.  `Hash` because
/// it identifies the `Subscription::run_with` recipe. The protocol is part
/// of the recipe so a protocol change restarts the worker and re-syncs.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ControllerWorkerConfig {
    pub instance: usize,
    pub protocol: String,
    pub input: Option<MidiPort>,
    pub output: Option<MidiPort>,
}

/// The device's own rawmidi ports, found by the card id (for example
/// `FirefaceUC`).  Port 1 is device 0, port 2 is device 1.  Returns fewer
/// than `count` ports when the card exposes fewer.
#[allow(dead_code)] // live port check, run with --ignored
pub fn device_ports(card_id: &str, count: usize) -> Vec<MidiPort> {
    if card_id.is_empty() {
        return Vec::new();
    }
    let Ok(cards) = std::fs::read_to_string("/proc/asound/cards") else {
        return Vec::new();
    };
    for line in cards.lines() {
        // " 2 [FirefaceUC     ]: FirefaceUSB - Fireface UC"
        let Some(open) = line.find('[') else { continue };
        let Some(close) = line.find(']') else { continue };
        if line[open + 1..close].trim() != card_id {
            continue;
        }
        let Ok(card) = line[..open].trim().parse::<u32>() else {
            continue;
        };
        return (0..count as u32)
            .map(|device| MidiPort { card, device })
            .collect();
    }
    Vec::new()
}

#[cfg(feature = "alsa")]
mod imp {
    use super::*;
    use std::io::{Read, Write};
    use std::time::Duration;

    use alsa::rawmidi::Rawmidi;
    use alsa::Direction;

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
    pub fn worker(config: &ControllerWorkerConfig) -> impl Stream<Item = Message> {
        let config = config.clone();
        let instance = config.instance;
        stream::channel(100, async move |mut output| {
            let (tx, mut rx) = mpsc::channel::<Vec<u8>>(4096);
            if output.send(Message::ControllerReady(instance, tx)).await.is_err() {
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
                "MIDI: controller {} up, in {:?}, out {:?}",
                instance + 1,
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
                                if output.send(Message::ControllerIn(instance, bytes)).await.is_err() {
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
pub use imp::worker;

/// Without the ALSA feature there is no rawmidi access.
#[cfg(not(feature = "alsa"))]
pub fn worker(_config: &ControllerWorkerConfig) -> impl Stream<Item = Message> {
    stream::channel(1, async move |_output| {})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_ports_parses_the_card_id() {
        // The live check runs with --ignored; here only the empty cases.
        assert!(device_ports("", 2).is_empty());
        assert!(device_ports("NoSuchCardId", 2).is_empty());
    }

    /// Live check: run with `cargo test -p totalcontrol-gui device_ports -- --ignored`.
    #[test]
    #[ignore]
    fn device_ports_lists_the_live_unit() {
        let cards = std::fs::read_to_string("/proc/asound/cards").unwrap_or_default();
        println!("{cards}");
        for id in ["FirefaceUC", "FirefaceUCX", "Babyface"] {
            println!("{id}: {:?}", device_ports(id, 2));
        }
    }
}
