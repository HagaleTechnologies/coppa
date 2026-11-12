pub mod airtime;
/// Air-protocol version 2 low-rate floor framing (COP-190): `FloorFrame`,
/// the continuous `FloorStream` receiver and their constants.
pub mod floor;
/// Per-speed-level tables: constellation, LDPC rate and payload capacity for
/// levels 1 to 9.
pub mod speed_levels;
pub mod streaming;
/// `CoppaTransceiver`, the one-frame encode/decode pipeline, and its
/// `TransmitError`/`ReceiveError` types.
pub mod transceiver;

pub use airtime::frame_airtime_s;
pub use floor::{FloorError, FloorFrame, FloorStream, FLOOR_AIR_PROTOCOL_VERSION};
pub use speed_levels::{
    code_rate_for_level, max_payload_for_level, modulation_name_for_level, speed_level_components,
};
pub use streaming::{DecodedFrame, StreamingReceiver};
pub use transceiver::{CoppaTransceiver, HarqPolicy, ReceiveError, TransmitError};
