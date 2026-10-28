//! `coppa doctor` (COP-59): inspect audio and rig control without transmitting.

use anyhow::{bail, Result};
use clap::Args;
use coppa_audio::{DeviceProbe, DeviceSupport, Direction, Support};
use coppa_radio::RigctldClient;
use std::fmt;
use std::io::Write;

#[derive(Args, Debug, Default, PartialEq, Eq)]
pub(crate) struct DoctorArgs {
    /// Audio device to check for both input and output (substring match, as for tx and rx).
    #[arg(long, value_name = "NAME")]
    device: Option<String>,
    /// Input device to check (substring match); overrides --device.
    #[arg(long, value_name = "NAME")]
    input_device: Option<String>,
    /// Output device to check (substring match); overrides --device.
    #[arg(long, value_name = "NAME")]
    output_device: Option<String>,
    /// rigctld address to connect to.
    #[arg(
        long,
        value_name = "HOST:PORT",
        default_value = "127.0.0.1:4532",
        conflicts_with = "no_rigctld"
    )]
    rigctld: String,
    /// Skip the rigctld checks (VOX or no-CAT stations).
    #[arg(long)]
    no_rigctld: bool,
}

impl DoctorArgs {
    fn requested(&self, direction: Direction) -> Option<&str> {
        match direction {
            Direction::Input => self.input_device.as_deref().or(self.device.as_deref()),
            Direction::Output => self.output_device.as_deref().or(self.device.as_deref()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Outcome {
    Pass,
    Fail,
    Skip,
}

impl fmt::Display for Outcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Pass => "PASS",
            Self::Fail => "FAIL",
            Self::Skip => "SKIP",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Check {
    name: &'static str,
    outcome: Outcome,
    detail: String,
    hint: Option<&'static str>,
}

impl Check {
    fn new(
        name: &'static str,
        outcome: Outcome,
        detail: impl Into<String>,
        hint: Option<&'static str>,
    ) -> Self {
        Self {
            name,
            outcome,
            detail: detail.into(),
            hint,
        }
    }
}

pub(crate) const HEADER: &str =
    "Checking audio devices and rig control. Nothing is transmitted and PTT is never keyed.";

const BACKEND_HINT: &str =
    "Rebuild with live audio: cargo build --release -p coppa-cli --features cpal-backend";
const DEVICES_HINT: &str = "The audio system reports no devices. Connect the radio's USB audio interface and confirm the OS lists it (Linux: arecord -l and aplay -l).";
const INPUT_DEVICE_HINT: &str = "Pass --input-device (or --device) with part of a name from the device list above, or set a system default input.";
const OUTPUT_DEVICE_HINT: &str = "Pass --output-device (or --device) with part of a name from the device list above, or set a system default output.";
const INPUT_FORMAT_HINT: &str = "Coppa records 1 channel of f32 samples at 48000 Hz. Close other programs using the device (is coppad running?), or pick another input with --input-device; on Linux a plughw: or default: device converts formats.";
const OUTPUT_FORMAT_HINT: &str = "Coppa plays 1 channel of f32 samples at 48000 Hz. Close other programs using the device (is coppad running?), or pick another output with --output-device; on Linux a plughw: or default: device converts formats.";
const RIG_CONNECTION_HINT: &str = "Start rigctld for your radio (for example: rigctld -m <model> -r <serial port>), pass --rigctld HOST:PORT if it listens elsewhere, or --no-rigctld if you key with VOX.";
const RIG_READ_HINT: &str = "rigctld is running but could not read the radio. Check that the radio is on and that rigctld's -m (model), -r (port) and -s (baud) match it.";

fn backend_check(cli_has_cpal: bool, backend: Option<&str>) -> Check {
    if !cli_has_cpal {
        Check::new("audio backend", Outcome::Fail, "this coppa was built without the cpal-backend feature, so tx, rx and tune cannot use live audio", Some(BACKEND_HINT))
    } else if let Some(name) = backend {
        Check::new("audio backend", Outcome::Pass, name, None)
    } else {
        Check::new(
            "audio backend",
            Outcome::Fail,
            "no audio backend in this build",
            Some(BACKEND_HINT),
        )
    }
}

fn devices_check(backend: Option<&str>, listing: &Result<Vec<DeviceSupport>>) -> Check {
    if backend.is_none() {
        Check::new("audio devices", Outcome::Skip, "no audio backend", None)
    } else {
        match listing {
            Ok(devices) if !devices.is_empty() => Check::new(
                "audio devices",
                Outcome::Pass,
                format!("{} found", devices.len()),
                None,
            ),
            Ok(_) => Check::new(
                "audio devices",
                Outcome::Fail,
                "none found",
                Some(DEVICES_HINT),
            ),
            Err(e) => Check::new(
                "audio devices",
                Outcome::Fail,
                format!("could not list devices: {e}"),
                Some(DEVICES_HINT),
            ),
        }
    }
}

fn device_checks(direction: Direction, requested: Option<&str>, probe: &DeviceProbe) -> [Check; 2] {
    let (device_name, format_name, device_hint, format_hint, label) = match direction {
        Direction::Input => (
            "input device",
            "input 48 kHz mono",
            INPUT_DEVICE_HINT,
            INPUT_FORMAT_HINT,
            "input",
        ),
        Direction::Output => (
            "output device",
            "output 48 kHz mono",
            OUTPUT_DEVICE_HINT,
            OUTPUT_FORMAT_HINT,
            "output",
        ),
    };
    match probe {
        DeviceProbe::NoBackend => [
            Check::new(device_name, Outcome::Skip, "no audio backend", None),
            Check::new(format_name, Outcome::Skip, "no audio backend", None),
        ],
        DeviceProbe::NoDefault => [
            Check::new(
                device_name,
                Outcome::Fail,
                format!("the system reports no default {label} device"),
                Some(device_hint),
            ),
            Check::new(
                format_name,
                Outcome::Skip,
                format!("no {label} device to open"),
                None,
            ),
        ],
        DeviceProbe::NoMatch { available } => {
            let detail = format!(
                "no {label} device name contains \"{}\"; available: {}",
                requested.unwrap_or_default(),
                available
                    .iter()
                    .map(|name| format!("\"{name}\""))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            [
                Check::new(device_name, Outcome::Fail, detail, Some(device_hint)),
                Check::new(
                    format_name,
                    Outcome::Skip,
                    format!("no {label} device to open"),
                    None,
                ),
            ]
        }
        DeviceProbe::Opened { device } => {
            let detail = match requested {
                Some(name) if !name.is_empty() => format!("\"{name}\" matched {device}"),
                _ => format!("system default: {device}"),
            };
            [
                Check::new(device_name, Outcome::Pass, detail, None),
                Check::new(
                    format_name,
                    Outcome::Pass,
                    "opened 1 ch f32 at 48000 Hz (not started)",
                    None,
                ),
            ]
        }
        DeviceProbe::OpenFailed {
            device,
            error,
            advertised,
        } => {
            let detail = match requested {
                Some(name) if !name.is_empty() => format!("\"{name}\" matched {device}"),
                _ => format!("system default: {device}"),
            };
            let reason = match advertised {
                Some(Support::No(reason)) => format!("; device offers {reason}"),
                _ => String::new(),
            };
            [
                Check::new(device_name, Outcome::Pass, detail, None),
                Check::new(
                    format_name,
                    Outcome::Fail,
                    format!("could not open 1 ch f32 at 48000 Hz: {error}{reason}"),
                    Some(format_hint),
                ),
            ]
        }
    }
}

fn rigctld_checks(addr: &str) -> [Check; 2] {
    match RigctldClient::connect(addr) {
        Ok(mut client) => {
            let connection = Check::new(
                "rigctld connection",
                Outcome::Pass,
                format!("connected to {addr}"),
                None,
            );
            let read = match client.query_frequency() {
                Ok(hz) => Check::new(
                    "rig CAT read",
                    Outcome::Pass,
                    format!("dial {:.6} MHz", hz as f64 / 1_000_000.0),
                    None,
                ),
                Err(e) => Check::new(
                    "rig CAT read",
                    Outcome::Fail,
                    format!("no frequency from the radio: {e}"),
                    Some(RIG_READ_HINT),
                ),
            };
            [connection, read]
        }
        Err(e) => {
            let message = e.to_string();
            let detail = if message.contains(addr) {
                message
            } else {
                format!("{addr}: {message}")
            };
            [
                Check::new(
                    "rigctld connection",
                    Outcome::Fail,
                    detail,
                    Some(RIG_CONNECTION_HINT),
                ),
                Check::new("rig CAT read", Outcome::Skip, "no rigctld connection", None),
            ]
        }
    }
}

fn skipped_rigctld_checks() -> [Check; 2] {
    [
        Check::new("rigctld connection", Outcome::Skip, "--no-rigctld", None),
        Check::new("rig CAT read", Outcome::Skip, "--no-rigctld", None),
    ]
}

fn printable(value: &str) -> String {
    value
        .chars()
        .flat_map(|c| {
            if c.is_control() {
                c.escape_default().collect::<Vec<_>>()
            } else {
                vec![c]
            }
        })
        .collect()
}

fn render(backend: Option<&str>, listing: &Result<Vec<DeviceSupport>>, checks: &[Check]) -> String {
    let mut output = String::new();
    match backend {
        Some(name) => output.push_str(&format!("\nAudio devices ({}):\n", printable(name))),
        None => output.push_str("\nAudio devices: no audio backend in this build\n"),
    }
    match listing {
        Ok(devices) if !devices.is_empty() => {
            for device in devices {
                output.push_str(&format!(
                    "  {} ({})\n",
                    printable(&device.name),
                    printable(&device.id)
                ));
                for (label, support) in [("input", &device.input), ("output", &device.output)] {
                    let detail = match support {
                        None => "none".to_owned(),
                        Some(Support::Yes) => "mono 48 kHz yes".to_owned(),
                        Some(Support::No(reason)) => {
                            format!("mono 48 kHz no ({})", printable(reason))
                        }
                    };
                    output.push_str(&format!("    {label}:  {detail}\n"));
                }
            }
        }
        Ok(_) => output.push_str("  (none found)\n"),
        Err(e) => output.push_str(&format!(
            "  (could not list devices: {})\n",
            printable(&e.to_string())
        )),
    }
    let width = checks
        .iter()
        .map(|c| c.name.len())
        .max()
        .unwrap_or(0)
        .max("CHECK".len())
        + 2;
    output.push_str(&format!("\n{:<width$}{:<8}DETAIL\n", "CHECK", "RESULT"));
    for check in checks {
        output.push_str(&format!(
            "{:<width$}{:<8}{}\n",
            check.name,
            check.outcome.to_string(),
            printable(&check.detail)
        ));
    }
    if checks.iter().any(|c| c.outcome == Outcome::Fail) {
        output.push_str("\nTo fix:\n");
        for check in checks.iter().filter(|c| c.outcome == Outcome::Fail) {
            if let Some(hint) = check.hint {
                output.push_str(&format!("  {}: {}\n", check.name, hint));
            }
        }
    }
    let passed = checks.iter().filter(|c| c.outcome == Outcome::Pass).count();
    let failed = checks.iter().filter(|c| c.outcome == Outcome::Fail).count();
    let skipped = checks.len() - passed - failed;
    output.push_str(&format!(
        "\nSummary: {passed} passed, {failed} failed, {skipped} skipped\n"
    ));
    output
}

fn verdict(checks: &[Check]) -> Result<()> {
    let failed = checks.iter().filter(|c| c.outcome == Outcome::Fail).count();
    if failed > 0 {
        bail!("{failed} of {} checks failed", checks.len());
    }
    Ok(())
}

pub(crate) fn cmd_doctor(args: &DoctorArgs) -> Result<()> {
    let rate = coppa_engine::config::EngineConfig::default().sample_rate;
    println!("{HEADER}");
    let _ = std::io::stdout().flush();
    let backend = coppa_audio::audio_backend_name();
    let listing = coppa_audio::list_device_support(rate);
    let mut checks = vec![
        backend_check(cfg!(feature = "cpal-backend"), backend),
        devices_check(backend, &listing),
    ];
    for direction in [Direction::Input, Direction::Output] {
        let requested = args.requested(direction);
        checks.extend(device_checks(
            direction,
            requested,
            &coppa_audio::probe_device(direction, requested, rate),
        ));
    }
    checks.extend(if args.no_rigctld {
        skipped_rigctld_checks()
    } else {
        rigctld_checks(&args.rigctld)
    });
    print!("{}", render(backend, &listing, &checks));
    verdict(&checks)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    fn check(
        name: &'static str,
        outcome: Outcome,
        detail: &str,
        hint: Option<&'static str>,
    ) -> Check {
        Check::new(name, outcome, detail, hint)
    }

    fn sample_checks() -> Vec<Check> {
        vec![
            check("audio backend", Outcome::Pass, "ALSA", None),
            check("audio devices", Outcome::Pass, "1 found", None),
            check("input device", Outcome::Pass, "default", None),
            check(
                "input 48 kHz mono",
                Outcome::Fail,
                "busy",
                Some(INPUT_FORMAT_HINT),
            ),
            check("output device", Outcome::Pass, "default", None),
            check(
                "output 48 kHz mono",
                Outcome::Fail,
                "busy",
                Some(OUTPUT_FORMAT_HINT),
            ),
            check(
                "rigctld connection",
                Outcome::Fail,
                "refused",
                Some(RIG_CONNECTION_HINT),
            ),
            check("rig CAT read", Outcome::Skip, "no connection", None),
        ]
    }

    #[test]
    fn table_has_a_header_and_aligned_columns_in_check_order() {
        let report = render(Some("ALSA"), &Ok(vec![]), &sample_checks());
        let lines: Vec<_> = report
            .lines()
            .filter(|line| {
                line.contains("PASS")
                    || line.contains("FAIL")
                    || line.contains("SKIP")
                    || line.starts_with("CHECK")
            })
            .collect();
        assert!(lines[0].starts_with("CHECK"));
        assert!(lines[1].starts_with("audio backend"));
        assert!(lines[8].starts_with("rig CAT read"));
        assert_eq!(lines[0].find("RESULT"), lines[1].find("PASS"));
        assert_eq!(lines[0].find("DETAIL"), lines[1].find("ALSA"));
    }

    #[test]
    fn hints_list_only_failed_checks_under_to_fix() {
        let report = render(Some("ALSA"), &Ok(vec![]), &sample_checks());
        let hints = report.split("To fix:\n").nth(1).unwrap();
        assert!(hints.contains("input 48 kHz mono: Coppa records"));
        assert!(hints.contains("output 48 kHz mono: Coppa plays"));
        assert!(hints.contains("rigctld connection: Start rigctld"));
        assert!(!hints.contains("audio backend:"));
    }

    #[test]
    fn no_to_fix_block_when_nothing_failed() {
        assert!(!render(None, &Ok(vec![]), &skipped_rigctld_checks()).contains("To fix:"));
    }

    #[test]
    fn summary_counts_each_outcome() {
        assert!(render(None, &Ok(vec![]), &sample_checks())
            .contains("Summary: 4 passed, 3 failed, 1 skipped"));
    }

    #[test]
    fn verdict_is_an_error_naming_the_failure_count() {
        assert_eq!(
            verdict(&sample_checks()).unwrap_err().to_string(),
            "3 of 8 checks failed"
        );
        assert!(verdict(&skipped_rigctld_checks()).is_ok());
    }

    #[test]
    fn control_characters_in_names_and_details_are_escaped() {
        let listing = vec![DeviceSupport {
            name: "USB\u{1b}[31m\n".into(),
            id: "id\r".into(),
            input: None,
            output: None,
        }];
        let report = render(
            Some("ALSA"),
            &Ok(listing),
            &[check("input device", Outcome::Fail, "bad\nthing", None)],
        );
        assert!(report.contains("USB\\u{1b}[31m\\n"));
        assert!(report.contains("id\\r"));
        assert!(report.contains("bad\\nthing"));
    }

    #[test]
    fn device_listing_shows_support_per_direction() {
        let listing = vec![DeviceSupport {
            name: "Card".into(),
            id: "alsa:card".into(),
            input: Some(Support::Yes),
            output: Some(Support::No("no 1-channel mode; offers 2 ch".into())),
        }];
        let report = render(Some("ALSA"), &Ok(listing), &[]);
        assert!(report.contains("input:  mono 48 kHz yes"));
        assert!(report.contains("output:  mono 48 kHz no (no 1-channel mode; offers 2 ch)"));
    }

    #[test]
    fn device_listing_without_backend_or_devices() {
        assert!(render(None, &Ok(vec![]), &[])
            .contains("Audio devices: no audio backend in this build\n  (none found)"));
    }

    #[test]
    fn backend_row_passes_with_the_cpal_feature() {
        assert_eq!(backend_check(true, Some("ALSA")).outcome, Outcome::Pass);
        assert_eq!(backend_check(true, Some("ALSA")).detail, "ALSA");
    }

    #[test]
    fn backend_row_fails_without_the_cpal_feature() {
        let row = backend_check(false, Some("ALSA"));
        assert_eq!(row.outcome, Outcome::Fail);
        assert!(row.hint.unwrap().contains("--features cpal-backend"));
    }

    #[test]
    fn devices_row_fails_on_none_found_and_on_error_and_skips_without_backend() {
        assert_eq!(
            devices_check(Some("ALSA"), &Ok(vec![])).outcome,
            Outcome::Fail
        );
        assert_eq!(
            devices_check(Some("ALSA"), &Err(anyhow::anyhow!("offline"))).detail,
            "could not list devices: offline"
        );
        assert_eq!(devices_check(None, &Ok(vec![])).outcome, Outcome::Skip);
    }

    #[test]
    fn a_resolved_default_that_opens_passes_both_rows() {
        let rows = device_checks(
            Direction::Input,
            None,
            &DeviceProbe::Opened {
                device: "Default Audio Device".into(),
            },
        );
        assert_eq!(rows[0].detail, "system default: Default Audio Device");
        assert_eq!(rows[1].detail, "opened 1 ch f32 at 48000 Hz (not started)");
        assert_eq!(rows[1].outcome, Outcome::Pass);
    }

    #[test]
    fn a_matched_name_is_shown_with_the_device_it_matched() {
        let rows = device_checks(
            Direction::Output,
            Some("CODEC"),
            &DeviceProbe::Opened {
                device: "USB Audio CODEC".into(),
            },
        );
        assert_eq!(rows[0].detail, "\"CODEC\" matched USB Audio CODEC");
    }

    #[test]
    fn an_open_failure_fails_with_the_error_and_the_advertised_reason() {
        let rows = device_checks(
            Direction::Input,
            None,
            &DeviceProbe::OpenFailed {
                device: "Card".into(),
                error: "busy".into(),
                advertised: Some(Support::No(
                    "no f32 mono mode at 48000 Hz (formats: i16)".into(),
                )),
            },
        );
        assert_eq!(rows[0].outcome, Outcome::Pass);
        assert!(rows[1]
            .detail
            .contains("busy; device offers no f32 mono mode"));
    }

    #[test]
    fn an_unmatched_name_fails_the_device_row_and_skips_the_format_row() {
        let rows = device_checks(
            Direction::Input,
            Some("CODEC"),
            &DeviceProbe::NoMatch {
                available: vec!["USB Card".into()],
            },
        );
        assert_eq!(rows[0].outcome, Outcome::Fail);
        assert!(rows[0]
            .detail
            .contains("no input device name contains \"CODEC\"; available: \"USB Card\""));
        assert_eq!(rows[1].outcome, Outcome::Skip);
    }

    #[test]
    fn no_default_device_fails_the_device_row_and_skips_the_format_row() {
        let rows = device_checks(Direction::Input, None, &DeviceProbe::NoDefault);
        assert_eq!(rows[0].detail, "the system reports no default input device");
        assert_eq!(rows[1].outcome, Outcome::Skip);
    }

    #[test]
    fn without_a_backend_both_device_rows_skip() {
        let rows = device_checks(Direction::Input, None, &DeviceProbe::NoBackend);
        assert!(rows.iter().all(|row| row.outcome == Outcome::Skip));
    }

    #[test]
    fn input_and_output_device_flags_override_device() {
        let args = DoctorArgs {
            device: Some("both".into()),
            input_device: Some("in".into()),
            output_device: Some("out".into()),
            ..Default::default()
        };
        assert_eq!(args.requested(Direction::Input), Some("in"));
        assert_eq!(args.requested(Direction::Output), Some("out"));
    }

    #[test]
    fn doctor_checks_the_engines_sample_rate() {
        assert_eq!(
            coppa_engine::config::EngineConfig::default().sample_rate,
            48_000
        );
        for name in coppa_engine::profiles::list_profiles() {
            let profile = coppa_engine::profiles::get_profile(name).unwrap();
            assert_eq!(
                coppa_engine::config::EngineConfig::from_profile(profile).sample_rate,
                48_000
            );
        }
    }

    fn fake(
        answer: &'static str,
    ) -> (String, Arc<Mutex<Vec<String>>>, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let commands = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&commands);
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            while let Ok(n) = {
                let mut line = String::new();
                let n = reader.read_line(&mut line);
                if let Ok(n) = n {
                    if n > 0 {
                        log.lock().unwrap().push(line.trim_end().into());
                    }
                }
                n
            } {
                if n == 0 {
                    break;
                }
                stream.write_all(answer.as_bytes()).unwrap();
            }
        });
        (addr, commands, handle)
    }

    #[test]
    fn rigctld_rows_pass_with_the_dial_frequency() {
        let (addr, _, handle) = fake("14074000\n");
        let rows = rigctld_checks(&addr);
        handle.join().unwrap();
        assert_eq!(rows[0].detail, format!("connected to {addr}"));
        assert_eq!(rows[1].detail, "dial 14.074000 MHz");
    }

    #[test]
    fn rigctld_rows_send_only_f() {
        let (addr, commands, handle) = fake("14074000\n");
        let _ = rigctld_checks(&addr);
        handle.join().unwrap();
        assert_eq!(*commands.lock().unwrap(), ["f"]);
    }

    #[test]
    fn a_rig_that_does_not_answer_fails_cat_read_with_rigctld_answer_and_sends_no_release() {
        let (addr, commands, handle) = fake("RPRT -5\n");
        let rows = rigctld_checks(&addr);
        handle.join().unwrap();
        assert_eq!(rows[1].outcome, Outcome::Fail);
        assert!(rows[1].detail.contains("RPRT -5"));
        assert_eq!(*commands.lock().unwrap(), ["f"]);
    }

    #[test]
    fn nothing_listening_fails_connection_and_skips_cat_read() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        drop(listener);
        let rows = rigctld_checks(&addr);
        assert_eq!(rows[0].outcome, Outcome::Fail);
        assert_eq!(rows[1].outcome, Outcome::Skip);
    }

    #[test]
    fn rigctld_detail_gets_the_address_only_when_missing() {
        let rows = rigctld_checks("127.0.0.1:notaport");
        assert!(rows[0].detail.starts_with("127.0.0.1:notaport:"));
    }

    #[test]
    fn disabled_rigctld_skips_both_rows_with_the_flag_name() {
        assert!(skipped_rigctld_checks()
            .iter()
            .all(|row| row.outcome == Outcome::Skip && row.detail == "--no-rigctld"));
    }
}
