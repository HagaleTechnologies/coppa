//! Linux-only black-box execution path.
use super::{
    cable::Cable,
    guard, integrity,
    native::{self, Capture, NativeFormat, SampleFormat},
    profile::Profile,
    relay::io::Relay,
    resume::{self, CsvIdentity},
    score,
    station::Station,
    tnc::{self, Host},
    BlackboxArgs,
};
use anyhow::{bail, Context, Result};
use coppa_audio::{AudioSink, AudioSource, CpalSink, CpalSource};
use coppa_channel::watterson::WattersonPreset;
use std::{
    collections::HashSet,
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::Ordering,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use super::CSV_HEADER as HEADER;

fn scratch() -> Result<PathBuf> {
    let dir = std::env::temp_dir().join(format!(
        "coppa-compare-{}-{}",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
    ));
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

pub fn probe_child() -> Result<()> {
    let out = PathBuf::from(
        std::env::var_os("COPPA_COMPARE_PROBE_OUT")
            .context("COPPA_COMPARE_PROBE_OUT is required")?,
    );
    let native = NativeFormat::from_env(
        &std::env::var("COPPA_COMPARE_PROBE_FORMAT")
            .context("COPPA_COMPARE_PROBE_FORMAT is required")?,
    )?;
    let output = coppa_audio::resolve_output_device(Some("coppa-cmp-A"), None)?
        .context("probe output device unavailable")?;
    let input = coppa_audio::resolve_input_device(Some("coppa-cmp-B"), None)?
        .context("probe input device unavailable")?;
    let dir = out.parent().context("probe output has no parent")?;
    let captured = if native == NativeFormat::CHANNEL {
        probe_f32_48k(output, input, dir)?
    } else {
        probe_native(output, input, native, dir)?
    };
    let mut file = fs::File::create(out)?;
    for sample in captured {
        file.write_all(&sample.to_le_bytes())?;
    }
    Ok(())
}

/// COP-214's probe, through coppa-audio's f32 wrappers as coppad opens them.
fn probe_f32_48k(output: cpal::Device, input: cpal::Device, dir: &Path) -> Result<Vec<f32>> {
    let mut sink = CpalSink::from_device(output, 48000, 16384)?;
    let mut source = CpalSource::from_device(input, 48000, 16384)?;
    source.start()?;
    sink.start()?;
    // Built after the streams start, as COP-214 shipped it: the first
    // writes then land after the file PCMs' startup, not inside it.
    let signal = NativeFormat::CHANNEL.probe_signal();
    let mut captured = Vec::new();
    let mut sent = 0usize;
    fs::write(dir.join("probe-start"), b"")?;
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(12) {
        if sent < signal.len() {
            let end = (sent + 960).min(signal.len());
            sent += sink.write(&signal[sent..end])?;
        }
        if sent == signal.len() && !dir.join("probe-end").exists() {
            fs::write(dir.join("probe-end"), b"")?;
        }
        let mut block = [0f32; 4096];
        let n = source.read(&mut block)?;
        captured.extend_from_slice(&block[..n]);
        thread::sleep(Duration::from_millis(10));
    }
    // ALSA's file PCM does not implement pause; dropping the streams closes it.
    let _ = sink.stop();
    let _ = source.stop();
    Ok(captured)
}

/// COP-221: opens both devices in the modem's declared format and rate, as
/// the modem would. coppa-audio's wrappers open f32 only, so this builds
/// mono cpal streams directly.
fn probe_native(
    output: cpal::Device,
    input: cpal::Device,
    native: NativeFormat,
    dir: &Path,
) -> Result<Vec<f32>> {
    use cpal::traits::{DeviceTrait, StreamTrait};
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    let config = cpal::StreamConfig {
        channels: 1,
        sample_rate: native.rate,
        buffer_size: cpal::BufferSize::Default,
    };
    // Empty until both streams run, so the output plays silence through the
    // file PCMs' startup; the f32 probe lost its first samples there.
    let pending = Arc::new(Mutex::new(VecDeque::new()));
    let captured = Arc::new(Mutex::new(Vec::new()));
    let (queue, store) = (Arc::clone(&pending), Arc::clone(&captured));
    let format = format!("{} {} Hz mono", native.format.name(), native.rate);
    let (play, record) = match native.format {
        SampleFormat::S16le => (
            output.build_output_stream::<i16, _, _>(
                config,
                move |data: &mut [i16], _: &cpal::OutputCallbackInfo| {
                    let mut queue = queue.lock().unwrap();
                    for sample in data {
                        *sample = native::to_i16(queue.pop_front().unwrap_or(0.0));
                    }
                },
                |e| eprintln!("integrity probe: output error: {e}"),
                None,
            ),
            input.build_input_stream::<i16, _, _>(
                config,
                move |data: &[i16], _: &cpal::InputCallbackInfo| {
                    let mut store = store.lock().unwrap();
                    store.extend(data.iter().map(|&x| f32::from(x) / 32768.0));
                },
                |e| eprintln!("integrity probe: input error: {e}"),
                None,
            ),
        ),
        SampleFormat::F32le => (
            output.build_output_stream::<f32, _, _>(
                config,
                move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                    let mut queue = queue.lock().unwrap();
                    for sample in data {
                        *sample = queue.pop_front().unwrap_or(0.0);
                    }
                },
                |e| eprintln!("integrity probe: output error: {e}"),
                None,
            ),
            input.build_input_stream::<f32, _, _>(
                config,
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    store.lock().unwrap().extend_from_slice(data);
                },
                |e| eprintln!("integrity probe: input error: {e}"),
                None,
            ),
        ),
    };
    let play = play.with_context(|| format!("opening the probe output as {format}"))?;
    let record = record.with_context(|| format!("opening the probe input as {format}"))?;
    record.play().context("starting the probe input")?;
    play.play().context("starting the probe output")?;
    fs::write(dir.join("probe-start"), b"")?;
    let signal = native.probe_signal();
    pending.lock().unwrap().extend(signal);
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(12) {
        if pending.lock().unwrap().is_empty() && !dir.join("probe-end").exists() {
            fs::write(dir.join("probe-end"), b"")?;
        }
        thread::sleep(Duration::from_millis(10));
    }
    // As above: dropping the streams closes the file PCMs.
    drop(play);
    drop(record);
    let captured = std::mem::take(&mut *captured.lock().unwrap());
    Ok(captured)
}

fn probe(cable: &Cable, native: NativeFormat) -> Result<integrity::Metrics> {
    let relay = Relay::start(&cable.dir, native, None, None, [1.0, 1.0], 0)?;
    let home = cable.station_home('A')?;
    let output = cable.dir.join("probe.raw");
    let status = Command::new(std::env::current_exe()?)
        .args(["compare", "blackbox-probe"])
        .env("HOME", home)
        .env("COPPA_COMPARE_PROBE_OUT", &output)
        .env("COPPA_COMPARE_PROBE_FORMAT", native.to_env())
        .status()?;
    if !status.success() {
        bail!("integrity probe child failed ({status})");
    }
    let starved = relay.counts.starved.load(Ordering::Relaxed);
    let overflow = relay.counts.overflow.load(Ordering::Relaxed);
    relay.stop()?;
    let data = fs::read(output)?;
    let received: Vec<_> = data
        .chunks_exact(4)
        .map(|x| f32::from_le_bytes(x.try_into().unwrap()))
        .collect();
    integrity::analyse(
        &native.probe_signal(),
        &received,
        native.rate as usize,
        starved,
        overflow,
    )
}

pub fn run(args: &BlackboxArgs) -> Result<()> {
    let profile = Profile::load(&args.modem)?;
    let native = profile.audio.native()?;
    let sha = profile.verify_binary()?;
    let channel = super::super::grid::Channel::parse(&args.channel)
        .context("unknown channel (expected wgn, mpg or mpp)")?;
    let cell = super::super::grid::Cell {
        channel,
        snr_db: args.snr,
        size_bytes: args.size,
    };
    cell.index().context("cell is off the comparison grid")?;
    anyhow::ensure!(
        args.trials > 0 && args.trials <= 65536,
        "trials must be 1..=65536"
    );
    anyhow::ensure!(args.jobs > 0, "jobs must be positive");
    let out_dir = args
        .out_dir
        .clone()
        .unwrap_or_else(|| guard::default_out_dir(&profile.name));
    if profile.adapter == "kiss" {
        guard::check(&out_dir, "kiss", None)?;
    }
    // COP-227: a CSV from before the payload column is refused before any
    // modem starts; the payload of each row is checked after calibration.
    let path = out_dir.join(format!("blackbox-{}.csv", profile.name));
    match fs::read_to_string(&path) {
        Ok(existing) => {
            resume::check_header(&existing)
                .with_context(|| format!("checking {}", path.display()))?;
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    }
    let root = scratch()?;
    let result = super::gated(
        || probe(&Cable::create(&root.join("probe"))?, native),
        |metrics| run_trials(args, &profile, &sha, &out_dir, &root, cell, &metrics),
    );
    if result.is_ok() {
        let _ = fs::remove_dir_all(&root);
    } else {
        eprintln!("compare blackbox: scratch files kept in {}", root.display());
    }
    result
}

fn run_trials(
    args: &BlackboxArgs,
    profile: &Profile,
    sha: &str,
    out_dir: &Path,
    root: &Path,
    cell: super::super::grid::Cell,
    metrics: &integrity::Metrics,
) -> Result<()> {
    let channel = cell.channel;
    let native = profile.audio.native()?;
    let (power, version) = calibrate(profile, root, args.ref_power.as_deref())?;
    // COP-256: one fixed gain for the whole trial, derived from the
    // calibrated powers, the cell and the native capture rate, and copied
    // into every trial context of the batch.
    let capture = Capture::trial(native, channel.preset().is_some(), args.snr, power);
    guard::check(out_dir, &profile.adapter, version.as_deref())?;
    let path = out_dir.join(format!("blackbox-{}.csv", profile.name));
    fs::create_dir_all(out_dir)?;
    if !path.exists() {
        fs::write(&path, format!("{HEADER}\n"))?;
    }
    let existing = fs::read_to_string(&path)?;
    let recorded = resume::recorded_cells(
        &existing,
        &CsvIdentity {
            profile,
            sha,
            version: version.as_deref().unwrap_or(""),
            seed_base: args.seed_base,
            power,
            capture_gain_db: capture.gain_db,
            requested: (channel, args.snr, args.size),
        },
    )
    .with_context(|| format!("checking {}", path.display()))?;
    let mut pending = Vec::new();
    for trial in 0..args.trials {
        let seed = cell
            .seed(args.seed_base, trial as usize)
            .context("invalid trial seed")?;
        if recorded.contains(&(channel.name().to_string(), args.snr, args.size, trial)) {
            continue;
        }
        pending.push((trial, seed));
    }
    for batch in pending.chunks(args.jobs) {
        let context = TrialContext {
            profile,
            root,
            channel,
            args,
            power,
            capture,
            lead_ms: metrics.cable_latency_ms,
            sha,
            version: version.as_deref().unwrap_or(""),
        };
        let results: Vec<Result<String>> = thread::scope(|scope| {
            let handles: Vec<_> = batch
                .iter()
                .map(|&(trial, seed)| scope.spawn(move || run_trial(context, trial, seed)))
                .collect();
            handles
                .into_iter()
                .map(|handle| {
                    handle
                        .join()
                        .map_err(|_| anyhow::anyhow!("trial worker panicked"))?
                })
                .collect()
        });
        // Record every finished trial of the batch before reporting a failed one.
        let mut file = fs::OpenOptions::new().append(true).open(&path)?;
        let mut first_error = None;
        for result in results {
            match result {
                Ok(line) => file.write_all(line.as_bytes())?,
                Err(e) => {
                    first_error.get_or_insert(e);
                }
            }
        }
        file.sync_all()?;
        if let Some(e) = first_error {
            return Err(e);
        }
    }
    eprintln!("compare blackbox: wrote {}", path.display());
    Ok(())
}

fn calibrate(
    profile: &Profile,
    root: &Path,
    specified: Option<&str>,
) -> Result<([f32; 2], Option<String>)> {
    let cable = Cable::create(&root.join("calibration"))?;
    let home_a = cable.station_home('A')?;
    let home_b = cable.station_home('B')?;
    let relay = Relay::start(
        &cable.dir,
        profile.audio.native()?,
        None,
        None,
        [1.0, 1.0],
        0,
    )?;
    let a = Station::launch(profile, &cable.dir.join("station-a"), &home_a, 'A')?;
    let b = Station::launch(profile, &cable.dir.join("station-b"), &home_b, 'B')?;
    let mut host_a = Host::connect(&a.command_addr, &a.data_addr)?;
    let mut host_b = Host::connect(&b.command_addr, &b.data_addr)?;
    let version = match profile.adapter.as_str() {
        "vara" => Some(tnc::vara::version(&mut host_a)?),
        "ardop" => Some(tnc::ardop::version(&mut host_a)?),
        _ => None,
    }
    // The reply is a CSV field; a comma or quote in it would shift columns.
    .map(|reply| reply.replace([',', '"'], ";"));
    let power = if let Some(pair) = specified {
        let (x, y) = pair.split_once(',').context("--ref-power expects A,B")?;
        [x.parse::<f32>()?, y.parse::<f32>()?]
    } else {
        if profile.adapter == "kiss" {
            relay.counts.reset_power();
            let frame = tnc::kiss::frame(&[0x55; 200]);
            host_a.send_data(&frame)?;
            thread::sleep(Duration::from_secs(4));
            host_b.send_data(&frame)?;
            thread::sleep(Duration::from_secs(4));
        } else {
            match profile.adapter.as_str() {
                "vara" => {
                    tnc::vara::listen(&mut host_b, "N0BBB")?;
                    tnc::vara::dial(&mut host_a, "N0AAA", "N0BBB")?;
                }
                "ardop" => {
                    tnc::ardop::listen(&mut host_b, "N0BBB")?;
                    tnc::ardop::dial(&mut host_a, "N0AAA", "N0BBB")?;
                }
                _ => unreachable!(),
            }
            relay.counts.reset_power();
            let frame = [0x55u8; 40];
            if profile.adapter == "ardop" {
                tnc::ardop::send(&mut host_a, &frame)?;
            } else {
                host_a.send_data(&frame)?;
            }
            let mut received = [0u8; 4096];
            let deadline = Instant::now() + Duration::from_secs(90);
            let mut got = false;
            while Instant::now() < deadline && !relay.is_dead() {
                if host_b.receive_data(&mut received)? > 0 {
                    got = true;
                    break;
                }
            }
            anyhow::ensure!(got, "calibration A-to-B data frame was not delivered");
            if profile.adapter == "ardop" {
                tnc::ardop::send(&mut host_b, &frame)?;
            } else {
                host_b.send_data(&frame)?;
            }
            let deadline = Instant::now() + Duration::from_secs(90);
            let mut got = false;
            while Instant::now() < deadline && !relay.is_dead() {
                if host_a.receive_data(&mut received)? > 0 {
                    got = true;
                    break;
                }
            }
            anyhow::ensure!(got, "calibration B-to-A data frame was not delivered");
        }
        relay.counts.ref_power()
    };
    if relay.is_dead() {
        relay.stop()?;
        bail!("the cable relay stopped during calibration");
    }
    anyhow::ensure!(
        power.iter().all(|p| p.is_finite() && *p > 0.0),
        "calibration found no active TX power"
    );
    relay.stop()?;
    Ok((power, version))
}

#[derive(Clone, Copy)]
struct TrialContext<'a> {
    profile: &'a Profile,
    root: &'a Path,
    channel: super::super::grid::Channel,
    args: &'a BlackboxArgs,
    power: [f32; 2],
    /// COP-256: the fixed capture gain, one value for the whole trial.
    capture: Capture,
    lead_ms: f64,
    sha: &'a str,
    version: &'a str,
}

fn run_trial(context: TrialContext<'_>, trial: u32, seed: u64) -> Result<String> {
    let TrialContext {
        profile,
        root,
        channel,
        args,
        power,
        capture,
        lead_ms,
        sha,
        version,
    } = context;
    // Built before the dial, so a no_connect row also records its payload.
    let payload = score::trial_payload(seed, args.size);
    if let Some(datagram_bytes) = profile
        .datagram_bytes
        .filter(|&size| profile.adapter == "kiss" && size > 0)
    {
        // The pacer and scorer identify datagrams by their bytes.
        let mut seen = HashSet::new();
        anyhow::ensure!(
            payload.chunks(datagram_bytes).all(|chunk| seen.insert(chunk)),
            "trial {trial} (seed 0x{seed:x}) has identical KISS payload datagrams; change datagram_bytes"
        );
    }
    let cable = Cable::create(&root.join(format!("trial-{trial}")))?;
    let home_a = cable.station_home('A')?;
    let home_b = cable.station_home('B')?;
    let fading = channel.preset().map(WattersonPreset::config);
    let relay = Relay::start_capture(&cable.dir, capture, fading, Some(args.snr), power, seed)?;
    let a = Station::launch(profile, &cable.dir.join("station-a"), &home_a, 'A')?;
    let b = Station::launch(profile, &cable.dir.join("station-b"), &home_b, 'B')?;
    let mut host_a = Host::connect(&a.command_addr, &a.data_addr)?;
    let mut host_b = Host::connect(&b.command_addr, &b.data_addr)?;
    let start = Instant::now();
    // COP-214 decision 4: starved and overflow ticks count from the dial,
    // the window wall_s covers, so first acquisition is inside the counts.
    relay.counts.counting.store(true, Ordering::Relaxed);
    let mut connect_s = String::new();
    let mut delivered = 0usize;
    let cap_s = if args.size == 2048 { 600 } else { 1800 };
    let mut outcome = "no_connect";
    let mut transfer_s = 0.0;
    let mut datagrams = 0usize;
    let mut exact = false;
    match profile.adapter.as_str() {
        "vara" => {
            tnc::vara::listen(&mut host_b, "N0BBB")?;
            if tnc::vara::dial(&mut host_a, "N0AAA", "N0BBB").is_ok() {
                connect_s = format!("{:.3}", start.elapsed().as_secs_f64());
                outcome = "cap";
            }
        }
        "ardop" => {
            tnc::ardop::listen(&mut host_b, "N0BBB")?;
            if tnc::ardop::dial(&mut host_a, "N0AAA", "N0BBB").is_ok() {
                connect_s = format!("{:.3}", start.elapsed().as_secs_f64());
                outcome = "cap";
            }
        }
        "kiss" => {
            outcome = "datagram";
        }
        _ => unreachable!(),
    }
    if outcome != "no_connect" {
        let write_time = Instant::now();
        let mut pacer = (profile.adapter == "kiss").then(|| {
            tnc::kiss::Pacer::new(
                &payload,
                profile.datagram_bytes.unwrap(),
                tnc::kiss::WINDOW,
                tnc::kiss::PRESUME_LOST,
                write_time,
            )
        });
        match profile.adapter.as_str() {
            "vara" => host_a.send_data(&payload)?,
            "ardop" => tnc::ardop::send(&mut host_a, &payload)?,
            // The first window only; the loop below sends the rest as B accounts for them.
            "kiss" => {
                for frame in pacer.as_mut().unwrap().due(write_time) {
                    host_a.send_data(&frame)?;
                }
            }
            _ => unreachable!(),
        }
        let deadline = write_time + Duration::from_secs(cap_s);
        let mut received = Vec::new();
        // KISS: every decoded datagram, scored after the loop (COP-227). The
        // loop runs until the intact datagrams cover the payload, so a
        // corrupt, foreign or repeated datagram cannot end it early.
        let mut datagram_list: Vec<Vec<u8>> = Vec::new();
        let mut kiss_intact_bytes = 0usize;
        let mut wire = Vec::new();
        let mut answerer_connected = false;
        let kiss = profile.adapter == "kiss";
        while Instant::now() < deadline
            && (if kiss {
                kiss_intact_bytes
            } else {
                received.len()
            }) < payload.len()
            && !relay.is_dead()
        {
            let mut bytes = [0u8; 4096];
            let n = match host_b.receive_data(&mut bytes) {
                Ok(n) => n,
                Err(_) => {
                    outcome = "drop";
                    break;
                }
            };
            if n > 0 {
                match profile.adapter.as_str() {
                    "vara" => received.extend_from_slice(&bytes[..n]),
                    "ardop" => {
                        wire.extend_from_slice(&bytes[..n]);
                        tnc::ardop::drain_frames(&mut wire, &mut received)?;
                    }
                    "kiss" => {
                        wire.extend_from_slice(&bytes[..n]);
                        for datagram in tnc::kiss::drain_datagrams(&mut wire) {
                            if let Some(pacer) = pacer.as_mut() {
                                pacer.received(&datagram, Instant::now());
                            }
                            received.extend_from_slice(&datagram);
                            datagram_list.push(datagram);
                            kiss_intact_bytes = score::datagram_delivered(
                                &datagram_list,
                                &payload,
                                profile.datagram_bytes.unwrap(),
                            )
                            .0;
                        }
                    }
                    _ => unreachable!(),
                }
            }
            // A's data connection failing ends the trial, as B's does above.
            if let Some(pacer) = pacer.as_mut() {
                if pacer
                    .due(Instant::now())
                    .iter()
                    .any(|frame| host_a.send_data(frame).is_err())
                {
                    outcome = "drop";
                    break;
                }
            }
            // Either station's session ending ends the trial: B's data
            // connection stays open when B reports DISCONNECTED.
            if profile.adapter != "kiss"
                && tnc::session_ended(&mut host_a, &mut host_b, &mut answerer_connected)?
            {
                outcome = "drop";
                break;
            }
        }
        // COP-227: only bytes that match the payload count.
        if profile.adapter == "kiss" {
            let (bytes, intact) = score::datagram_delivered(
                &datagram_list,
                &payload,
                profile.datagram_bytes.unwrap(),
            );
            datagrams = intact;
            delivered = bytes;
        } else {
            delivered = score::stream_delivered(&received, &payload);
        }
        transfer_s = write_time.elapsed().as_secs_f64();
        exact = received == payload;
        if exact {
            outcome = if profile.adapter == "kiss" {
                "datagram"
            } else {
                "complete"
            };
        }
    }
    // Close the window on every path, no_connect included, before reading.
    relay.counts.counting.store(false, Ordering::Relaxed);
    let wall_s = start.elapsed().as_secs_f64();
    let starved = relay.counts.starved.load(Ordering::Relaxed);
    let overflow = relay.counts.overflow.load(Ordering::Relaxed);
    relay.stop()?;

    let rate = if transfer_s > 0.0 {
        delivered as f64 * 60.0 / transfer_s
    } else {
        0.0
    };
    let drops = if profile.adapter == "kiss" {
        tnc::kiss::lost_datagrams(args.size, profile.datagram_bytes.unwrap(), datagrams)
    } else {
        usize::from(outcome == "drop")
    };
    eprintln!(
        "compare blackbox: {channel:?} {trial}: {outcome}, {delivered} bytes, wall {wall_s:.1} s"
    );
    Ok(format!("{},{},{},{},{},{},{},{},{},0x{:x},{:.6},{:.6},{},{},{},{},{},{:.3},{:.3},{},{:.3},{:.0},{},{},{},{}\n",
        profile.name, profile.adapter, version, sha, profile.mode, channel.name(), args.snr, args.size, trial, seed,
        power[0], power[1], cap_s, outcome, connect_s, delivered, exact, transfer_s, rate, drops, wall_s, lead_ms, starved, overflow,
        score::payload_crc32(&payload), capture.gain_db))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compare::grid::Channel;

    #[test]
    fn kiss_trial_refuses_indistinguishable_payload_datagrams() {
        let modem = crate::compare::repo_root().join("tools/compare/coppad-kiss.toml");
        let mut profile = Profile::load(&modem).unwrap();
        profile.datagram_bytes = Some(1);
        let seed = 0x1700_0000;
        let args = BlackboxArgs {
            modem,
            channel: "wgn".into(),
            snr: 30,
            size: 2048,
            trials: 1,
            seed_base: seed,
            jobs: 1,
            ref_power: None,
            out_dir: None,
        };
        // A repeat of chunk zero has the same bytes as chunk 258. The
        // trial must refuse this payload before it tries to create a cable.
        let payload = score::trial_payload(seed, args.size);
        assert_eq!(payload[0], payload[258]);
        let context = TrialContext {
            profile: &profile,
            root: Path::new("/dev/null"), // A cable cannot be created here.
            channel: Channel::Wgn,
            args: &args,
            power: [1.0; 2],
            capture: Capture::unity(NativeFormat::CHANNEL),
            lead_ms: 0.0,
            sha: "",
            version: "",
        };
        let error = run_trial(context, 0, seed).unwrap_err().to_string();
        assert!(
            error.contains("identical KISS payload datagrams"),
            "{error}"
        );
    }
}
