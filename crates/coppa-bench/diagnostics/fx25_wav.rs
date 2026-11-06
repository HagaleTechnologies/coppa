//! Generate one FX.25 interoperability recording for the external oracle.
use anyhow::{Context, Result};
fn main() -> Result<()> {
    let path = std::env::args_os()
        .nth(1)
        .context("usage: fx25_wav OUTPUT.wav")?;
    let mut payload = vec![
        0x82, 0xa0, 0xa4, 0xa6, 0x40, 0x40, 0xe0, 0x9c, 0x60, 0x86, 0x82, 0x98, 0x98, 0xe1, 3, 0xf0,
    ];
    payload.extend(b"coppa COP-114 FX25 fixture\n");
    let frame = coppa_codec::afsk::append_fcs(&payload);
    let samples = coppa_codec::afsk::modulate_fx25(&frame)?;
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 48000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut wav = hound::WavWriter::create(path, spec)?;
    for sample in samples {
        wav.write_sample((sample * 16000.0) as i16)?;
    }
    wav.finalize()?;
    Ok(())
}
