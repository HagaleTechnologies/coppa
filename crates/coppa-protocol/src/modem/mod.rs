pub mod airtime;
pub mod speed_levels;
pub mod streaming;
pub mod transceiver;

pub use airtime::frame_airtime_s;
pub use speed_levels::{
    code_rate_for_level, max_payload_for_level, modulation_name_for_level, speed_level_components,
};
pub use streaming::{DecodedFrame, StreamingReceiver};
pub use transceiver::{CoppaTransceiver, ReceiveError, TransmitError};
