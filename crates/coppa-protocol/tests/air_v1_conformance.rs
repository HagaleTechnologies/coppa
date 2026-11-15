//! Air-protocol version 1 conformance guards (COP-178, `docs/SPEC.md` §0.3).
//!
//! Every test here pins one row of SPEC §0.3's freeze table with a literal
//! expected value: header bit assignments, enumerations, profile parameters,
//! protected-header coding, the BG2 mother code, per-level FEC parameters,
//! rate matching for RV0-RV3, the scrambler, constellation bit order, the
//! block interleaver, MAC/transport type values and every capability bit.
//!
//! The expected values are literals (or FNV-1a 64 digests of integer/bit
//! streams produced from fixed inputs), never a second computation from the
//! production constants. A change to any of them is an incompatible air
//! change: SPEC §0.3 requires a new air-protocol version for it, not an edit
//! of the literal here. Every assertion message names `air protocol v1` and
//! the frozen field it guards. Float waveform samples are deliberately not
//! hashed (they are not bit-identical across platforms, COP-129); the
//! committed WAVs in `golden_vectors.rs` guard the analog end of the chain.

use coppa_codec::ofdm::frame::{CoppaFrameType, CoppaHeader};
use coppa_codec::ofdm::header_fec::{encode_header, PROTECTED_HEADER_CODED_BITS};
use coppa_codec::ofdm::interleaver::BlockInterleaver;
use coppa_codec::ofdm::CoppaProfile;
use coppa_protocol::fec::ldpc::{nr_bg2, rate_match, NrLdpc};
use coppa_protocol::fec::scrambler::prbs_bits;
use coppa_protocol::mac::{Callsign, MacFrameType, MacPdu};
use coppa_protocol::modem::speed_levels::{
    code_rate_for_level, k_used_for_level, max_payload_for_level, modulation_name_for_level,
    speed_level_components,
};
use coppa_protocol::session::LinkCapabilities;
use coppa_protocol::transport::TransportType;

/// The air-protocol version this file guards. A literal on purpose.
const AIR: &str = "air protocol v1";

/// Coded bits per codeword on air (SPEC §5.2, `CODED_BLOCK_LEN`).
const E: usize = 1944;

/// FNV-1a 64 over a byte stream: a stable digest of integer/bit sequences.
fn fnv1a(bytes: impl IntoIterator<Item = u8>) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A fixed, platform-independent info-bit pattern of `n` bits (an LCG, not
/// the scrambler, so the two guards stay independent).
fn fixed_bits(n: usize, seed: u32) -> Vec<u8> {
    let mut x = seed;
    (0..n)
        .map(|_| {
            x = x.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            ((x >> 16) & 1) as u8
        })
        .collect()
}

fn header(frame_type: CoppaFrameType) -> CoppaHeader {
    CoppaHeader {
        version: 1,
        phy_mode: 0,
        frame_type,
        bandwidth: 0,
        fec_type: 0,
        speed_level: 1,
        seq_num: 0,
        payload_len: 0,
        codewords: 1,
    }
}

/// SPEC §4.2: each header field sits at its frozen bit position, MSB first.
#[test]
fn air_v1_phy_header_bit_layout() {
    let h = CoppaHeader {
        version: 1,
        phy_mode: 1,
        frame_type: CoppaFrameType::Beacon,
        bandwidth: 2,
        fec_type: 3,
        speed_level: 9,
        seq_num: 0xA5,
        payload_len: 0xABC,
        codewords: 8,
    };
    assert_eq!(
        hex(&h.to_bytes()),
        "116239a5abc7",
        "{AIR}: PHY header byte layout (§4.2)"
    );
    // One field at a time, so a failure names the moved field.
    let base = header(CoppaFrameType::Data);
    let cases: [(&str, CoppaHeader, &str); 8] = [
        ("version nibble", base.clone(), "100001000000"),
        (
            "phy_mode nibble",
            CoppaHeader {
                phy_mode: 1,
                ..base.clone()
            },
            "110001000000",
        ),
        (
            "frame_type nibble",
            CoppaHeader {
                frame_type: CoppaFrameType::Ack,
                ..base.clone()
            },
            "101001000000",
        ),
        (
            "bandwidth nibble",
            CoppaHeader {
                bandwidth: 3,
                ..base.clone()
            },
            "100301000000",
        ),
        (
            "fec_type (RV) nibble",
            CoppaHeader {
                fec_type: 2,
                ..base.clone()
            },
            "100021000000",
        ),
        (
            "seq_num byte",
            CoppaHeader {
                seq_num: 0x7F,
                ..base.clone()
            },
            "1000017f0000",
        ),
        (
            "payload_len 12 bits",
            CoppaHeader {
                payload_len: 4095,
                ..base.clone()
            },
            "10000100fff0",
        ),
        (
            "codewords-1 nibble",
            CoppaHeader {
                codewords: 16,
                ..base.clone()
            },
            "10000100000f",
        ),
    ];
    for (field, h, expected) in cases {
        assert_eq!(hex(&h.to_bytes()), expected, "{AIR}: PHY header {field}");
    }
}

/// SPEC §4.2: PHY `frame_type` enumeration; 7..=15 invalid.
#[test]
fn air_v1_phy_frame_type_values() {
    let values = [
        (CoppaFrameType::Data, 0u8),
        (CoppaFrameType::Ack, 1),
        (CoppaFrameType::Nak, 2),
        (CoppaFrameType::Connect, 3),
        (CoppaFrameType::ConnectAck, 4),
        (CoppaFrameType::Disconnect, 5),
        (CoppaFrameType::Beacon, 6),
    ];
    for (t, v) in values {
        assert_eq!(t as u8, v, "{AIR}: PHY frame_type {t:?}");
        assert_eq!(
            CoppaFrameType::from_u8(v),
            Some(t),
            "{AIR}: PHY frame_type {v}"
        );
    }
    for v in 7u8..=15 {
        assert_eq!(
            CoppaFrameType::from_u8(v),
            None,
            "{AIR}: PHY frame_type {v} stays invalid/reserved"
        );
    }
}

/// SPEC §1.1: profile carrier and timing parameters.
#[test]
fn air_v1_profile_parameters() {
    let row = |p: CoppaProfile| -> ProfileRow {
        (
            p.fft_size,
            p.sample_rate,
            p.cp_samples,
            p.data_carriers,
            p.pilot_carriers,
            p.phy_mode,
            p.bandwidth_id,
            p.carrier_offset,
        )
    };
    let profiles: [(&str, CoppaProfile, ProfileRow); 7] = [
        ("hf_standard", CoppaProfile::hf_standard(), HF_STANDARD),
        ("hf_robust", CoppaProfile::hf_robust(), HF_ROBUST),
        ("hf_narrow", CoppaProfile::hf_narrow(), HF_NARROW),
        ("hf_wide", CoppaProfile::hf_wide(), HF_WIDE),
        (
            "hf_standard_short_cp",
            CoppaProfile::hf_standard_short_cp(),
            HF_STANDARD_SHORT_CP,
        ),
        ("vhf_narrow", CoppaProfile::vhf_narrow(), VHF_NARROW),
        ("vhf_wide", CoppaProfile::vhf_wide(), VHF_WIDE),
    ];
    for (name, p, expected) in profiles {
        assert_eq!(
            row(p),
            expected,
            "{AIR}: profile {name} (fft, rate, cp, data, pilots, phy_mode, bandwidth_id, carrier_offset)"
        );
    }
}

/// (fft_size, sample_rate, cp_samples, data_carriers, pilot_carriers,
/// phy_mode, bandwidth_id, carrier_offset)
type ProfileRow = (usize, u32, usize, usize, usize, u8, u8, usize);

const HF_STANDARD: ProfileRow = (960, 48000, 300, 44, 4, 0, 1, 6);
const HF_ROBUST: ProfileRow = (960, 48000, 300, 36, 12, 0, 3, 6);
const HF_NARROW: ProfileRow = (960, 48000, 300, 8, 2, 0, 0, 6);
const HF_WIDE: ProfileRow = (960, 48000, 300, 46, 4, 0, 2, 6);
const HF_STANDARD_SHORT_CP: ProfileRow = (960, 48000, 144, 44, 4, 0, 4, 6);
const VHF_NARROW: ProfileRow = (960, 48000, 60, 44, 4, 1, 1, 6);
const VHF_WIDE: ProfileRow = (960, 48000, 60, 104, 8, 1, 2, 6);

/// SPEC §4.3: the protected-header code turns 48 header bits into 144 coded
/// bits; pinned by length and digest for every frame type.
#[test]
fn air_v1_protected_header_coding() {
    assert_eq!(
        PROTECTED_HEADER_CODED_BITS, 144,
        "{AIR}: protected header coded length"
    );
    let mut all = Vec::new();
    for t in [
        CoppaFrameType::Data,
        CoppaFrameType::Ack,
        CoppaFrameType::Beacon,
    ] {
        let mut h = header(t);
        h.speed_level = 7;
        h.payload_len = 1234;
        h.codewords = 3;
        h.fec_type = 1;
        let coded = encode_header(&h);
        assert_eq!(coded.len(), 144, "{AIR}: protected header length for {t:?}");
        all.extend(coded);
    }
    assert_eq!(
        fnv1a(all),
        PROTECTED_HEADER_DIGEST,
        "{AIR}: protected-header FEC coded bits (§4.3)"
    );
}
const PROTECTED_HEADER_DIGEST: u64 = 15_064_244_052_753_686_161;

/// SPEC §5.1: NR BG2 mother code, Zc = 176, base graph table content.
#[test]
fn air_v1_bg2_mother_code() {
    assert_eq!(
        (
            nr_bg2::ZC,
            nr_bg2::BASE_ROWS,
            nr_bg2::BASE_COLS,
            nr_bg2::KB,
            nr_bg2::CORE_PARITY_COLS,
            nr_bg2::PUNCTURED_INFO_COLS
        ),
        (176, 42, 52, 10, 4, 2),
        "{AIR}: BG2 dimensions (Zc, rows, cols, Kb, core parity, punctured)"
    );
    assert_eq!(
        (NrLdpc::INFO_LEN, NrLdpc::MOTHER_LEN),
        (1760, 8800),
        "{AIR}: BG2 info and mother lengths"
    );
    let table: Vec<u8> = nr_bg2::ENTRIES
        .iter()
        .flat_map(|&(r, c, s)| {
            [r as u16, c as u16, s as u16]
                .into_iter()
                .flat_map(u16::to_be_bytes)
        })
        .collect();
    assert_eq!(
        (nr_bg2::ENTRIES.len(), fnv1a(table)),
        (BG2_ENTRY_COUNT, BG2_TABLE_DIGEST),
        "{AIR}: BG2 parity-check table (row, col, shift) content"
    );
    let info = fixed_bits(NrLdpc::INFO_LEN, 178);
    let mother = NrLdpc::new().encode(&info);
    assert_eq!(
        (mother.len(), fnv1a(mother)),
        (8800, BG2_CODEWORD_DIGEST),
        "{AIR}: BG2 encoder output for a fixed info block"
    );
}
const BG2_ENTRY_COUNT: usize = 197;
const BG2_TABLE_DIGEST: u64 = 11_631_875_142_742_523_440;
const BG2_CODEWORD_DIGEST: u64 = 2_812_424_940_314_626_297;

/// SPEC §5.2 and §6: per-level K_used, payload budget, code rate and
/// modulation; levels 0 and 10..=15 invalid.
#[test]
fn air_v1_speed_level_fec_parameters() {
    // (level, K_used, payload bytes, code rate, modulation, bits per symbol)
    type LevelRow = (u8, usize, usize, (u16, u16), &'static str, usize);
    let expected: [LevelRow; 9] = [
        (1, 486, 56, (1, 4), "BPSK", 1),
        (2, 972, 117, (1, 2), "BPSK", 1),
        (3, 972, 117, (1, 2), "QPSK", 2),
        (4, 1458, 178, (3, 4), "QPSK", 2),
        (5, 1296, 158, (2, 3), "8PSK", 3),
        (6, 972, 117, (1, 2), "16-QAM", 4),
        (7, 1458, 178, (3, 4), "16-QAM", 4),
        (8, 1296, 158, (2, 3), "64-QAM", 6),
        (9, 1620, 198, (5, 6), "64-QAM", 6),
    ];
    for (level, k, budget, rate, modulation, bps) in expected {
        assert_eq!(
            k_used_for_level(level),
            Some(k),
            "{AIR}: level {level} K_used"
        );
        assert_eq!(
            max_payload_for_level(level),
            Some(budget),
            "{AIR}: level {level} payload bytes per codeword"
        );
        assert_eq!(
            code_rate_for_level(level),
            Some(rate),
            "{AIR}: level {level} code rate"
        );
        assert_eq!(
            modulation_name_for_level(level),
            Some(modulation),
            "{AIR}: level {level} modulation"
        );
        let (mapper, _) = speed_level_components(level).expect("valid level");
        assert_eq!(
            mapper.bits_per_symbol(),
            bps,
            "{AIR}: level {level} bits per symbol"
        );
    }
    for level in [0u8, 10, 11, 15] {
        assert_eq!(
            k_used_for_level(level),
            None,
            "{AIR}: level {level} stays invalid"
        );
    }
}

/// SPEC §5.2-§5.3: shortening + circular-buffer rate matching for every
/// level and RV0-RV3 from one fixed mother codeword.
#[test]
fn air_v1_rate_matching_rv0_to_rv3() {
    let info = fixed_bits(NrLdpc::INFO_LEN, 7);
    let mother = NrLdpc::new().encode(&info);
    let mut got = Vec::new();
    for level in 1u8..=9 {
        let k_used = k_used_for_level(level).expect("valid level");
        for rv in 0u8..=3 {
            let coded = rate_match::rate_match(&mother, k_used, E, rv);
            assert_eq!(coded.len(), E, "{AIR}: level {level} RV{rv} coded length");
            got.push((level, rv, fnv1a(coded)));
        }
    }
    for ((level, rv, digest), expected) in got.iter().zip(RATE_MATCH_DIGESTS) {
        assert_eq!(
            *digest, expected,
            "{AIR}: rate matching level {level} RV{rv}"
        );
    }
}
#[rustfmt::skip]
const RATE_MATCH_DIGESTS: [u64; 36] = [
    // level 1: RV0, RV1, RV2, RV3
    14932745533230648722, 8506098187265837922, 9826042755055548162, 6550825733377793927,
    // level 2: RV0, RV1, RV2, RV3
    3361061721912350758, 15424846059309272151, 7366482102687137435, 8637815120251679132,
    // level 3: RV0, RV1, RV2, RV3
    3361061721912350758, 15424846059309272151, 7366482102687137435, 8637815120251679132,
    // level 4: RV0, RV1, RV2, RV3
    2392431069049628434, 15557218861922342610, 1113275094879209428, 3345725351525249467,
    // level 5: RV0, RV1, RV2, RV3
    11391062761837092646, 14430733987120827060, 4558494189842173682, 8045713353181244994,
    // level 6: RV0, RV1, RV2, RV3
    3361061721912350758, 15424846059309272151, 7366482102687137435, 8637815120251679132,
    // level 7: RV0, RV1, RV2, RV3
    2392431069049628434, 15557218861922342610, 1113275094879209428, 3345725351525249467,
    // level 8: RV0, RV1, RV2, RV3
    11391062761837092646, 14430733987120827060, 4558494189842173682, 8045713353181244994,
    // level 9: RV0, RV1, RV2, RV3
    14584906771439188313, 4036927957437036309, 12152188842110193927, 6981580525844119561,
];

/// SPEC §5.4: scrambler PRBS sequence.
#[test]
fn air_v1_scrambler_sequence() {
    let bits = prbs_bits(128);
    let packed: Vec<u8> = bits
        .chunks(8)
        .map(|c| c.iter().fold(0u8, |acc, b| (acc << 1) | (b & 1)))
        .collect();
    assert_eq!(
        hex(&packed),
        SCRAMBLER_FIRST_128_BITS,
        "{AIR}: scrambler PRBS first 128 bits"
    );
}
const SCRAMBLER_FIRST_128_BITS: &str = "bf0382090c3628b4f3ba299cf54a3fbc";

/// SPEC §6.1: constellation bit order. Each mapper's points for every bit
/// pattern, quantised to 1e-4, digested per modulation.
#[test]
fn air_v1_constellation_bit_order() {
    for (level, name, expected) in CONSTELLATION_DIGESTS {
        let (mapper, _) = speed_level_components(level).expect("valid level");
        let bps = mapper.bits_per_symbol();
        let mut q = Vec::new();
        for pattern in 0u32..(1 << bps) {
            let bits: Vec<u8> = (0..bps)
                .map(|i| ((pattern >> (bps - 1 - i)) & 1) as u8)
                .collect();
            let p = mapper.map(&bits);
            for v in [p.re, p.im] {
                q.extend(((v * 10_000.0).round() as i32).to_be_bytes());
            }
        }
        assert_eq!(fnv1a(q), expected, "{AIR}: {name} constellation bit order");
    }
}
const CONSTELLATION_DIGESTS: [(u8, &str, u64); 5] = [
    (1, "BPSK", 415_941_087_579_764_024),
    (3, "QPSK", 2_326_548_348_279_954_125),
    (5, "8PSK", 4_991_269_883_943_086_625),
    (6, "16-QAM", 17_520_000_849_374_845_673),
    (8, "64-QAM", 17_928_094_433_764_042_469),
];

/// SPEC §7.1: intra-codeword block interleaver (1944 bits over each
/// profile's data-carrier count: 8, 36, 44, 46 and 104).
#[test]
fn air_v1_block_interleaver_permutation() {
    let input = fixed_bits(E, 71);
    for (carriers, expected) in INTERLEAVER_DIGESTS {
        let out = BlockInterleaver::new(E, carriers).interleave(&input);
        assert_eq!(
            (out.len(), fnv1a(out)),
            (E, expected),
            "{AIR}: block interleaver over {carriers} carriers"
        );
    }
}
const INTERLEAVER_DIGESTS: [(usize, u64); 5] = [
    (8, 14_591_584_153_888_278_527),
    (36, 17_201_681_172_309_814_503),
    (44, 6_368_873_182_445_482_873),
    (46, 9367846692942164969),
    (104, 10714425664465799203),
];

/// SPEC §17.3: MAC frame type nibbles, including datagram (9) and the
/// negotiated outer-code shard (10); 11..=15 reserved.
#[test]
fn air_v1_mac_frame_type_values() {
    let values = [
        (MacFrameType::Data, 0u8),
        (MacFrameType::Ack, 1),
        (MacFrameType::ConnectReq, 2),
        (MacFrameType::ConnectAck, 3),
        (MacFrameType::ConnectCfm, 4),
        (MacFrameType::Disconnect, 5),
        (MacFrameType::Beacon, 6),
        (MacFrameType::Keepalive, 7),
        (MacFrameType::Relay, 8),
        (MacFrameType::Datagram, 9),
        (MacFrameType::OuterData, 10),
    ];
    for (t, v) in values {
        assert_eq!(t as u8, v, "{AIR}: MAC frame type {t:?}");
        assert!(
            MacFrameType::from_u8(v).is_ok(),
            "{AIR}: MAC frame type {v} parses"
        );
    }
    for v in 11u8..=15 {
        assert!(
            MacFrameType::from_u8(v).is_err(),
            "{AIR}: MAC frame type {v} stays reserved"
        );
    }
    // §17.1-§17.2: version nibble, type nibble, callsigns, SSID, length.
    let pdu = MacPdu::new(
        MacFrameType::Datagram,
        Callsign::new("W1AW").unwrap(),
        Callsign::new("VK3ABC").unwrap(),
        5,
        vec![0xAA],
    );
    assert_eq!(
        hex(&pdu.to_bytes()),
        MAC_DATAGRAM_HEX,
        "{AIR}: MAC header (version, type, dest, src, ssid, length)"
    );
}
const MAC_DATAGRAM_HEX: &str = "195dc05700000058b78108300005aa";

/// SPEC §18.2: transport type values.
#[test]
fn air_v1_transport_type_values() {
    let values = [
        (TransportType::Unreliable, 0u8),
        (TransportType::Reliable, 1),
        (TransportType::Ack, 2),
        (TransportType::Nak, 3),
        (TransportType::Reset, 4),
        (TransportType::CpControl, 5),
    ];
    for (t, v) in values {
        assert_eq!(t as u8, v, "{AIR}: transport type {t:?}");
    }
}

/// SPEC §20.1: every CONNECT capability flag bit, one at a time, plus the
/// fixed fields.
#[test]
fn air_v1_capability_bits() {
    let base = LinkCapabilities::default();
    assert_eq!(
        hex(&base.to_bytes()),
        "00c80102080000",
        "{AIR}: default capabilities blob"
    );
    let flags: [(&str, LinkCapabilities, u8); 6] = [
        (
            "compression (bit 0)",
            LinkCapabilities {
                compression: true,
                ..base.clone()
            },
            0x01,
        ),
        (
            "multi_frame_ack (bit 1)",
            LinkCapabilities {
                multi_frame_ack: true,
                ..base.clone()
            },
            0x02,
        ),
        (
            "outer_code_rx (bit 2)",
            LinkCapabilities {
                outer_code_rx: true,
                ..base.clone()
            },
            0x04,
        ),
        (
            "outer_code_req (bit 3)",
            LinkCapabilities {
                outer_code_req: true,
                ..base.clone()
            },
            0x08,
        ),
        (
            "app_messages (bit 4)",
            LinkCapabilities {
                app_messages: true,
                ..base.clone()
            },
            0x10,
        ),
        (
            "windowed_arq (bit 5, COP-168)",
            LinkCapabilities {
                windowed_arq: true,
                ..base.clone()
            },
            0x20,
        ),
    ];
    for (name, caps, bit) in flags {
        let b = caps.to_bytes();
        assert_eq!(b.len(), 7, "{AIR}: capabilities length");
        assert_eq!(b[5], bit, "{AIR}: capability flag {name}");
    }
    let fields = LinkCapabilities {
        max_payload_size: 0x1234,
        fec_rate_num: 3,
        fec_rate_den: 4,
        arq_window: 16,
        mcs_index: 9,
        ..base
    };
    assert_eq!(
        hex(&fields.to_bytes()),
        "12340304100009",
        "{AIR}: capability fields (max payload BE, rate num/den, window, flags, mcs)"
    );
}
