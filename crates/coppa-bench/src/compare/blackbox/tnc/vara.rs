use super::Host;
use anyhow::Result;
use std::time::Duration;

pub fn version(host: &mut Host) -> Result<String> {
    host.command("VERSION")?;
    host.wait_for("VERSION ", Duration::from_secs(5))
}
pub fn listen(host: &mut Host, call: &str) -> Result<()> {
    host.command(&format!("MYCALL {call}"))?;
    host.wait_for("OK", Duration::from_secs(5))?;
    host.command("LISTEN ON")?;
    host.wait_for("OK", Duration::from_secs(5))?;
    Ok(())
}
pub fn dial(host: &mut Host, own: &str, target: &str) -> Result<()> {
    host.command(&format!("MYCALL {own}"))?;
    host.wait_for("OK", Duration::from_secs(5))?;
    host.command(&format!("CONNECT {own} {target}"))?;
    host.wait_for("CONNECTED", Duration::from_secs(120))?;
    Ok(())
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
    fn greeting_and_mycall() {
        let cmd = TcpListener::bind("127.0.0.1:0").unwrap();
        let data = TcpListener::bind("127.0.0.1:0").unwrap();
        let caddr = cmd.local_addr().unwrap();
        let daddr = data.local_addr().unwrap();
        let worker = thread::spawn(move || {
            let (mut c, _) = cmd.accept().unwrap();
            let _d = data.accept().unwrap();
            assert_eq!(line(&mut c), "VERSION");
            c.write_all(b"VERSION Coppa 0.16.0\r").unwrap();
            assert_eq!(line(&mut c), "MYCALL N0BBB");
            c.write_all(b"OK\r").unwrap();
            assert_eq!(line(&mut c), "LISTEN ON");
            c.write_all(b"OK\r").unwrap();
        });
        let mut host = Host::connect(&caddr.to_string(), &daddr.to_string()).unwrap();
        assert_eq!(version(&mut host).unwrap(), "VERSION Coppa 0.16.0");
        listen(&mut host, "N0BBB").unwrap();
        worker.join().unwrap();
    }

    #[test]
    fn connect_reports_connected_time() {
        let cmd = TcpListener::bind("127.0.0.1:0").unwrap();
        let data = TcpListener::bind("127.0.0.1:0").unwrap();
        let caddr = cmd.local_addr().unwrap();
        let daddr = data.local_addr().unwrap();
        let worker = thread::spawn(move || {
            let (mut c, _) = cmd.accept().unwrap();
            let _d = data.accept().unwrap();
            assert_eq!(line(&mut c), "MYCALL N0AAA");
            c.write_all(b"OK\r").unwrap();
            assert_eq!(line(&mut c), "CONNECT N0AAA N0BBB");
            c.write_all(b"OK\rCONNECTED N0AAA N0BBB 2300\r").unwrap();
        });
        let mut host = Host::connect(&caddr.to_string(), &daddr.to_string()).unwrap();
        let start = std::time::Instant::now();
        dial(&mut host, "N0AAA", "N0BBB").unwrap();
        assert!(start.elapsed() < Duration::from_secs(1));
        worker.join().unwrap();
    }
    #[test]
    fn lines_end_in_bare_cr() {
        let cmd = TcpListener::bind("127.0.0.1:0").unwrap();
        let data = TcpListener::bind("127.0.0.1:0").unwrap();
        let caddr = cmd.local_addr().unwrap();
        let daddr = data.local_addr().unwrap();
        let worker = thread::spawn(move || {
            let (mut c, _) = cmd.accept().unwrap();
            let _d = data.accept().unwrap();
            let mut b = [0u8; 8];
            c.read_exact(&mut b).unwrap();
            assert_eq!(&b, b"VERSION\r");
            c.write_all(b"VERSION Coppa 0.16.0\r").unwrap();
        });
        let mut host = Host::connect(&caddr.to_string(), &daddr.to_string()).unwrap();
        assert_eq!(version(&mut host).unwrap(), "VERSION Coppa 0.16.0");
        worker.join().unwrap();
    }

    #[test]
    fn ptt_and_buffer_lines_are_tolerated() {
        let cmd = TcpListener::bind("127.0.0.1:0").unwrap();
        let data = TcpListener::bind("127.0.0.1:0").unwrap();
        let caddr = cmd.local_addr().unwrap();
        let daddr = data.local_addr().unwrap();
        let worker = thread::spawn(move || {
            let (mut c, _) = cmd.accept().unwrap();
            let _d = data.accept().unwrap();
            c.write_all(b"PTT ON\rBUFFER 12\rCONNECTED N0AAA N0BBB 2300\r")
                .unwrap();
        });
        let mut host = Host::connect(&caddr.to_string(), &daddr.to_string()).unwrap();
        assert!(host
            .wait_for("CONNECTED", Duration::from_secs(1))
            .unwrap()
            .contains("N0BBB"));
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
            c.write_all(b"CONNECTED N0AAA N0BBB 2300\rDISCONNECTED\r")
                .unwrap();
        });
        let mut host = Host::connect(&caddr.to_string(), &daddr.to_string()).unwrap();
        host.wait_for("CONNECTED", Duration::from_secs(1)).unwrap();
        assert_eq!(host.poll_line().unwrap().as_deref(), Some("DISCONNECTED"));
        worker.join().unwrap();
    }

    #[test]
    fn missing_reply_times_out() {
        let cmd = TcpListener::bind("127.0.0.1:0").unwrap();
        let data = TcpListener::bind("127.0.0.1:0").unwrap();
        let caddr = cmd.local_addr().unwrap();
        let daddr = data.local_addr().unwrap();
        let worker = thread::spawn(move || {
            let _c = cmd.accept().unwrap();
            let _d = data.accept().unwrap();
            thread::sleep(Duration::from_millis(150));
        });
        let mut host = Host::connect(&caddr.to_string(), &daddr.to_string()).unwrap();
        assert!(host
            .wait_for("VERSION ", Duration::from_millis(20))
            .is_err());
        worker.join().unwrap();
    }
}
