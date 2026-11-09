//! Connectionless AGWPE adapter for the KISS TNC (AFSK 1200 or G3RUH 9600).
//! Frames exclude FCS.
use crate::HostError;
use std::{
    collections::HashSet,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{broadcast, mpsc, Semaphore},
};
const HEADER: usize = 36;
const MAX_BODY: usize = 1024;
const TX_CAP: usize = 256;

fn packet(kind: u8, source: &[u8], dest: &[u8], pid: u8, body: &[u8]) -> Vec<u8> {
    let mut out = vec![0; HEADER];
    out[4] = kind;
    out[6] = pid;
    out[8..8 + source.len().min(10)].copy_from_slice(&source[..source.len().min(10)]);
    out[18..18 + dest.len().min(10)].copy_from_slice(&dest[..dest.len().min(10)]);
    out[28..32].copy_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(body);
    out
}
fn call(bytes: &[u8]) -> Option<String> {
    let end = bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len());
    if bytes[end..].iter().any(|b| *b != 0) {
        return None;
    }
    let text = std::str::from_utf8(&bytes[..end])
        .ok()?
        .to_ascii_uppercase();
    let mut parts = text.split('-');
    let base = parts.next()?;
    if base.is_empty() || base.len() > 6 || !base.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return None;
    }
    let ssid = match parts.next() {
        None => 0,
        Some(s) => {
            if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            s.parse::<u8>().ok()?
        }
    };
    if ssid > 15 || parts.next().is_some() {
        return None;
    }
    Some(if ssid == 0 {
        base.to_string()
    } else {
        format!("{base}-{ssid}")
    })
}
fn address(text: &str, last: bool) -> Vec<u8> {
    let (base, ssid) = text
        .split_once('-')
        .map_or((text, 0), |(a, b)| (a, b.parse::<u8>().unwrap_or(0)));
    let mut out = vec![b' ' << 1; 6];
    for (i, b) in base.bytes().enumerate() {
        out[i] = b << 1;
    }
    out.push(0x60 | (ssid << 1) | u8::from(last));
    out
}
fn ui(header: &[u8], body: &[u8]) -> Option<Vec<u8>> {
    let src = call(&header[8..18])?;
    let dst = call(&header[18..28])?;
    let mut paths = Vec::new();
    let mut info = body;
    if header[4] == b'V' {
        let count = usize::from(*body.first()?);
        if count > 8 || body.len() < 1 + count * 10 {
            return None;
        }
        for chunk in body[1..1 + count * 10].chunks_exact(10) {
            paths.push(call(chunk)?);
        }
        info = &body[1 + count * 10..];
    }
    if info.len() > 256 {
        return None;
    }
    let mut out = address(&dst, false);
    out.extend(address(&src, paths.is_empty()));
    for (i, p) in paths.iter().enumerate() {
        out.extend(address(p, i + 1 == paths.len()));
    }
    out.extend([3, header[6]]);
    out.extend(info);
    Some(out)
}
fn monitor(frame: &[u8], time: &str) -> Option<Vec<u8>> {
    let mut calls = Vec::new();
    let mut marks = Vec::new();
    let mut offset = 0;
    loop {
        let a = frame.get(offset..offset + 7)?;
        if calls.len() == 10 || a[..6].iter().any(|b| b & 1 != 0) {
            return None;
        }
        let text: Vec<u8> = a[..6].iter().map(|b| b >> 1).collect();
        let base = std::str::from_utf8(&text).ok()?.trim_end();
        let ssid = (a[6] >> 1) & 15;
        let name = if ssid == 0 {
            base.to_string()
        } else {
            format!("{base}-{ssid}")
        };
        calls.push(call(name.as_bytes())?);
        marks.push(a[6] & 128 != 0);
        offset += 7;
        if a[6] & 1 != 0 {
            break;
        }
    }
    if calls.len() < 2 {
        return None;
    }
    let control = *frame.get(offset)?;
    if control & !0x10 != 3 {
        return None;
    }
    let pid = *frame.get(offset + 1)?;
    let info = &frame[offset + 2..];
    let mut via = String::new();
    let last_repeat = marks
        .iter()
        .enumerate()
        .skip(2)
        .filter(|(_, b)| **b)
        .map(|(i, _)| i)
        .next_back();
    for (i, c) in calls.iter().enumerate().skip(2) {
        via.push_str(if i == 2 { " Via " } else { "," });
        via.push_str(c);
        if Some(i) == last_repeat {
            via.push('*');
        }
    }
    let pf = if marks[0] != marks[1] {
        if marks[0] {
            "P"
        } else {
            "F"
        }
    } else {
        "PF"
    };
    let mut body = format!(
        " 1:Fm {} To {}{} <UI pid={pid:02X} Len={} {pf}={} >[{time}]\r",
        calls[1],
        calls[0],
        via,
        info.len(),
        (control >> 4) & 1
    )
    .into_bytes();
    body.extend(info);
    body.extend(b"\r\0");
    Some(packet(
        b'U',
        calls[1].as_bytes(),
        calls[0].as_bytes(),
        pid,
        &body,
    ))
}
fn utc_time() -> String {
    let s = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        % 86400;
    format!("{:02}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
}

/// The single radio port an [`AgwServer`] describes in its `G` (port
/// information) and `g` (port capabilities) replies.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgwPortInfo {
    description: &'static str,
    baud_code: u8,
}
impl AgwPortInfo {
    /// AFSK 1200, the default: `G` replies `1;Port1 coppa AFSK 1200;` plus
    /// NUL, and `g` byte 0 is baud code 0 (1200).
    pub fn afsk_1200() -> Self {
        Self {
            description: "coppa AFSK 1200",
            baud_code: 0,
        }
    }
    /// G3RUH 9600: `G` replies `1;Port1 coppa G3RUH 9600;` plus NUL, and
    /// `g` byte 0 is baud code 3 (9600).
    pub fn g3ruh_9600() -> Self {
        Self {
            description: "coppa G3RUH 9600",
            baud_code: 3,
        }
    }
    /// `G` reply body: one port, this description, then NUL.
    fn port_reply(&self) -> Vec<u8> {
        format!("1;Port1 {};\0", self.description).into_bytes()
    }
    /// `g` reply body: the on-air baud code, `FF`, then ten zeroes for
    /// traffic and timing statistics coppa does not report.
    fn capabilities_reply(&self) -> [u8; 12] {
        let mut caps = [0; 12];
        caps[0] = self.baud_code;
        caps[1] = 0xFF;
        caps
    }
}

/// Bounded connectionless AGWPE TCP server, disabled unless explicitly started.
pub struct AgwServer {
    port: u16,
    bind_addr: String,
    tx: mpsc::Sender<Vec<u8>>,
    receiver: Option<mpsc::Receiver<Vec<u8>>>,
    rx: broadcast::Sender<Vec<u8>>,
    pending: Arc<AtomicUsize>,
    info: AgwPortInfo,
}
impl AgwServer {
    /// Bind to loopback when started.
    pub fn new(port: u16) -> Self {
        Self::with_bind_addr(port, "127.0.0.1".into())
    }
    /// Select the listening address. The port is described as AFSK 1200
    /// until [`AgwServer::with_port_info`] says otherwise.
    pub fn with_bind_addr(port: u16, bind_addr: String) -> Self {
        let (tx, receiver) = mpsc::channel(TX_CAP);
        let (rx, _) = broadcast::channel(64);
        Self {
            port,
            bind_addr,
            tx,
            receiver: Some(receiver),
            rx,
            pending: Arc::new(AtomicUsize::new(0)),
            info: AgwPortInfo::afsk_1200(),
        }
    }
    /// Share the transmitter's active and queued frame snapshot.
    #[must_use]
    pub fn with_pending_frames(mut self, pending: Arc<AtomicUsize>) -> Self {
        self.pending = pending;
        self
    }
    /// Describe the TNC's modem in the `G` and `g` replies.
    #[must_use]
    pub fn with_port_info(mut self, info: AgwPortInfo) -> Self {
        self.info = info;
        self
    }
    /// Take the FCS-free AX.25 transmit channel once.
    pub fn take_tx_receiver(&mut self) -> Option<mpsc::Receiver<Vec<u8>>> {
        self.receiver.take()
    }
    /// Fan received FCS-free AX.25 frames out to subscribers.
    pub fn rx_sender(&self) -> broadcast::Sender<Vec<u8>> {
        self.rx.clone()
    }
    /// Bind and serve until cancelled.
    pub async fn start(&self) -> Result<(), HostError> {
        self.run_with_listener(TcpListener::bind((self.bind_addr.as_str(), self.port)).await?)
            .await
    }
    /// Serve using a listener whose successful bind has already been checked.
    pub async fn run_with_listener(&self, listener: TcpListener) -> Result<(), HostError> {
        let permits = Arc::new(Semaphore::new(16));
        let mut clients = tokio::task::JoinSet::new();
        loop {
            let accepted = tokio::select! {
                result = listener.accept() => result,
                _ = clients.join_next(), if !clients.is_empty() => continue,
            };
            let (stream, _) = accepted?;
            let Ok(permit) = permits.clone().try_acquire_owned() else {
                continue;
            };
            let tx = self.tx.clone();
            let rx = self.rx.subscribe();
            let pending = self.pending.clone();
            let info = self.info.clone();
            clients.spawn(async move {
                let _permit = permit;
                client(stream, tx, rx, pending, info).await;
            });
        }
    }
}
/// Longest an outbound record waits for queue space.
const REPLY_WAIT: Duration = Duration::from_secs(5);

/// Queue one outbound record, waiting so the writer can drain request or
/// receive bursts. False means the client must be dropped: the writer has
/// gone (its own write timed out) or stayed stalled for `REPLY_WAIT`.
async fn enqueue(out: &mpsc::Sender<Vec<u8>>, record: Vec<u8>) -> bool {
    matches!(
        tokio::time::timeout(REPLY_WAIT, out.send(record)).await,
        Ok(Ok(()))
    )
}

async fn client(
    stream: TcpStream,
    tx: mpsc::Sender<Vec<u8>>,
    mut rx: broadcast::Receiver<Vec<u8>>,
    pending: Arc<AtomicUsize>,
    info: AgwPortInfo,
) {
    let (mut reader, mut writer) = stream.into_split();
    let (out, mut outgoing) = mpsc::channel::<Vec<u8>>(64);
    let writing = async move {
        while let Some(record) = outgoing.recv().await {
            match tokio::time::timeout(Duration::from_secs(5), writer.write_all(&record)).await {
                Ok(Ok(())) => {}
                _ => break,
            }
        }
    };
    let reading = async move {
        let mut accum = Vec::new();
        let mut chunk = [0; 1024];
        let mut raw = false;
        let mut mon = false;
        let mut calls = HashSet::new();
        loop {
            tokio::select! {
                read=reader.read(&mut chunk)=> {
                    let n=match read {Ok(0)|Err(_)=>break,Ok(n)=>n}; accum.extend_from_slice(&chunk[..n]);
                    while accum.len()>=HEADER {
                        let len=u32::from_le_bytes(accum[28..32].try_into().unwrap()) as usize;
                        if len>MAX_BODY {tracing::warn!(len,"AGWPE body too large");return;}
                        if accum.len()<HEADER+len {break;}
                        let request:Vec<_>=accum.drain(..HEADER+len).collect(); let h=&request[..HEADER]; let b=&request[HEADER..];
                        if h[0]!=0 {continue;}
                        let response=match h[4] {
                            b'R'=>Some(packet(b'R',&[],&[],0,&[0xd5,7,0,0,127,0,0,0])),
                            b'G'=>Some(packet(b'G',&[],&[],0,&info.port_reply())),
                            b'g'=>Some(packet(b'g',&[],&[],0,&info.capabilities_reply())),
                            b'X'=> {let c=call(&h[8..18]); let ok=c.as_ref().is_some_and(|c| calls.contains(c)||calls.len()<16); if ok {calls.insert(c.clone().unwrap());} Some(packet(b'X',c.as_deref().unwrap_or("").as_bytes(),&[],0,&[u8::from(ok)]))},
                            b'x'=>{if let Some(c)=call(&h[8..18]) {calls.remove(&c);} None},
                            b'k'=>{raw = !raw;None}, b'm'=>{mon = !mon;None},
                            b'y'=>{let count=(TX_CAP-tx.capacity()).saturating_add(pending.load(Ordering::Acquire)).min(u32::MAX as usize) as u32;Some(packet(b'y',&[],&[],0,&count.to_le_bytes()))},
                            b'M'|b'V'|b'K'=>{
                                let frame=if h[4]==b'K' {b.get(1..).filter(|b|!b.is_empty()).map(|b|b.to_vec())} else {ui(h,b)};
                                if let Some(frame)=frame {if tx.try_send(frame).is_err() {tracing::warn!("AGWPE TX queue unavailable");return;}} else {tracing::warn!("Malformed AGWPE transmit request");} None
                            }, _=>None
                        };
                        if let Some(r)=response {if !enqueue(&out,r).await {return;}}
                    }
                },
                received=rx.recv()=>match received {
                    Ok(frame)=> {
                        if raw {let mut b=vec![0]; b.extend(&frame); if !enqueue(&out,packet(b'K',&[],&[],0,&b)).await {return;}}
                        if mon {if let Some(record)=monitor(&frame,&utc_time()) {if !enqueue(&out,record).await {return;}}}
                    },
                    Err(broadcast::error::RecvError::Lagged(n))=>tracing::warn!(n,"AGWPE receiver lagged"),
                    Err(broadcast::error::RecvError::Closed)=>return,
                }
            }
        }
    };
    tokio::pin!(writing, reading);
    tokio::select! {_=&mut writing=>{}, _=&mut reading=>{}}
}

#[cfg(test)]
mod tests {
    use super::*;
    async fn receive(s: &mut TcpStream) -> Vec<u8> {
        tokio::time::timeout(Duration::from_secs(3), async {
            let mut h = vec![0; 36];
            s.read_exact(&mut h).await.unwrap();
            let len = u32::from_le_bytes(h[28..32].try_into().unwrap()) as usize;
            let mut b = vec![0; len];
            s.read_exact(&mut b).await.unwrap();
            h.extend(b);
            h
        })
        .await
        .unwrap()
    }
    #[test]
    fn golden_header_and_ui_binary() {
        let p = packet(b'M', b"N0CALL", b"APRS", 0xf0, b"hello");
        assert_eq!(&p[..8], &[0, 0, 0, 0, b'M', 0, 240, 0]);
        assert_eq!(&p[8..18], b"N0CALL\0\0\0\0");
        assert_eq!(&p[28..36], &[5, 0, 0, 0, 0, 0, 0, 0]);
        let f = ui(&p[..36], &p[36..]).unwrap();
        assert_eq!(
            f,
            [
                vec![130, 160, 164, 166, 64, 64, 96, 156, 96, 134, 130, 152, 152, 97, 3, 240],
                b"hello".to_vec()
            ]
            .concat()
        );
        assert_eq!(
            &monitor(&f, "12:34:56").unwrap()[36..],
            b" 1:Fm N0CALL To APRS <UI pid=F0 Len=5 PF=0 >[12:34:56]\rhello\r\0"
        );
        assert!(ui(&p[..36], &[0; 257]).is_none());
        assert_eq!(ui(&p[..36], &[0; 256]).unwrap().len(), 272);
        for c in ["", "ABCDEFG", "N0CALL-16", "A--1", "A-", "A B", "é"] {
            assert!(call(c.as_bytes()).is_none(), "{c}");
        }
        assert_eq!(call(b"n0call-15"), Some("N0CALL-15".into()));
        let mut v = packet(b'V', b"N0CALL", b"APRS", 0xcf, &[]);
        for count in [0, 1, 8] {
            let mut body = vec![count];
            for _ in 0..count {
                body.extend(b"WIDE1-1\0\0\0");
            }
            body.extend(b"a\0b");
            let f = ui(&v, &body).unwrap();
            assert_eq!(&f[f.len() - 5..], &[3, 207, b'a', 0, b'b']);
            assert_eq!(f.len(), 16 + 7 * usize::from(count) + 3);
        }
        assert!(ui(&v, &[9]).is_none());
        assert!(ui(&v, &[1, 0]).is_none());
        v[8] = b'!';
        assert!(ui(&v, &[0]).is_none());
    }
    #[tokio::test]
    async fn tcp_fragmentation_receive_and_replies() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let pending = Arc::new(AtomicUsize::new(3));
        let mut server = AgwServer::new(0).with_pending_frames(pending.clone());
        let mut tx = server.take_tx_receiver().unwrap();
        let rx = server.rx_sender();
        let task = tokio::spawn(async move { server.run_with_listener(listener).await.unwrap() });
        let mut c = TcpStream::connect(addr).await.unwrap();
        c.write_all(
            &[
                packet(b'k', &[], &[], 0, &[]),
                packet(b'm', &[], &[], 0, &[]),
                packet(b'R', &[], &[], 0, &[]),
            ]
            .concat(),
        )
        .await
        .unwrap();
        assert_eq!(&receive(&mut c).await[36..], &[213, 7, 0, 0, 127, 0, 0, 0]);
        let req = packet(b'M', b"N0CALL", b"APRS", 240, b"hi\0");
        let frame = ui(&req, &req[36..]).unwrap();
        for split in 1..req.len() {
            c.write_all(&req[..split]).await.unwrap();
            rx.send(frame.clone()).unwrap();
            assert_eq!(
                &receive(&mut c).await[36..],
                [vec![0], frame.clone()].concat()
            );
            assert_eq!(receive(&mut c).await[4], b'U');
            c.write_all(&req[split..]).await.unwrap();
            assert_eq!(
                tokio::time::timeout(Duration::from_secs(3), tx.recv())
                    .await
                    .unwrap()
                    .unwrap(),
                frame
            );
        }
        for (kind, expected) in [
            (b'G', b"1;Port1 coppa AFSK 1200;\0".to_vec()),
            (b'g', vec![0, 255, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
            (b'X', vec![1]),
            (b'y', 3u32.to_le_bytes().to_vec()),
        ] {
            c.write_all(&packet(kind, b"N0CALL", &[], 0, &[]))
                .await
                .unwrap();
            assert_eq!(&receive(&mut c).await[36..], expected);
        }
        c.write_all(&packet(b'K', &[], &[], 0, &[0, 0x42, 0x00]))
            .await
            .unwrap();
        assert_eq!(tx.recv().await.unwrap(), vec![0x42, 0]);
        task.abort();
        let _ = task.await;
        let mut one = [0];
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(3), c.read(&mut one))
                .await
                .unwrap()
                .unwrap(),
            0
        );
    }
    #[tokio::test]
    async fn oversized_and_unknown_packets() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = AgwServer::new(0);
        let task = tokio::spawn(async move { server.run_with_listener(listener).await.unwrap() });
        for length in [1025, u32::MAX] {
            let mut c = TcpStream::connect(addr).await.unwrap();
            let mut h = packet(b'M', &[], &[], 0, &[]);
            h[28..32].copy_from_slice(&length.to_le_bytes());
            c.write_all(&h).await.unwrap();
            let mut b = [0];
            assert_eq!(
                tokio::time::timeout(Duration::from_secs(3), c.read(&mut b))
                    .await
                    .unwrap()
                    .unwrap(),
                0
            );
        }
        let mut c = TcpStream::connect(addr).await.unwrap();
        c.write_all(
            &[
                packet(b'P', &[], &[], 0, &[1; 1024]),
                packet(b'R', &[], &[], 0, &[]),
            ]
            .concat(),
        )
        .await
        .unwrap();
        assert_eq!(receive(&mut c).await[4], b'R');
        task.abort();
        let _ = task.await;
    }
    #[tokio::test]
    async fn registration_limits_and_saturated_tx_isolate_clients() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let mut server = AgwServer::new(0);
        let _tx = server.take_tx_receiver().unwrap();
        let task = tokio::spawn(async move { server.run_with_listener(listener).await.unwrap() });
        let mut c = TcpStream::connect(addr).await.unwrap();
        for i in 0..17 {
            c.write_all(&packet(b'X', format!("C{i}").as_bytes(), &[], 0, &[]))
                .await
                .unwrap();
            assert_eq!(receive(&mut c).await[36], u8::from(i < 16));
        }
        for (name, ok) in [("C0", 1), ("INVALID", 0)] {
            c.write_all(&packet(b'X', name.as_bytes(), &[], 0, &[]))
                .await
                .unwrap();
            assert_eq!(receive(&mut c).await[36], ok);
        }
        c.write_all(
            &[
                packet(b'x', b"C0", &[], 0, &[]),
                packet(b'X', b"C16", &[], 0, &[]),
            ]
            .concat(),
        )
        .await
        .unwrap();
        assert_eq!(receive(&mut c).await[36], 1);
        for _ in 0..256 {
            c.write_all(&packet(b'K', &[], &[], 0, &[0, 1]))
                .await
                .unwrap();
        }
        c.write_all(&packet(b'y', &[], &[], 0, &[])).await.unwrap();
        assert_eq!(&receive(&mut c).await[36..], &256u32.to_le_bytes());
        c.write_all(&packet(b'K', &[], &[], 0, &[0, 1]))
            .await
            .unwrap();
        let mut b = [0];
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(3), c.read(&mut b))
                .await
                .unwrap()
                .unwrap(),
            0
        );
        let mut healthy = TcpStream::connect(addr).await.unwrap();
        healthy
            .write_all(&packet(b'R', &[], &[], 0, &[]))
            .await
            .unwrap();
        assert_eq!(receive(&mut healthy).await[4], b'R');
        task.abort();
        let _ = task.await;
    }

    // ---- COP-137 remediation: the planned fixture and socket matrix ----

    /// Hand-assembled AX.25 address field: six shifted, space-padded
    /// characters and an SSID byte with C/H bit `high` and end bit `last`.
    fn ax_addr(base: &[u8], ssid: u8, high: bool, last: bool) -> Vec<u8> {
        let mut out = [0x40u8; 7];
        for (i, b) in base.iter().enumerate() {
            out[i] = b << 1;
        }
        out[6] = 0x60 | (ssid << 1) | (u8::from(high) << 7) | u8::from(last);
        out.to_vec()
    }

    async fn spawn_server(
        pending: Arc<AtomicUsize>,
    ) -> (
        std::net::SocketAddr,
        mpsc::Receiver<Vec<u8>>,
        broadcast::Sender<Vec<u8>>,
        tokio::task::JoinHandle<()>,
    ) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let mut server = AgwServer::new(0).with_pending_frames(pending);
        let tx = server.take_tx_receiver().unwrap();
        let rx = server.rx_sender();
        let task = tokio::spawn(async move { server.run_with_listener(listener).await.unwrap() });
        (addr, tx, rx, task)
    }

    async fn barrier(c: &mut TcpStream) {
        c.write_all(&packet(b'R', &[], &[], 0, &[])).await.unwrap();
        assert_eq!(receive(c).await[4], b'R');
    }

    async fn assert_closed(c: &mut TcpStream) {
        let mut sink = vec![0; 65536];
        tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                match c.read(&mut sink).await {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
            }
        })
        .await
        .expect("server must close this client");
    }

    /// Phase 1 golden monitor bytes, written by hand rather than by `ui()`:
    /// a repeated digipeater gets `*`, the C bits select P or F, and the
    /// P/F bit is read from the control byte.
    #[test]
    fn monitor_golden_digipeaters_and_poll_final() {
        let mut f = ax_addr(b"APRS", 0, true, false);
        f.extend(ax_addr(b"N0CALL", 0, false, false));
        f.extend(ax_addr(b"WIDE1", 1, true, false));
        f.extend(ax_addr(b"WIDE2", 2, false, true));
        f.extend([0x03, 0xf0, b'h', b'i']);
        let record = monitor(&f, "01:02:03").unwrap();
        assert_eq!(&record[..8], &[0, 0, 0, 0, b'U', 0, 0xf0, 0]);
        assert_eq!(&record[8..18], b"N0CALL\0\0\0\0");
        assert_eq!(&record[18..28], b"APRS\0\0\0\0\0\0");
        assert_eq!(
            &record[36..],
            b" 1:Fm N0CALL To APRS Via WIDE1-1*,WIDE2-2 <UI pid=F0 Len=2 P=0 >[01:02:03]\rhi\r\0"
        );
        assert_eq!(
            u32::from_le_bytes(record[28..32].try_into().unwrap()) as usize,
            record.len() - 36
        );

        let mut f = ax_addr(b"APRS", 0, false, false);
        f.extend(ax_addr(b"N0CALL", 15, true, true));
        f.extend([0x13, 0xcf]);
        assert_eq!(
            &monitor(&f, "23:59:59").unwrap()[36..],
            b" 1:Fm N0CALL-15 To APRS <UI pid=CF Len=0 F=1 >[23:59:59]\r\r\0"
        );

        // Both C bits equal (old AX.25 v1 style): neither P nor F alone.
        let mut f = ax_addr(b"APRS", 0, false, false);
        f.extend(ax_addr(b"N0CALL", 0, false, true));
        f.extend([0x03, 0xf0, 0, 0xff]);
        let record = monitor(&f, "00:00:00").unwrap();
        assert_eq!(
            &record[36..],
            b" 1:Fm N0CALL To APRS <UI pid=F0 Len=2 PF=0 >[00:00:00]\r\0\xff\r\0"
        );
    }

    /// Non-UI and malformed frames produce no monitor record.
    #[test]
    fn monitor_rejects_non_ui_and_malformed_headers() {
        let head = [
            ax_addr(b"APRS", 0, true, false),
            ax_addr(b"N0CALL", 0, false, true),
        ]
        .concat();
        for control in [0x3f, 0x00, 0x01, 0x0f] {
            let f = [head.clone(), vec![control, 0xf0, b'x']].concat();
            assert!(monitor(&f, "00:00:00").is_none(), "control {control:#x}");
        }
        // Truncated address, no end bit, one address only, missing PID,
        // low bit set inside a callsign, and an illegal callsign.
        let no_end = [
            ax_addr(b"APRS", 0, true, false),
            ax_addr(b"N0CALL", 0, false, false),
        ]
        .concat();
        let one = ax_addr(b"APRS", 0, true, true);
        let mut odd = head.clone();
        odd[0] |= 1;
        let mut bad_call = head.clone();
        bad_call[0] = b'!' << 1;
        for f in [
            head[..10].to_vec(),
            no_end,
            [one, vec![0x03, 0xf0]].concat(),
            [head.clone(), vec![0x03]].concat(),
            [odd, vec![0x03, 0xf0]].concat(),
            [bad_call, vec![0x03, 0xf0]].concat(),
            Vec::new(),
        ] {
            assert!(monitor(&f, "00:00:00").is_none(), "{f:02x?}");
        }
    }

    /// An `M` frame matches an independently written AX.25 UI header: C bits
    /// clear as `coppa-protocol`'s encoder writes them, end bit on the last
    /// address, control 03 and the PID.
    #[test]
    fn ui_frame_matches_hand_assembled_header() {
        let h = packet(b'M', b"n0call-7", b"APRS-15", 0xcf, &[]);
        let expected = [
            ax_addr(b"APRS", 15, false, false),
            ax_addr(b"N0CALL", 7, false, true),
            vec![0x03, 0xcf, 0, b'z', 0],
        ]
        .concat();
        assert_eq!(ui(&h, &[0, b'z', 0]).unwrap(), expected);
        let mut body = vec![2];
        body.extend(b"WIDE1-1\0\0\0");
        body.extend(b"WIDE2-2\0\0\0");
        body.push(b'q');
        let h = packet(b'V', b"N0CALL", b"APRS", 0xf0, &[]);
        let expected = [
            ax_addr(b"APRS", 0, false, false),
            ax_addr(b"N0CALL", 0, false, false),
            ax_addr(b"WIDE1", 1, false, false),
            ax_addr(b"WIDE2", 2, false, true),
            vec![0x03, 0xf0, b'q'],
        ]
        .concat();
        assert_eq!(ui(&h, &body).unwrap(), expected);
        // Malformed digipeater counts and truncated fields.
        assert!(ui(&h, &[]).is_none());
        assert!(ui(&h, &[2, 0, 0]).is_none());
        assert!(ui(&h, &body[..20]).is_none());
        let mut nine = vec![9];
        for _ in 0..9 {
            nine.extend(b"WIDE1-1\0\0\0");
        }
        assert!(ui(&h, &nine).is_none());
        let mut bad = body.clone();
        bad[1] = b'!';
        assert!(ui(&h, &bad).is_none());
    }

    /// EOF at every offset of a request closes only that client and
    /// transmits nothing; a complete request followed by EOF transmits.
    #[tokio::test]
    async fn eof_at_every_offset_closes_only_that_client() {
        let (addr, mut tx, _rx, task) = spawn_server(Arc::default()).await;
        let mut healthy = TcpStream::connect(addr).await.unwrap();
        let req = packet(b'M', b"N0CALL", b"APRS", 0xf0, b"hello");
        for split in 1..req.len() {
            let mut c = TcpStream::connect(addr).await.unwrap();
            c.write_all(&req[..split]).await.unwrap();
            c.shutdown().await.unwrap();
            assert_closed(&mut c).await;
            barrier(&mut healthy).await;
        }
        assert!(tx.try_recv().is_err(), "a truncated request transmitted");
        let mut c = TcpStream::connect(addr).await.unwrap();
        c.write_all(&req).await.unwrap();
        c.shutdown().await.unwrap();
        assert_closed(&mut c).await;
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(3), tx.recv())
                .await
                .unwrap()
                .unwrap(),
            ui(&req, &req[36..]).unwrap()
        );
        task.abort();
        let _ = task.await;
    }

    /// Unsupported ports and malformed transmit requests are consumed
    /// without a reply or a transmission; raw `K` keeps non-UI bytes
    /// unchanged with no FCS or extra trailing byte.
    #[tokio::test]
    async fn port_rejection_malformed_requests_and_raw_non_ui() {
        let (addr, mut tx, _rx, task) = spawn_server(Arc::default()).await;
        let mut c = TcpStream::connect(addr).await.unwrap();
        let mut wrong_port = Vec::new();
        for &kind in b"GMKXy" {
            let mut p = packet(kind, b"N0CALL", b"APRS", 0xf0, &[0, 1, 2]);
            p[0] = 1;
            wrong_port.extend(p);
        }
        let mut digi = vec![1];
        digi.extend(b"WIDE");
        let malformed = [
            packet(b'M', b"N0CALL", b"APRS", 0xf0, &[0; 257]),
            packet(b'M', b"BAD CALL", b"APRS", 0xf0, b"x"),
            packet(b'V', b"N0CALL", b"APRS", 0xf0, &digi),
            packet(b'K', &[], &[], 0, &[0]),
            packet(b'K', &[], &[], 0, &[]),
        ]
        .concat();
        c.write_all(&[wrong_port, malformed].concat())
            .await
            .unwrap();
        barrier(&mut c).await;
        assert!(tx.try_recv().is_err(), "a rejected request transmitted");

        let sabm = [
            ax_addr(b"N0CALL", 1, true, false),
            ax_addr(b"N0CALL", 2, false, true),
            vec![0x3f],
        ]
        .concat();
        let mut body = vec![0];
        body.extend(&sabm);
        c.write_all(&packet(b'K', &[], &[], 0, &body))
            .await
            .unwrap();
        barrier(&mut c).await;
        assert_eq!(tx.try_recv().unwrap(), sabm);
        assert!(tx.try_recv().is_err());
        task.abort();
        let _ = task.await;
    }

    /// Toggles are per connection, start off and flip on each request;
    /// registration and toggles reset on reconnect.
    #[tokio::test]
    async fn toggles_are_per_connection_and_reset_on_reconnect() {
        let (addr, _tx, rx, task) = spawn_server(Arc::default()).await;
        let frame = [
            ax_addr(b"APRS", 0, true, false),
            ax_addr(b"N0CALL", 0, false, true),
            vec![0x03, 0xf0, b'o', b'k'],
        ]
        .concat();
        let mut raw = TcpStream::connect(addr).await.unwrap();
        let mut mon = TcpStream::connect(addr).await.unwrap();
        let mut both = TcpStream::connect(addr).await.unwrap();
        let mut neither = TcpStream::connect(addr).await.unwrap();
        // `k` twice turns raw back off; `m` proves the frame was handled.
        let mut raw_off = TcpStream::connect(addr).await.unwrap();
        let toggle = |kinds: &[u8]| -> Vec<u8> {
            kinds
                .iter()
                .map(|k| packet(*k, &[], &[], 0, &[]))
                .collect::<Vec<_>>()
                .concat()
        };
        for (c, kinds) in [
            (&mut raw, &b"k"[..]),
            (&mut mon, b"m"),
            (&mut both, b"km"),
            (&mut neither, b""),
            (&mut raw_off, b"kkm"),
        ] {
            c.write_all(&toggle(kinds)).await.unwrap();
            barrier(c).await;
        }
        for c in [&mut raw, &mut both] {
            for x in 0..16 {
                c.write_all(&packet(b'X', format!("R{x}").as_bytes(), &[], 0, &[]))
                    .await
                    .unwrap();
                assert_eq!(receive(c).await[36], 1);
            }
        }
        rx.send(frame.clone()).unwrap();
        let k = receive(&mut raw).await;
        assert_eq!(
            (k[4], &k[36..]),
            (b'K', &[vec![0], frame.clone()].concat()[..])
        );
        assert_eq!(receive(&mut mon).await[4], b'U');
        assert_eq!(receive(&mut both).await[4], b'K');
        assert_eq!(receive(&mut both).await[4], b'U');
        assert_eq!(receive(&mut raw_off).await[4], b'U');
        for c in [&mut raw, &mut mon, &mut both, &mut raw_off] {
            barrier(c).await;
        }
        barrier(&mut neither).await;
        let mut one = [0];
        assert!(
            tokio::time::timeout(Duration::from_millis(300), neither.read(&mut one))
                .await
                .is_err(),
            "an unsubscribed client received a record"
        );

        // Reconnect: the 17th registration is refused on the old
        // connection but succeeds on a fresh one, and toggles start off.
        raw.write_all(&packet(b'X', b"R16", &[], 0, &[]))
            .await
            .unwrap();
        assert_eq!(receive(&mut raw).await[36], 0);
        drop(raw);
        let mut again = TcpStream::connect(addr).await.unwrap();
        again
            .write_all(&[toggle(b"m"), packet(b'X', b"R16", &[], 0, &[])].concat())
            .await
            .unwrap();
        assert_eq!(receive(&mut again).await[36], 1);
        rx.send(frame).unwrap();
        assert_eq!(receive(&mut again).await[4], b'U');
        barrier(&mut again).await;
        task.abort();
        let _ = task.await;
    }

    /// Sixteen clients are served; a 17th is closed while the sixteen keep
    /// working, and a disconnect frees its slot.
    #[tokio::test]
    async fn seventeenth_client_is_refused_and_slots_are_released() {
        let (addr, _tx, _rx, task) = spawn_server(Arc::default()).await;
        let mut clients = Vec::new();
        for _ in 0..16 {
            let mut c = TcpStream::connect(addr).await.unwrap();
            barrier(&mut c).await;
            clients.push(c);
        }
        let mut extra = TcpStream::connect(addr).await.unwrap();
        let _ = extra.write_all(&packet(b'R', &[], &[], 0, &[])).await;
        assert_closed(&mut extra).await;
        for c in &mut clients {
            barrier(c).await;
        }
        drop(clients.pop());
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let mut c = TcpStream::connect(addr).await.unwrap();
                c.write_all(&packet(b'R', &[], &[], 0, &[])).await.unwrap();
                let mut h = [0; 36];
                if c.read_exact(&mut h).await.is_ok() {
                    assert_eq!(h[4], b'R');
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("a released slot must admit a new client");
        task.abort();
        let _ = task.await;
    }

    /// A raw subscriber that never reads fills its bounded outbound queue
    /// and is disconnected once its writes stay stalled past the write
    /// timeout; another client's version request is still answered while
    /// that happens.
    #[tokio::test]
    async fn nonreading_client_is_dropped_without_blocking_others() {
        let (addr, _tx, rx, task) = spawn_server(Arc::default()).await;
        let socket = tokio::net::TcpSocket::new_v4().unwrap();
        socket.set_recv_buffer_size(4096).unwrap();
        let mut stalled = socket.connect(addr).await.unwrap();
        stalled
            .write_all(&packet(b'k', &[], &[], 0, &[]))
            .await
            .unwrap();
        barrier(&mut stalled).await;
        let mut healthy = TcpStream::connect(addr).await.unwrap();
        barrier(&mut healthy).await;
        let big = vec![0x55; 60_000];
        for i in 0..400 {
            let _ = rx.send(big.clone());
            tokio::task::yield_now().await;
            if i % 50 == 0 {
                barrier(&mut healthy).await;
            }
        }
        barrier(&mut healthy).await;
        // Keep not reading past the 5 s write timeout and REPLY_WAIT; a
        // briefly slow reader is not dropped.
        tokio::time::sleep(Duration::from_secs(7)).await;
        barrier(&mut healthy).await;
        assert_closed(&mut stalled).await;
        barrier(&mut healthy).await;
        task.abort();
        let _ = task.await;
    }

    /// A client that pipelines more reply-producing requests than the
    /// outbound queue holds, while reading replies, stays connected and gets
    /// every reply: request handling waits for the writer instead of
    /// treating a momentarily full queue as a stalled receiver.
    #[tokio::test]
    async fn pipelined_requests_beyond_queue_capacity_are_all_answered() {
        let (addr, _tx, _rx, task) = spawn_server(Arc::default()).await;
        let c = TcpStream::connect(addr).await.unwrap();
        let (mut reader, mut writer) = c.into_split();
        let burst = packet(b'R', &[], &[], 0, &[]).repeat(200);
        let send = tokio::spawn(async move {
            writer.write_all(&burst).await.unwrap();
            writer
        });
        let mut reply = vec![0; 44];
        for i in 0..200 {
            tokio::time::timeout(Duration::from_secs(5), reader.read_exact(&mut reply))
                .await
                .unwrap_or_else(|_| panic!("reply {i} timed out"))
                .unwrap_or_else(|e| panic!("reply {i}: {e}"));
            assert_eq!(reply[4], b'R');
        }
        let mut c = send.await.unwrap().reunite(reader).unwrap();
        barrier(&mut c).await;
        task.abort();
        let _ = task.await;
    }

    /// A raw+monitor subscriber that is reading gets every record of a
    /// buffered receive burst larger than its outbound queue.
    #[tokio::test]
    async fn buffered_receive_burst_reaches_a_reading_subscriber() {
        let (addr, _tx, rx, task) = spawn_server(Arc::default()).await;
        let frame = [
            ax_addr(b"APRS", 0, true, false),
            ax_addr(b"N0CALL", 0, false, true),
            vec![0x03, 0xf0, b'o', b'k'],
        ]
        .concat();
        let mut c = TcpStream::connect(addr).await.unwrap();
        c.write_all(
            &[
                packet(b'k', &[], &[], 0, &[]),
                packet(b'm', &[], &[], 0, &[]),
            ]
            .concat(),
        )
        .await
        .unwrap();
        barrier(&mut c).await;
        for _ in 0..40 {
            rx.send(frame.clone()).unwrap();
        }
        for i in 0..80 {
            let record = receive(&mut c).await;
            assert_eq!(
                record[4],
                if i % 2 == 0 { b'K' } else { b'U' },
                "record {i}"
            );
        }
        barrier(&mut c).await;
        task.abort();
        let _ = task.await;
    }

    /// A client that floods requests but never reads replies is still
    /// disconnected once its writes stall.
    #[tokio::test]
    async fn nonreading_request_flood_is_disconnected() {
        let (addr, _tx, _rx, task) = spawn_server(Arc::default()).await;
        let socket = tokio::net::TcpSocket::new_v4().unwrap();
        socket.set_recv_buffer_size(4096).unwrap();
        let c = socket.connect(addr).await.unwrap();
        let (mut reader, mut writer) = c.into_split();
        let burst = packet(b'R', &[], &[], 0, &[]).repeat(20_000);
        let flood = tokio::spawn(async move {
            let _ = tokio::time::timeout(Duration::from_secs(20), writer.write_all(&burst)).await;
        });
        tokio::time::sleep(Duration::from_secs(8)).await;
        let mut sink = vec![0; 65536];
        tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                match reader.read(&mut sink).await {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
            }
        })
        .await
        .expect("server must close a client that never reads");
        flood.abort();
        task.abort();
        let _ = task.await;
    }

    /// COP-164: `G` and `g` describe the modem the server was built with.
    /// The 9600 G3RUH port says so in `G` and carries baud code 3 in `g`
    /// byte 0; the 1200 AFSK port keeps COP-137's bytes.
    #[tokio::test]
    async fn port_info_and_capabilities_follow_the_configured_modem() {
        for (info, port, code) in [
            (
                AgwPortInfo::g3ruh_9600(),
                &b"1;Port1 coppa G3RUH 9600;\0"[..],
                3,
            ),
            (
                AgwPortInfo::afsk_1200(),
                &b"1;Port1 coppa AFSK 1200;\0"[..],
                0,
            ),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let server = AgwServer::new(0).with_port_info(info.clone());
            let task =
                tokio::spawn(async move { server.run_with_listener(listener).await.unwrap() });
            let mut c = TcpStream::connect(addr).await.unwrap();
            c.write_all(
                &[
                    packet(b'G', &[], &[], 0, &[]),
                    packet(b'g', &[], &[], 0, &[]),
                ]
                .concat(),
            )
            .await
            .unwrap();
            let g = receive(&mut c).await;
            assert_eq!((g[4], &g[36..]), (b'G', port), "{info:?}");
            let caps = receive(&mut c).await;
            let mut expected = vec![0; 12];
            expected[0] = code;
            expected[1] = 0xFF;
            assert_eq!((caps[4], &caps[36..]), (b'g', &expected[..]), "{info:?}");
            task.abort();
            let _ = task.await;
        }
    }

    /// `y` adds the shared transmitter snapshot to this server's queued
    /// frames, and returns zero once both are drained.
    #[tokio::test]
    async fn outstanding_frames_count_drains_to_zero() {
        let pending = Arc::new(AtomicUsize::new(2));
        let (addr, mut tx, _rx, task) = spawn_server(pending.clone()).await;
        let mut c = TcpStream::connect(addr).await.unwrap();
        let y = packet(b'y', &[], &[], 0, &[]);
        let k = packet(b'K', &[], &[], 0, &[0, 1, 2]);
        c.write_all(&[k.clone(), k.clone(), k, y.clone()].concat())
            .await
            .unwrap();
        let reply = receive(&mut c).await;
        assert_eq!((reply[4], &reply[36..]), (b'y', &5u32.to_le_bytes()[..]));
        for _ in 0..3 {
            assert_eq!(tx.recv().await.unwrap(), vec![1, 2]);
        }
        pending.store(0, Ordering::Release);
        c.write_all(&y).await.unwrap();
        assert_eq!(&receive(&mut c).await[36..], &0u32.to_le_bytes());
        task.abort();
        let _ = task.await;
    }
}
