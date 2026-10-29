//! ARDOP TNC host interface (COP-97): command and data TCP ports speaking
//! the ARDOP host protocol that Pat (via `wl2k-go/transport/ardop`) and
//! Winlink Express drive.
//!
//! The wire facts here come from Pat's own ARDOP transport source (read
//! verbatim during COP-97's research): no greeting on connect, `\r`-terminated
//! lines, commands acked by echoing them back, `NEWSTATE`/`CONNECTED`/
//! `TARGET`/`DISCONNECTED`/`BUSY`/`PTT`/`BUFFER` notifications, and a data
//! port framed as `<len:u16 BE><payload>` host→TNC and
//! `<len:u16 BE><tag:3 ASCII><payload>` TNC→host. Over the air coppa still
//! speaks coppa: this is a host *protocol* interface, not an ARDOP waveform.
//!
//! The module deliberately duplicates `vara/`'s shape rather than sharing
//! it: the two wire protocols differ where it matters (echo vs `OK`,
//! `TRUE`/`FALSE` vs `ON`/`OFF`, framed vs raw data port, no greeting), and
//! a premature abstraction across two wire protocols would couple their
//! evolutions for no saved code.

pub mod command;
pub mod data;
pub mod frame;
pub mod protocol;
pub mod server;

pub use command::{queue_response, response_channel, Queued, COMMAND_QUEUE_DEPTH};
pub use data::{data_channel, queue_data, DataQueued, DATA_QUEUE_DEPTH};
pub use protocol::{ArdopCommand, ArdopResponse};
pub use server::ArdopServer;

/// First client ID handed to an ARDOP command-port connection
/// (`0xC000_0001`+). Disjoint from the VARA command (1+), WebSocket
/// (`0x4000_0001`+) and VARA data (`0x8000_0001`+) namespaces, so a
/// client-scoped reply keyed by ID can never land on the wrong server.
pub const FIRST_ARDOP_CMD_CLIENT_ID: u32 = 0xC000_0001;

/// First client ID handed to an ARDOP data-port connection
/// (`0xA000_0001`+); see [`FIRST_ARDOP_CMD_CLIENT_ID`].
pub const FIRST_ARDOP_DATA_CLIENT_ID: u32 = 0xA000_0001;
