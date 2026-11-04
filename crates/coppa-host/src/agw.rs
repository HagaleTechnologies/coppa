//! Connectionless AGWPE adapter for the AFSK TNC. Frames exclude FCS.
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

/// Bounded connectionless AGWPE TCP server, disabled unless explicitly started.
pub struct AgwServer {
    port: u16,
    bind_addr: String,
    tx: mpsc::Sender<Vec<u8>>,
    receiver: Option<mpsc::Receiver<Vec<u8>>>,
    rx: broadcast::Sender<Vec<u8>>,
    pending: Arc<AtomicUsize>,
}
impl AgwServer {
    /// Bind to loopback when started.
    pub fn new(port: u16) -> Self {
        Self::with_bind_addr(port, "127.0.0.1".into())
    }
    /// Select the listening address.
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
        }
    }
    /// Share the transmitter's active and queued frame snapshot.
    #[must_use]
    pub fn with_pending_frames(mut self, pending: Arc<AtomicUsize>) -> Self {
        self.pending = pending;
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
            clients.spawn(async move {
                let _permit = permit;
                client(stream, tx, rx, pending).await;
            });
        }
    }
}
async fn client(
    stream: TcpStream,
    tx: mpsc::Sender<Vec<u8>>,
    mut rx: broadcast::Receiver<Vec<u8>>,
    pending: Arc<AtomicUsize>,
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
                            b'G'=>Some(packet(b'G',&[],&[],0,b"1;Port1 coppa AFSK 1200;\0")),
                            b'g'=>Some(packet(b'g',&[],&[],0,&[0,255,0,0,0,0,0,0,0,0,0,0])),
                            b'X'=> {let c=call(&h[8..18]); let ok=c.as_ref().is_some_and(|c| calls.contains(c)||calls.len()<16); if ok {calls.insert(c.clone().unwrap());} Some(packet(b'X',c.as_deref().unwrap_or("").as_bytes(),&[],0,&[u8::from(ok)]))},
                            b'x'=>{if let Some(c)=call(&h[8..18]) {calls.remove(&c);} None},
                            b'k'=>{raw = !raw;None}, b'm'=>{mon = !mon;None},
                            b'y'=>{let count=(TX_CAP-tx.capacity()).saturating_add(pending.load(Ordering::Acquire)).min(u32::MAX as usize) as u32;Some(packet(b'y',&[],&[],0,&count.to_le_bytes()))},
                            b'M'|b'V'|b'K'=>{
                                let frame=if h[4]==b'K' {b.get(1..).filter(|b|!b.is_empty()).map(|b|b.to_vec())} else {ui(h,b)};
                                if let Some(frame)=frame {if tx.try_send(frame).is_err() {tracing::warn!("AGWPE TX queue unavailable");return;}} else {tracing::warn!("Malformed AGWPE transmit request");} None
                            }, _=>None
                        };
                        if let Some(r)=response {if out.try_send(r).is_err() {return;}}
                    }
                },
                received=rx.recv()=>match received {
                    Ok(frame)=> {
                        if raw {let mut b=vec![0]; b.extend(&frame); if out.try_send(packet(b'K',&[],&[],0,&b)).is_err() {return;}}
                        if mon {if let Some(record)=monitor(&frame,&utc_time()) {if out.try_send(record).is_err() {return;}}}
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
}
