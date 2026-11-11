//! LinBPQ 6.0.26.1 startup and gateway session sequence on a half-duplex cable.
//! Virtual time cannot detect real-time decode stalls. Use tools/bpq-loop for that.
mod vara_loop;
use std::time::Duration;
use vara_loop::*;

struct BpqVara(
    PatVara,
    bool,
    Option<tokio::time::Instant>,
    Vec<tokio::time::Instant>,
);
impl BpqVara {
    async fn wait(&mut self, prefix: &str, timeout: Duration) -> String {
        let line = self.0.wait(prefix, timeout).await;
        match prefix {
            "PENDING" | "CONNECTED" => self.1 = true,
            "DISCONNECTED" => {
                self.1 = false;
                self.2 = Some(tokio::time::Instant::now() + Duration::from_secs(15));
            }
            "CANCELPENDING" => {
                self.1 = false;
                self.2 = None;
            }
            _ => {}
        }
        line
    }
    async fn hop(&mut self) {
        if self.1 || self.2.is_some_and(|t| tokio::time::Instant::now() < t) {
            return;
        }
        self.0.command("LISTEN OFF", "OK").await;
        self.0.command("BW2300", "OK").await;
        self.0.command("LISTEN ON", "OK").await;
        self.3.push(tokio::time::Instant::now());
    }

    async fn open(station: &Station) -> Self {
        let mut host = PatVara::open(station).await;
        tokio::time::sleep(Duration::from_secs(1)).await;
        host.send("MYCALL N0GW N0GW-1\rBW2300\rRETRIES 20\rLISTEN ON")
            .await;
        for _ in 0..4 {
            assert_eq!(
                tokio::time::timeout(Duration::from_secs(1), host.next_reply())
                    .await
                    .unwrap(),
                "OK"
            );
        }
        Self(host, false, None, Vec::new())
    }
}

#[tokio::test(start_paused = true)]
async fn bpq_gateway_answers_a_caller_and_holds_its_scan() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let (gateway, ga) = station("N0GW").await;
            let (caller, ca) = station("N1CALL").await;
            spawn_cable(&gateway, ga, &caller, ca);
            let mut bpq = BpqVara::open(&gateway).await;
            let mut pat = PatVara::open(&caller).await;
            pat.command("CONNECT N1CALL N0GW-1", "OK").await;
            bpq.wait("PENDING", CONNECT_TIMEOUT).await;
            assert_eq!(
                bpq.wait("CONNECTED", CONNECT_TIMEOUT).await,
                "CONNECTED N1CALL N0GW-1 2400"
            );
            pat.wait("CONNECTED", CONNECT_TIMEOUT).await;
            bpq.hop().await;
            assert!(bpq.3.is_empty(), "scanner must stay parked");
            let banner = b"[BPQ-6.0.26.1-B2FWIHJM$]\r";
            bpq.0.write_data(banner).await;
            assert_eq!(pat.read_data(banner.len()).await, banner);
            let payload = vec![b'x'; 2048];
            pat.write_data(&payload).await;
            assert_eq!(bpq.0.read_data(payload.len()).await, payload);
            bpq.0.command("DISCONNECT", "OK").await;
            bpq.wait("DISCONNECTED", DISCONNECT_TIMEOUT).await;
            pat.wait("DISCONNECTED", DISCONNECT_TIMEOUT).await;
            bpq.hop().await;
            assert!(bpq.3.is_empty());
            tokio::time::sleep(Duration::from_secs(15)).await;
            bpq.hop().await;
            assert_eq!(bpq.3.len(), 1);
        })
        .await;
}

#[tokio::test(start_paused = true)]
async fn bpq_gateway_reconnect_resends_the_startup_block() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let (gateway, ga) = station("N0GW").await;
            let (caller, ca) = station("N1CALL").await;
            spawn_cable(&gateway, ga, &caller, ca);
            drop(BpqVara::open(&gateway).await);
            let mut bpq = BpqVara::open(&gateway).await;
            let mut pat = PatVara::open(&caller).await;
            pat.command("CONNECT N1CALL N0GW-1", "OK").await;
            bpq.wait("CONNECTED", CONNECT_TIMEOUT).await;
            pat.wait("CONNECTED", CONNECT_TIMEOUT).await;
            drop(bpq);
            let mut bpq = BpqVara::open(&gateway).await;
            tokio::time::sleep(Duration::from_secs(17)).await;
            bpq.0.command("ABORT", "OK").await;
            bpq.wait("DISCONNECTED", DISCONNECT_TIMEOUT).await;
        })
        .await;
}

#[tokio::test(start_paused = true)]
async fn scanning_gateway_is_reached_by_a_retrying_caller() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let (gateway, ga) = station("N0GW").await;
            let (caller, ca) = station("N1CALL").await;
            spawn_cable(&gateway, ga, &caller, ca);
            let mut bpq = BpqVara::open(&gateway).await;
            let mut pat = PatVara::open(&caller).await;
            bpq.0.command("LISTEN OFF", "OK").await;
            pat.command("RETRIES 3", "OK").await;
            pat.command("CONNECT N1CALL N0GW-1", "OK").await;
            tokio::time::sleep(Duration::from_secs(5)).await;
            bpq.0.command("LISTEN ON", "OK").await;
            bpq.wait("CONNECTED", CONNECT_TIMEOUT).await;
            pat.wait("CONNECTED", CONNECT_TIMEOUT).await;
        })
        .await;
}

#[tokio::test(start_paused = true)]
async fn stalled_inbound_attempt_ends_on_the_hop() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let (gateway, ga) = station("N0GW").await;
            let (caller, ca) = station("N1CALL").await;
            let drop_audio = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let gate = drop_audio.clone();
            let gw_keyed = gateway.keyed.clone();
            let caller_keyed = caller.keyed.clone();
            tokio::task::spawn_local(async move {
                let (mut ga, mut ca) = (ga, ca);
                let mut tick = tokio::time::interval(Duration::from_millis(20));
                let mut buf = vec![0.0; 960];
                loop {
                    tick.tick().await;
                    cable_step(&mut ga.output, &mut ca.input, &caller_keyed, &mut buf);
                    if gate.load(std::sync::atomic::Ordering::SeqCst) {
                        ca.output.read(&mut buf);
                        ga.input.write(&[0.0; 960]);
                    } else {
                        cable_step(&mut ca.output, &mut ga.input, &gw_keyed, &mut buf);
                    }
                }
            });
            let mut bpq = BpqVara::open(&gateway).await;
            let mut pat = PatVara::open(&caller).await;
            // Lose the confirmation while the six-dwell pending hold expires.
            pat.command("CONNECT N1CALL N0GW-1", "OK").await;
            bpq.wait("PENDING", CONNECT_TIMEOUT).await;
            drop_audio.store(true, std::sync::atomic::Ordering::SeqCst);
            tokio::time::sleep(Duration::from_secs(18)).await;
            bpq.0.command("LISTEN OFF", "OK").await;
            bpq.wait("CANCELPENDING", CONNECT_TIMEOUT).await;
            bpq.0.command("LISTEN ON", "OK").await;
        })
        .await;
}
