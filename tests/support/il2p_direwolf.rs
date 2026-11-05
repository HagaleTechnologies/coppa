//! External oracle shared by the integration and private daemon adapter tests.
pub(crate) fn verify(encode: fn(&[u8]) -> Vec<f32>) {
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};
    let program = std::env::var_os("DIREWOLF").unwrap_or_else(|| "direwolf".into());
    let version = Command::new(&program)
        .arg("-h")
        .output()
        .expect("Direwolf 1.7 is required; install it or set DIREWOLF");
    let version = String::from_utf8_lossy(&version.stdout).to_string()
        + &String::from_utf8_lossy(&version.stderr);
    assert!(
        version.contains("version 1.7"),
        "requires Direwolf 1.7: {version}"
    );
    println!("oracle: Dire Wolf version 1.7");
    let dir = std::env::temp_dir().join(format!(
        "cop115-{}-{}",
        std::process::id(),
        TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    ));
    std::fs::create_dir(&dir).unwrap();
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let config = dir.join("direwolf.conf");
    std::fs::write(&config,format!("ADEVICE stdin null\nACHANNELS 1\nARATE 48000\nMYCALL N0CALL\nAGWPORT 0\nKISSPORT {port}\n")).unwrap();
    let log = std::fs::File::create(dir.join("direwolf.log")).unwrap();
    let child = Command::new(program)
        .args(["-c", config.to_str().unwrap(), "-t", "0", "-d", "22"])
        .stdin(Stdio::piped())
        .stdout(log.try_clone().unwrap())
        .stderr(log)
        .spawn()
        .unwrap();
    struct Guard(std::process::Child);
    impl Drop for Guard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let mut child = Guard(child);
    let start = Instant::now();
    let mut socket = loop {
        if let Ok(s) = TcpStream::connect(("127.0.0.1", port)) {
            break s;
        }
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "Direwolf KISS not ready; log {}",
            dir.display()
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    socket
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let mut stdin = child.0.stdin.take().unwrap();
    // Keep the pipe open until all KISS frames arrive. EOF otherwise ends the
    // decoder before its delayed packet selection has delivered the last frame.
    let names = ["ui-max", "transparent", "maximum"];
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let root = if root.join("testdata").exists() {
        root.to_path_buf()
    } else {
        root.join("../..")
    };
    let expected: Vec<Vec<u8>> = names
        .iter()
        .map(|name| std::fs::read(root.join(format!("testdata/il2p/{name}.ax25"))).unwrap())
        .collect();
    let audio: Vec<u8> = expected
        .iter()
        .flat_map(|f| encode(f))
        .flat_map(|s| ((s * 16000.0) as i16).to_le_bytes())
        .collect();
    let writer = std::thread::spawn(move || {
        stdin.write_all(&audio).unwrap();
        stdin
    });
    let mut frames = Vec::new();
    let (mut frame, mut escaped) = (Vec::new(), false);
    while frames.len() < expected.len() {
        let mut byte = [0];
        socket
            .read_exact(&mut byte)
            .expect("Direwolf did not deliver expected KISS frames");
        match byte[0] {
            0xc0 => {
                if frame.first() == Some(&0) {
                    frames.push(frame[1..].to_vec());
                }
                frame.clear();
                escaped = false;
            }
            0xdb => escaped = true,
            b => {
                frame.push(if escaped {
                    match b {
                        0xdc => 0xc0,
                        0xdd => 0xdb,
                        _ => panic!("invalid KISS escape"),
                    }
                } else {
                    b
                });
                escaped = false;
            }
        }
    }
    let stdin = writer.join().unwrap();
    assert_eq!(frames, expected);
    for (name, frame) in names.iter().zip(&frames) {
        println!("{name}: {} KISS bytes match", frame.len());
    }
    std::thread::sleep(Duration::from_millis(200));
    drop(stdin);
    drop(child);
    let log_bytes = std::fs::read(dir.join("direwolf.log")).unwrap();
    let log = String::from_utf8_lossy(&log_bytes);
    assert!(!log.contains("FAILED") && !log.contains("INVALID"), "{log}");
    // Debug level 2 reports each header correction and the final chosen
    // packet's correction count. Inspect every reported count, not monitor text.
    for line in log.lines().filter(|l| l.contains("after correcting")) {
        assert!(line.contains("correcting 0 symbols"), "{line}");
    }
    let reports: Vec<_> = log
        .lines()
        .filter(|l| l.contains("audio level =") && l.contains("FX.25"))
        .collect();
    assert_eq!(reports.len(), 3, "missing Direwolf FEC status: {log}");
    for line in reports {
        let spectrum = line.split_whitespace().last().unwrap();
        assert!(
            spectrum.contains('0') && spectrum.chars().all(|c| c == '0' || c == '_'),
            "nonzero FEC correction count: {line}"
        );
    }
    println!("Direwolf log: {}", dir.join("direwolf.log").display());
}
