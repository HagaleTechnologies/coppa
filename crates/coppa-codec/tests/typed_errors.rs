use coppa_codec::{BpskModem, CodecError, Modem};
#[test]
fn public_error_contract() {
    fn bounds<T: std::error::Error + Send + Sync + 'static>() {}
    bounds::<CodecError>();
    let mut modem = BpskModem::new();
    for bits in [&[][..], &[0, 1, 0, 1][..]] {
        let audio: Result<Vec<f32>, CodecError> = modem.modulate(bits);
        assert!(audio.is_ok());
    }
    for samples in [&[][..], &[0.0][..]] {
        let decoded: Result<Vec<f32>, CodecError> = modem.demodulate_soft(samples);
        assert!(decoded.is_ok());
    }
}
