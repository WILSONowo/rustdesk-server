//! Runs the actual hbbs executable against the native 1.4.9 handshake protocol.
use hbb_common::{
    protobuf::Message, rendezvous_proto::*, tcp::FramedStream, tokio,
    tokio::{io::{AsyncReadExt, AsyncWriteExt}, net::{TcpStream, UdpSocket}, time::{sleep, Duration}},
};
use sodiumoxide::crypto::{box_, secretbox, sign};
use std::{path::PathBuf, process::{Child, Command, Stdio}};

struct Server { child: Child, data: PathBuf, port: u16, pk: sign::PublicKey, key: String }
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.data);
    }
}
impl Server {
    async fn start() -> Self {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let data = std::env::temp_dir().join(format!("hbbs-handshake-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&data).unwrap();
        let binary = std::env::var("HBBS_TEST_BINARY").unwrap_or_else(|_| env!("CARGO_BIN_EXE_hbbs").into());
        let child = Command::new(binary)
            .args(["-k", "_", "-p", &port.to_string()])
            .current_dir(&data).env_remove("DB_URL")
            .stdout(Stdio::inherit()).stderr(Stdio::inherit()).spawn().unwrap();
        let mut server = Self { child, data, port, pk: sign::PublicKey([0; 32]), key: String::new() };
        for _ in 0..100 {
            assert!(server.child.try_wait().unwrap().is_none(), "hbbs exited during startup");
            if TcpStream::connect(format!("127.0.0.1:{port}")).await.is_ok() {
                server.key = std::fs::read_to_string(server.data.join("id_ed25519.pub")).unwrap().trim().into();
                server.pk = sign::PublicKey::from_slice(&base64::decode(&server.key).unwrap()).unwrap();
                return server;
            }
            sleep(Duration::from_millis(50)).await;
        }
        panic!("hbbs startup timed out");
    }

    async fn connect(&self, secure: bool) -> FramedStream {
        let mut conn = FramedStream::new(format!("127.0.0.1:{}", self.port), None, 2000).await.unwrap();
        let offer = next(&mut conn).await;
        assert!(matches!(offer.union, Some(rendezvous_message::Union::KeyExchange(_))));
        if secure {
            // Same signature verification, zero-nonce box and secretbox framing as client 1.4.9.
            assert_eq!(offer.key_exchange().keys.len(), 1);
            let pk = sign::verify(&offer.key_exchange().keys[0], &self.pk).unwrap();
            let pk = box_::PublicKey::from_slice(&pk).unwrap();
            let (client_pk, client_sk) = box_::gen_keypair();
            let key = secretbox::gen_key();
            let sealed = box_::seal(&key.0, &box_::Nonce([0; box_::NONCEBYTES]), &pk, &client_sk);
            let mut reply = RendezvousMessage::new();
            reply.set_key_exchange(KeyExchange {
                keys: vec![client_pk.0.to_vec().into(), sealed.into()], ..Default::default()
            });
            conn.send(&reply).await.unwrap();
            conn.set_key(key);
        }
        conn
    }
}
async fn next(conn: &mut FramedStream) -> RendezvousMessage {
    let bytes = conn.next_timeout(3000).await.expect("response timeout").unwrap();
    RendezvousMessage::parse_from_bytes(&bytes).unwrap()
}
async fn udp_next(socket: &UdpSocket) -> RendezvousMessage {
    let mut buffer = [0; 4096];
    let n = hbb_common::timeout(3000, socket.recv(&mut buffer)).await.unwrap().unwrap();
    RendezvousMessage::parse_from_bytes(&buffer[..n]).unwrap()
}

#[tokio::test]
async fn native_clients_plain_secure_delayed_replies_and_helper_port() {
    let server = Server::start().await;
    for secure in [false, true] {
        let mut conn = server.connect(secure).await;
        let mut request = RendezvousMessage::new();
        request.set_test_nat_request(Default::default());
        conn.send(&request).await.unwrap();
        assert!(matches!(next(&mut conn).await.union, Some(rendezvous_message::Union::TestNatResponse(_))));

        // Punch responses move their sink into tcp_punch before returning.
        let mut conn = server.connect(secure).await;
        request.set_punch_hole_request(PunchHoleRequest {
            id: "missing-peer".into(), licence_key: server.key.clone(), token: "test-api-token".into(),
            ..Default::default()
        });
        conn.send(&request).await.unwrap();
        assert_eq!(next(&mut conn).await.punch_hole_response().failure.enum_value().unwrap(), punch_hole_response::Failure::ID_NOT_EXIST);
    }

    // Register a UDP target, then deliver its TCP response to a secured TCP caller.
    let peer = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    peer.connect(format!("127.0.0.1:{}", server.port)).await.unwrap();
    let mut msg = RendezvousMessage::new();
    msg.set_register_pk(RegisterPk { id: "123456789".into(), uuid: vec![1; 16].into(), pk: vec![2; 32].into(), ..Default::default() });
    peer.send(&msg.write_to_bytes().unwrap()).await.unwrap();
    assert!(matches!(udp_next(&peer).await.union, Some(rendezvous_message::Union::RegisterPkResponse(_))));
    msg.set_register_peer(RegisterPeer { id: "123456789".into(), ..Default::default() });
    peer.send(&msg.write_to_bytes().unwrap()).await.unwrap();
    assert!(matches!(udp_next(&peer).await.union, Some(rendezvous_message::Union::RegisterPeerResponse(_))));
    let mut conn = server.connect(true).await;
    msg.set_punch_hole_request(PunchHoleRequest { id: "123456789".into(), licence_key: server.key.clone(), token: "test-api-token".into(), ..Default::default() });
    conn.send(&msg).await.unwrap();
    let routed = udp_next(&peer).await;
    let addr = match routed.union.unwrap() {
        rendezvous_message::Union::FetchLocalAddr(m) => m.socket_addr,
        rendezvous_message::Union::PunchHole(m) => m.socket_addr,
        _ => panic!("unexpected target request"),
    };
    msg.set_punch_hole_sent(PunchHoleSent { socket_addr: addr, id: "123456789".into(), ..Default::default() });
    let mut target = server.connect(false).await;
    target.send(&msg).await.unwrap();
    assert!(!next(&mut conn).await.punch_hole_response().socket_addr.is_empty());

    // API-login relay negotiation uses the same secured connection to hbbs.
    let mut conn = server.connect(true).await;
    msg.set_request_relay(RequestRelay { id: "123456789".into(), uuid: "relay-test".into(), token: "test-api-token".into(), ..Default::default() });
    conn.send(&msg).await.unwrap();
    let routed = udp_next(&peer).await;
    let addr = routed.request_relay().socket_addr.clone();
    assert!(!addr.is_empty());
    msg.set_relay_response(RelayResponse { socket_addr: addr, uuid: "relay-test".into(), relay_server: "127.0.0.1:21117".into(), ..Default::default() });
    let mut target = server.connect(false).await;
    target.send(&msg).await.unwrap();
    assert_eq!(next(&mut conn).await.relay_response().uuid, "relay-test");

    // WebSocket clients skip secure_tcp in 1.4.9; retain the existing wire format.
    use hbb_common::futures_util::{SinkExt, StreamExt};
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{}", server.port + 2)).await.unwrap();
    msg.set_test_nat_request(Default::default());
    ws.send(tungstenite::Message::Binary(msg.write_to_bytes().unwrap())).await.unwrap();
    let response = hbb_common::timeout(2000, ws.next()).await.unwrap().unwrap().unwrap().into_data();
    assert!(matches!(RendezvousMessage::parse_from_bytes(&response).unwrap().union, Some(rendezvous_message::Union::TestNatResponse(_))));

    // Local admin command port must not emit a binary key exchange greeting.
    let mut helper = TcpStream::connect(format!("127.0.0.1:{}", server.port - 1)).await.unwrap();
    helper.write_all(b"h").await.unwrap();
    let mut buf = [0; 4096];
    let n = hbb_common::timeout(2000, helper.read(&mut buf)).await.unwrap().unwrap();
    assert!(std::str::from_utf8(&buf[..n]).unwrap().contains("relay-servers"));
}
