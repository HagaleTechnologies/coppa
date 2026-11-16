use super::Host;
use anyhow::Result;
use std::time::Duration;

pub fn version(host: &mut Host) -> Result<String> {
    host.command("VERSION")?;
    host.wait_for("VERSION ", Duration::from_secs(5))
}
pub fn listen(host: &mut Host, call: &str) -> Result<()> {
    host.command("INITIALIZE")?;
    host.wait_for("INITIALIZE", Duration::from_secs(5))?;
    host.command(&format!("MYCALL {call}"))?;
    host.wait_for("MYCALL", Duration::from_secs(5))?;
    host.command("LISTEN TRUE")?;
    host.wait_for("LISTEN", Duration::from_secs(5))?;
    Ok(())
}
pub fn dial(host: &mut Host, own: &str, target: &str) -> Result<()> {
    host.command("INITIALIZE")?;
    host.wait_for("INITIALIZE", Duration::from_secs(5))?;
    host.command(&format!("MYCALL {own}"))?;
    host.wait_for("MYCALL", Duration::from_secs(5))?;
    host.command(&format!("ARQCALL {target}"))?;
    host.wait_for("CONNECTED", Duration::from_secs(120))?;
    Ok(())
}
pub fn send(host: &mut Host, bytes: &[u8]) -> Result<()> {
    let len = u16::try_from(bytes.len())?;
    let mut frame = Vec::with_capacity(bytes.len() + 2);
    frame.extend_from_slice(&len.to_be_bytes());
    frame.extend_from_slice(bytes);
    host.send_data(&frame)
}

pub fn drain_frames(wire: &mut Vec<u8>, payload: &mut Vec<u8>) -> Result<usize> {
    let mut count = 0;
    while wire.len() >= 2 {
        let len = u16::from_be_bytes([wire[0], wire[1]]) as usize;
        if wire.len() < len + 2 {
            break;
        }
        anyhow::ensure!(len >= 3, "invalid ARDOP data frame length {len}");
        // The data port also carries FEC and IDF frames; only ARQ frames
        // belong to the connected transfer.
        if &wire[2..5] == b"ARQ" {
            payload.extend_from_slice(&wire[5..len + 2]);
            count += 1;
        }
        wire.drain(..len + 2);
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    fn line(stream: &mut std::net::TcpStream) -> String {
        let mut bytes = Vec::new();
        let mut one = [0u8; 1];
        loop {
            stream.read_exact(&mut one).unwrap();
            if one[0] == b'\r' {
                break;
            }
            bytes.push(one[0]);
        }
        String::from_utf8(bytes).unwrap()
    }

    #[test]
    fn initialize_mycall_listen() {
        let cmd = TcpListener::bind("127.0.0.1:0").unwrap();
        let data = TcpListener::bind("127.0.0.1:0").unwrap();
        let caddr = cmd.local_addr().unwrap();
        let daddr = data.local_addr().unwrap();
        let worker = thread::spawn(move || {
            let (mut c, _) = cmd.accept().unwrap();
            let _d = data.accept().unwrap();
            for expected in ["INITIALIZE", "MYCALL N0BBB", "LISTEN TRUE"] {
                assert_eq!(line(&mut c), expected);
                c.write_all(format!("{expected}\r").as_bytes()).unwrap();
            }
        });
        let mut host = Host::connect(&caddr.to_string(), &daddr.to_string()).unwrap();
        listen(&mut host, "N0BBB").unwrap();
        worker.join().unwrap();
    }

    #[test]
    fn arqcall_reports_connected() {
        let cmd = TcpListener::bind("127.0.0.1:0").unwrap();
        let data = TcpListener::bind("127.0.0.1:0").unwrap();
        let caddr = cmd.local_addr().unwrap();
        let daddr = data.local_addr().unwrap();
        let worker = thread::spawn(move || {
            let (mut c, _) = cmd.accept().unwrap();
            let _d = data.accept().unwrap();
            for expected in ["INITIALIZE", "MYCALL N0AAA"] {
                assert_eq!(line(&mut c), expected);
                c.write_all(format!("{expected}\r").as_bytes()).unwrap();
            }
            assert_eq!(line(&mut c), "ARQCALL N0BBB");
            c.write_all(b"NEWSTATE ISS\rCONNECTED N0BBB 500MAX\r")
                .unwrap();
        });
        let mut host = Host::connect(&caddr.to_string(), &daddr.to_string()).unwrap();
        dial(&mut host, "N0AAA", "N0BBB").unwrap();
        worker.join().unwrap();
    }

    #[test]
    fn disconnected_after_connect_is_a_drop() {
        let cmd = TcpListener::bind("127.0.0.1:0").unwrap();
        let data = TcpListener::bind("127.0.0.1:0").unwrap();
        let caddr = cmd.local_addr().unwrap();
        let daddr = data.local_addr().unwrap();
        let worker = thread::spawn(move || {
            let (mut c, _) = cmd.accept().unwrap();
            let _d = data.accept().unwrap();
            c.write_all(b"CONNECTED N0BBB 500MAX\rDISCONNECTED\r")
                .unwrap();
        });
        let mut host = Host::connect(&caddr.to_string(), &daddr.to_string()).unwrap();
        host.wait_for("CONNECTED", Duration::from_secs(1)).unwrap();
        assert_eq!(host.poll_line().unwrap().as_deref(), Some("DISCONNECTED"));
        worker.join().unwrap();
    }
    #[test]
    fn data_frames_carry_a_big_endian_length() {
        assert_eq!(2048u16.to_be_bytes(), [8, 0]);
    }
    #[test]
    fn tnc_frames_decode_across_reads() {
        let mut wire = vec![0, 5, b'A', b'R'];
        let mut payload = Vec::new();
        assert_eq!(drain_frames(&mut wire, &mut payload).unwrap(), 0);
        wire.extend_from_slice(b"Qxy");
        assert_eq!(drain_frames(&mut wire, &mut payload).unwrap(), 1);
        assert_eq!(payload, b"xy");
    }
    #[test]
    fn idf_and_fec_frames_are_skipped() {
        let mut wire = Vec::new();
        for (tag, body) in [
            (&b"IDF"[..], &b"N0AAA"[..]),
            (b"ARQ", b"data"),
            (b"FEC", b"x"),
        ] {
            wire.extend_from_slice(&((tag.len() + body.len()) as u16).to_be_bytes());
            wire.extend_from_slice(tag);
            wire.extend_from_slice(body);
        }
        let mut payload = Vec::new();
        assert_eq!(drain_frames(&mut wire, &mut payload).unwrap(), 1);
        assert_eq!(payload, b"data");
        assert!(wire.is_empty());
    }
}
