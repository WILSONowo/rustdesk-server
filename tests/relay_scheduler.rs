//! Real hbbs/hbbr processes: protocol routing, private telemetry and node failure.
use hbb_common::{
    protobuf::Message,
    rendezvous_proto::*,
    tcp::FramedStream,
    tokio,
    tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::{TcpStream, UdpSocket},
        time::{sleep, Duration},
    },
};
use serde_json::{json, Value};
use std::{
    path::PathBuf,
    process::{Child, Command, Stdio},
};

const TOKEN: &str = "relay-integration-test-token-not-a-production-secret";
struct Process {
    child: Child,
    data: PathBuf,
    port: u16,
}
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.data);
    }
}
fn port() -> u16 {
    // Reserve a free base port with its neighbouring RustDesk listeners available.
    loop {
        let l = std::net::TcpListener::bind("0.0.0.0:0").unwrap();
        let p = l.local_addr().unwrap().port();
        if p < 65533
            && std::net::TcpListener::bind(("0.0.0.0", p + 2)).is_ok()
            && std::net::TcpListener::bind(("0.0.0.0", p - 1)).is_ok()
        {
            return p;
        }
    }
}
fn data() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rustdesk-relay-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&dir).unwrap();
    dir
}
async fn ready(process: &mut Process) {
    for _ in 0..100 {
        assert!(
            process.child.try_wait().unwrap().is_none(),
            "process exited during startup"
        );
        if TcpStream::connect(("127.0.0.1", process.port))
            .await
            .is_ok()
        {
            return;
        }
        sleep(Duration::from_millis(50)).await;
    }
    panic!("process did not start");
}
async fn relay(id: &str, metrics_port: u16) -> Process {
    let dir = data();
    let p = port();
    let binary =
        std::env::var("HBBR_TEST_BINARY").unwrap_or_else(|_| env!("CARGO_BIN_EXE_hbbr").into());
    let child = Command::new(binary)
        .args(["-p", &p.to_string(), "-k", "test-relay-key"])
        .env("RELAY_NODE_ID", id)
        .env("RELAY_METRICS_BIND", format!("127.0.0.1:{metrics_port}"))
        .env("RELAY_METRICS_TOKEN", TOKEN)
        .current_dir(&dir)
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let mut process = Process {
        child,
        data: dir,
        port: p,
    };
    ready(&mut process).await;
    process
}
async fn admin(server: &Process, command: &str) -> String {
    let mut conn = TcpStream::connect(("127.0.0.1", server.port - 1))
        .await
        .unwrap();
    conn.write_all(command.as_bytes()).await.unwrap();
    let mut result = String::new();
    hbb_common::timeout(3000, conn.read_to_string(&mut result))
        .await
        .unwrap()
        .unwrap();
    result
}
async fn wait_health(server: &Process, expected: &[bool]) {
    for _ in 0..100 {
        let status: Value = serde_json::from_str(&admin(server, "rst").await).unwrap();
        if expected
            .iter()
            .enumerate()
            .all(|(i, value)| status[i]["healthy"] == *value)
        {
            return;
        }
        sleep(Duration::from_millis(100)).await;
    }
    panic!("unexpected relay health: {}", admin(server, "rst").await);
}
async fn connect(server: &Process) -> FramedStream {
    let mut conn = FramedStream::new(format!("127.0.0.1:{}", server.port), None, 2000)
        .await
        .unwrap();
    assert!(matches!(
        next(&mut conn).await.union,
        Some(rendezvous_message::Union::KeyExchange(_))
    ));
    conn
}
async fn next(conn: &mut FramedStream) -> RendezvousMessage {
    RendezvousMessage::parse_from_bytes(&conn.next_timeout(3000).await.unwrap().unwrap()).unwrap()
}
async fn udp_next(peer: &UdpSocket) -> RendezvousMessage {
    let mut buf = [0; 4096];
    let n = hbb_common::timeout(3000, peer.recv(&mut buf))
        .await
        .unwrap()
        .unwrap();
    RendezvousMessage::parse_from_bytes(&buf[..n]).unwrap()
}
async fn selected(server: &Process, peer: &UdpSocket, key: &str) -> String {
    let mut conn = connect(server).await;
    let mut msg = RendezvousMessage::new();
    msg.set_punch_hole_request(PunchHoleRequest {
        id: "relay-target".into(),
        licence_key: key.into(),
        ..Default::default()
    });
    conn.send(&msg).await.unwrap();
    match udp_next(peer).await.union.unwrap() {
        rendezvous_message::Union::FetchLocalAddr(m) => m.relay_server,
        rendezvous_message::Union::PunchHole(m) => m.relay_server,
        other => panic!("unexpected {other:?}"),
    }
}
async fn metrics(client: &reqwest::Client, port: u16) -> Value {
    client
        .get(format!("http://127.0.0.1:{port}/metrics"))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap()
}

#[tokio::test]
async fn routes_new_sessions_away_from_failed_nodes_and_preserves_active_relays() {
    let m0 = port();
    let m1 = port();
    let r0 = relay("first", m0).await;
    let mut r1 = relay("second", m1).await;
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(2))
        .build()
        .unwrap();
    let response = client
        .get(format!("http://127.0.0.1:{m0}/metrics"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 401);
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert_eq!(metrics(&client, m0).await["active_sessions"], 0);

    let directory = data();
    let p = port();
    let node = |id, relay_port, metrics_port| {
        json!({
            "id": id, "address": format!("127.0.0.1:{relay_port}"),
            "metrics_url": format!("http://127.0.0.1:{metrics_port}/metrics"), "token_env": "TEST_METRICS_TOKEN",
            "location": {"latitude":0.0,"longitude":0.0}, "max_sessions":100, "bandwidth_mbps":1000.0
        })
    };
    let config = json!({"poll_seconds":1,"timeout_ms":500,"stale_seconds":3,
        "nodes":[node("first",r0.port,m0),node("second",r1.port,m1)]});
    std::fs::write(
        directory.join("scheduler.json"),
        serde_json::to_vec(&config).unwrap(),
    )
    .unwrap();
    std::fs::write(
        directory.join(".env"),
        format!("RELAY_SCHEDULER_CONFIG=scheduler.json\nTEST_METRICS_TOKEN={TOKEN}\n"),
    )
    .unwrap();
    let binary =
        std::env::var("HBBS_TEST_BINARY").unwrap_or_else(|_| env!("CARGO_BIN_EXE_hbbs").into());
    let child = Command::new(binary)
        .args(["-p", &p.to_string(), "-k", "_"])
        .env("ALWAYS_USE_RELAY", "Y")
        .env_remove("DB_URL")
        .current_dir(&directory)
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let mut server = Process {
        child,
        data: directory,
        port: p,
    };
    ready(&mut server).await;
    wait_health(&server, &[true, true]).await;
    let key = std::fs::read_to_string(server.data.join("id_ed25519.pub")).unwrap();
    let peer = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    peer.connect(("127.0.0.1", server.port)).await.unwrap();
    let mut msg = RendezvousMessage::new();
    msg.set_register_pk(RegisterPk {
        id: "relay-target".into(),
        uuid: vec![1; 16].into(),
        pk: vec![2; 32].into(),
        ..Default::default()
    });
    peer.send(&msg.write_to_bytes().unwrap()).await.unwrap();
    udp_next(&peer).await;
    msg.set_register_peer(RegisterPeer {
        id: "relay-target".into(),
        ..Default::default()
    });
    peer.send(&msg.write_to_bytes().unwrap()).await.unwrap();
    udp_next(&peer).await;
    assert_eq!(
        selected(&server, &peer, key.trim()).await,
        format!("127.0.0.1:{}", r0.port)
    );
    assert_eq!(
        selected(&server, &peer, key.trim()).await,
        format!("127.0.0.1:{}", r1.port)
    );

    // Native relay framing over the container's non-loopback IP. Loopback TCP is hbbr's admin port.
    let ip = local_ip_address::local_ip().unwrap();
    assert!(
        !ip.is_loopback(),
        "run this integration test in the documented Docker environment"
    );
    let address = std::net::SocketAddr::new(ip, r0.port).to_string();
    let mut a = FramedStream::new(address.clone(), None, 2000)
        .await
        .unwrap();
    let mut b = FramedStream::new(address, None, 2000).await.unwrap();
    msg.set_request_relay(RequestRelay {
        uuid: "live-session".into(),
        licence_key: "test-relay-key".into(),
        ..Default::default()
    });
    a.send(&msg).await.unwrap();
    b.send(&msg).await.unwrap();
    for _ in 0..40 {
        if metrics(&client, m0).await["active_sessions"] == 1 {
            break;
        }
        sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(metrics(&client, m0).await["active_sessions"], 1);
    a.set_raw();
    b.set_raw();
    a.send_raw(b"before-failure".to_vec()).await.unwrap();
    assert_eq!(
        &b.next_timeout(2000).await.unwrap().unwrap()[..],
        b"before-failure"
    );
    let state = metrics(&client, m0).await;
    assert_eq!(state["active_sessions"], 1);
    assert!(state["forwarded_bytes"].as_u64().unwrap() >= 14);
    assert!(state.get("peers").is_none());

    r1.child.kill().unwrap();
    r1.child.wait().unwrap();
    wait_health(&server, &[true, false]).await;
    assert_eq!(
        selected(&server, &peer, key.trim()).await,
        format!("127.0.0.1:{}", r0.port)
    );
    b.send_raw(b"after-failure".to_vec()).await.unwrap();
    assert_eq!(
        &a.next_timeout(2000).await.unwrap().unwrap()[..],
        b"after-failure"
    );

    // Refuse an already-negotiating failed address instead of sending peers to different relays.
    let mut conn = connect(&server).await;
    msg.set_request_relay(RequestRelay {
        id: "relay-target".into(),
        uuid: "stale".into(),
        relay_server: format!("127.0.0.1:{}", r1.port),
        ..Default::default()
    });
    conn.send(&msg).await.unwrap();
    assert!(next(&mut conn)
        .await
        .relay_response()
        .refuse_reason
        .contains("reconnect"));
    // A NAT-symmetric target can initiate a relay response without RequestRelay.
    let mut initiator = connect(&server).await;
    msg.set_punch_hole_request(PunchHoleRequest {
        id: "relay-target".into(), licence_key: key.trim().into(), ..Default::default()
    });
    initiator.send(&msg).await.unwrap();
    let destination = match udp_next(&peer).await.union.unwrap() {
        rendezvous_message::Union::FetchLocalAddr(m) => m.socket_addr,
        rendezvous_message::Union::PunchHole(m) => m.socket_addr,
        other => panic!("unexpected {other:?}"),
    };
    let mut target = connect(&server).await;
    msg.set_relay_response(RelayResponse {
        socket_addr: destination, relay_server: format!("127.0.0.1:{}", r1.port),
        uuid: "target-fallback".into(), ..Default::default()
    });
    target.send(&msg).await.unwrap();
    let response = next(&mut initiator).await;
    assert!(response.relay_response().relay_server.is_empty());
    assert!(response.relay_response().refuse_reason.contains("reconnect"));
    drop(a);
    drop(b);
    for _ in 0..30 {
        if metrics(&client, m0).await["active_sessions"] == 0 {
            break;
        }
        sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(metrics(&client, m0).await["active_sessions"], 0);

    drop(r0);
    wait_health(&server, &[false, false]).await;
    let mut conn = connect(&server).await;
    msg.set_punch_hole_request(PunchHoleRequest {
        id: "relay-target".into(),
        licence_key: key.trim().into(),
        ..Default::default()
    });
    conn.send(&msg).await.unwrap();
    assert!(next(&mut conn)
        .await
        .punch_hole_response()
        .other_failure
        .contains("No healthy relay"));
    admin(&server, "aur N").await;
    assert_eq!(
        selected(&server, &peer, key.trim()).await,
        "",
        "direct P2P can still be negotiated without any relay"
    );
}
