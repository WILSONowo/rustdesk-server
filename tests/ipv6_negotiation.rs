//! Run the real hbbs with two simulated clients. Requests use the field numbers
//! in the official 1.4.9/1.5.0 schema rather than our generated request setters.
//! This verifies signaling, not the reachability of the advertised IPv6 hosts.
use hbb_common::{protobuf::Message, rendezvous_proto::*, AddrMangle};
use std::{
    fs,
    io::{Read, Write},
    net::{TcpListener, TcpStream, UdpSocket},
    path::PathBuf,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

const PEER: &str = "ipv6-test-peer";
const RELAY: &str = "127.0.0.1:21117";

fn bytes_field(field: u8, value: &[u8]) -> Vec<u8> {
    assert!(value.len() < 128);
    let mut wire = vec![(field << 3) | 2, value.len() as u8];
    wire.extend(value);
    wire
}

fn envelope(field: u8, value: &[u8]) -> Vec<u8> {
    // Envelope field numbers exceed 15, so encode their tags as varints.
    let tag = (u16::from(field) << 3) | 2;
    let mut wire = if tag >= 128 { vec![(tag as u8 & 127) | 128, (tag >> 7) as u8] }
        else { vec![tag as u8] };
    assert!(value.len() < 128);
    wire.push(value.len() as u8);
    wire.extend(value);
    wire
}

fn candidate(ip: &str, port: u16) -> Vec<u8> {
    AddrMangle::encode(format!("[{ip}]:{port}").parse().unwrap())
}

fn request(version: &str, v6: &[u8], force: bool) -> Vec<u8> {
    request_with_key(version, v6, force, b"")
}

fn request_with_key(version: &str, v6: &[u8], force: bool, key: &[u8]) -> Vec<u8> {
    let mut ph = bytes_field(1, PEER.as_bytes());
    ph.extend([2 << 3, 1]); // ASYMMETRIC
    ph.extend(bytes_field(3, key));
    ph.extend(bytes_field(6, version.as_bytes()));
    ph.extend([8 << 3, u8::from(force)]);
    ph.extend(bytes_field(10, v6));
    envelope(8, &ph)
}

fn send(stream: &mut TcpStream, bytes: &[u8]) {
    assert!(bytes.len() < 16384);
    let header = if bytes.len() < 64 { vec![(bytes.len() << 2) as u8] }
        else { (((bytes.len() as u16) << 2) | 1).to_le_bytes().to_vec() };
    stream.write_all(&header).unwrap();
    stream.write_all(bytes).unwrap();
}

fn receive_bytes(stream: &mut TcpStream) -> Vec<u8> {
    let mut head = [0; 4];
    stream.read_exact(&mut head[..1]).unwrap();
    let size = (head[0] & 3) as usize + 1;
    stream.read_exact(&mut head[1..size]).unwrap();
    let length = (u32::from_le_bytes(head) >> 2) as usize;
    assert!(length < 65536);
    let mut bytes = vec![0; length];
    stream.read_exact(&mut bytes).unwrap();
    bytes
}

fn receive(stream: &mut TcpStream) -> RendezvousMessage {
    RendezvousMessage::parse_from_bytes(&receive_bytes(stream)).unwrap()
}

struct Server {
    child: Child,
    dir: PathBuf,
    port: u16,
}

impl Server {
    fn start(force: bool, mask: bool) -> Self {
        Self::start_with_key(force, mask, "")
    }

    fn start_with_key(force: bool, mask: bool, key: &str) -> Self {
        Self::start_with_options(force, mask, key, false)
    }

    fn start_with_options(force: bool, mask: bool, key: &str, scheduler: bool) -> Self {
        let port = loop {
            // Stay below the normal ephemeral range: a startup probe must not
            // accidentally self-connect before the child binds its listener.
            let random = uuid::Uuid::new_v4();
            let port = 16000 + u16::from_le_bytes([random.as_bytes()[0], random.as_bytes()[1]]) % 14000;
            let Ok(_reservation) = TcpListener::bind(("127.0.0.1", port)) else { continue };
            if TcpListener::bind(("127.0.0.1", port - 1)).is_ok()
                && TcpListener::bind(("127.0.0.1", port + 2)).is_ok()
                && UdpSocket::bind(("127.0.0.1", port)).is_ok()
            { break port; }
        };
        let dir = std::env::temp_dir().join(format!("hbbs-ipv6-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&dir).unwrap();
        let log = fs::File::create(dir.join("hbbs.log")).unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_hbbs"));
        command.current_dir(&dir).args(["-p", &port.to_string(), "-k", key, "-r", RELAY])
            .env("DB_URL", dir.join("peers.sqlite3"))
            .env("TEST_HBBS", "no")
            .env("ALWAYS_USE_RELAY", if force { "Y" } else { "N" })
            .env("RUST_LOG", "info,hbbs=debug")
            .env("TOKIO_WORKER_THREADS", "2")
            .env_remove("RELAY_SCHEDULER_CONFIG")
            .env_remove("RELAY-SCHEDULER-CONFIG")
            .stdout(Stdio::from(log.try_clone().unwrap())).stderr(Stdio::from(log));
        if mask { command.args(["--mask", "127.0.0.1/32"]).env("LOCAL-IP", "127.0.0.1"); }
        if scheduler {
            let config = dir.join("scheduler.json");
            fs::write(&config, serde_json::json!({
                "poll_seconds": 1, "timeout_ms": 100, "stale_seconds": 3,
                "recovery_successes": 2,
                "nodes": [{ "id": "unavailable", "address": "127.0.0.1:9",
                    "metrics_url": "http://127.0.0.1:9/metrics", "token_env": "IPV6_TEST_METRICS_TOKEN",
                    "location": {"latitude": 0, "longitude": 0},
                    "max_sessions": 10, "bandwidth_mbps": 100 }]
            }).to_string()).unwrap();
            command.env("RELAY_SCHEDULER_CONFIG", config)
                .env("IPV6_TEST_METRICS_TOKEN", "local-test-token-not-a-secret-12345");
        }
        let child = command.spawn().unwrap();
        let mut server = Self { child, dir, port };
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            assert!(server.child.try_wait().unwrap().is_none(), "hbbs exited during startup");
            // A listening port alone does not establish that hbbs is ready to
            // process messages. Require an application response from its loop.
            let ready = (|| -> std::io::Result<bool> {
                let mut probe = TcpStream::connect(("127.0.0.1", port - 1))?;
                probe.set_read_timeout(Some(Duration::from_millis(200)))?;
                probe.set_write_timeout(Some(Duration::from_millis(200)))?;
                probe.write_all(b"relay-servers")?;
                let mut reply = String::new();
                probe.read_to_string(&mut reply)?;
                Ok(reply.contains(RELAY) || reply.contains("Adaptive scheduler enabled"))
            })().unwrap_or(false);
            if ready { break; }
            assert!(Instant::now() < deadline, "hbbs startup timed out");
            thread::sleep(Duration::from_millis(20));
        }
        server
    }

    fn connect(&self) -> TcpStream {
        let stream = TcpStream::connect(("127.0.0.1", self.port)).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        stream.set_write_timeout(Some(Duration::from_secs(3))).unwrap();
        stream
    }

    fn peer(&self, same_public_ip: bool) -> UdpSocket {
        let peer = UdpSocket::bind((if same_public_ip { "127.0.0.1" } else { "127.0.0.2" }, 0)).unwrap();
        peer.connect(("127.0.0.1", self.port)).unwrap();
        peer.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        let mut message = RendezvousMessage::new();
        message.set_register_pk(RegisterPk {
            id: PEER.into(), uuid: vec![1; 16].into(), pk: vec![2; 32].into(), ..Default::default()
        });
        peer.send(&message.write_to_bytes().unwrap()).unwrap();
        assert_eq!(udp_receive(&peer).register_pk_response().result.enum_value().unwrap(), register_pk_response::Result::OK);
        message.set_register_peer(RegisterPeer { id: PEER.into(), ..Default::default() });
        peer.send(&message.write_to_bytes().unwrap()).unwrap();
        assert!(matches!(udp_receive(&peer).union, Some(rendezvous_message::Union::RegisterPeerResponse(_))));
        peer
    }

    fn reply(&self, route: &[u8], v6: &[u8], kind: u8) {
        let mut reply = bytes_field(1, route);
        match kind {
            10 => { // PunchHoleSent: v6 field 7
                reply.extend(bytes_field(2, PEER.as_bytes()));
                reply.extend(bytes_field(3, RELAY.as_bytes()));
                reply.extend([4 << 3, 1]);
                reply.extend(bytes_field(5, b"1.5.0"));
                reply.extend(bytes_field(7, v6));
            }
            13 => { // LocalAddr: v6 field 6
                reply.extend(bytes_field(2, &AddrMangle::encode("192.168.1.10:12345".parse().unwrap())));
                reply.extend(bytes_field(3, RELAY.as_bytes()));
                reply.extend(bytes_field(4, PEER.as_bytes()));
                reply.extend(bytes_field(5, b"1.4.9"));
                reply.extend(bytes_field(6, v6));
            }
            19 => { // RelayResponse: v6 field 10, unknown fields must survive.
                reply.extend(bytes_field(2, b"relay-session"));
                reply.extend(bytes_field(3, RELAY.as_bytes()));
                reply.extend(bytes_field(10, v6));
                reply.extend(bytes_field(12, b"future-field"));
            }
            _ => panic!("unknown reply fixture"),
        }
        send(&mut self.connect(), &envelope(kind, &reply));
    }

    fn force_relay_now(&self) {
        let mut stream = TcpStream::connect(("127.0.0.1", self.port - 1)).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        stream.write_all(b"always-use-relay Y").unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        // The setter has an empty response; query separately to confirm state.
        let mut query = TcpStream::connect(("127.0.0.1", self.port - 1)).unwrap();
        query.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        query.write_all(b"always-use-relay").unwrap();
        query.read_to_string(&mut response).unwrap();
        assert!(response.contains("true"), "{response}");
        let mut query = TcpStream::connect(("127.0.0.1", self.port - 1)).unwrap();
        query.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        query.write_all(b"relay-servers").unwrap();
        response.clear();
        query.read_to_string(&mut response).unwrap();
        assert!(response.contains(RELAY), "toggling relay policy changed the relay list: {response}");
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if thread::panicking() {
            eprintln!("hbbs log: {}", fs::read_to_string(self.dir.join("hbbs.log")).unwrap_or_default());
        }
        let _ = fs::remove_dir_all(&self.dir);
    }
}

#[track_caller]
fn udp_receive(peer: &UdpSocket) -> RendezvousMessage {
    let mut raw = [0; 65536];
    let n = peer.recv(&mut raw).unwrap();
    RendezvousMessage::parse_from_bytes(&raw[..n]).unwrap()
}

fn forwarded(message: RendezvousMessage, lan: bool) -> (Vec<u8>, Vec<u8>, bool) {
    match message.union.unwrap() {
        rendezvous_message::Union::PunchHole(p) if !lan => {
            if p.force_relay { assert_eq!(p.nat_type.enum_value().unwrap(), NatType::SYMMETRIC); }
            (p.socket_addr.to_vec(), p.socket_addr_v6.to_vec(), p.force_relay)
        }
        rendezvous_message::Union::FetchLocalAddr(p) if lan =>
            (p.socket_addr.to_vec(), p.socket_addr_v6.to_vec(), false),
        other => panic!("unexpected forwarded message: {other:?}"),
    }
}

fn returned(message: &RendezvousMessage) -> &[u8] {
    match message.union.as_ref().unwrap() {
        rendezvous_message::Union::PunchHoleResponse(p) => &p.socket_addr_v6,
        rendezvous_message::Union::RelayResponse(r) => &r.socket_addr_v6,
        other => panic!("unexpected response: {other:?}"),
    }
}

#[test]
fn official_wire_candidates_survive_normal_lan_and_relay_response_paths() {
    for lan in [false, true] {
        let server = Server::start(false, false);
        let peer = server.peer(lan);
        for version in ["1.4.9", "1.5.0"] {
            for kind in [if lan { 13 } else { 10 }, 19] {
                let a = candidate("2001:db8::a", 41001);
                let b = candidate("fd12:3456::b", 41002);
                let mut client = server.connect();
                send(&mut client, &request(version, &a, false));
                let (route, actual, forced) = forwarded(udp_receive(&peer), lan);
                assert_eq!(actual, a);
                assert!(!forced);
                server.reply(&route, &b, kind);
                let response = receive(&mut client);
                assert_eq!(returned(&response), b);
                if kind == 19 {
                    let r = response.relay_response();
                    assert_eq!(r.uuid, "relay-session");
                    assert_eq!(r.relay_server, RELAY);
                    assert!(r.special_fields.unknown_fields().get(12).is_some());
                } else {
                    assert_eq!(response.punch_hole_response().is_local(), lan);
                    assert!(!response.punch_hole_response().socket_addr.is_empty());
                }
            }
        }
    }
}

#[test]
fn invalid_missing_and_unoffered_candidates_leave_legacy_paths_intact() {
    let server = Server::start(false, false);
    let peer = server.peer(false);
    let valid = candidate("2001:db8::a", 41001);
    let invalid = [vec![], vec![255; 17], candidate("fe80::1", 41001), candidate("2001:db8::1", 0)];
    for offered in &invalid {
        for kind in [10, 19] {
            let mut client = server.connect();
            send(&mut client, &request("1.4.9", offered, false));
            let (route, actual, forced) = forwarded(udp_receive(&peer), false);
            assert!(actual.is_empty());
            assert!(!forced);
            server.reply(&route, &valid, kind);
            let response = receive(&mut client);
            assert!(returned(&response).is_empty());
        }
    }
    for reply in &invalid {
        let mut client = server.connect();
        send(&mut client, &request("1.5.0", &valid, false));
        let (route, _, _) = forwarded(udp_receive(&peer), false);
        server.reply(&route, reply, 10);
        let response = receive(&mut client);
        assert!(returned(&response).is_empty());
        assert!(!response.punch_hole_response().socket_addr.is_empty());
        assert_eq!(response.punch_hole_response().relay_server, RELAY);
    }
}

#[test]
fn forced_relay_blocks_ipv6_and_bypasses_the_same_public_ip_lan_path() {
    for (server_force, client_force, mask, same_ip) in [
        (true, false, false, true), (false, true, false, true),
        (true, false, false, false), (false, true, false, false),
        (false, false, true, false),
    ] {
        let server = Server::start(server_force, mask);
        let peer = server.peer(same_ip);
        for kind in [10, 13, 19] {
            let mut client = server.connect();
            let valid = candidate("2001:db8::1", 41001);
            send(&mut client, &request("1.5.0", &valid, client_force));
            let (route, actual, forced) = forwarded(udp_receive(&peer), false);
            assert!(forced);
            assert!(actual.is_empty());
            // Even a peer that ignores the policy cannot return an IPv6 route.
            server.reply(&route, &valid, kind);
            assert!(returned(&receive(&mut client)).is_empty());
        }
    }
}

#[test]
fn relay_policy_is_rechecked_when_a_pending_response_arrives() {
    let server = Server::start(false, false);
    let peer = server.peer(false);
    let valid = candidate("2001:db8::1", 41001);
    let mut client = server.connect();
    send(&mut client, &request("1.5.0", &valid, false));
    let (route, actual, _) = forwarded(udp_receive(&peer), false);
    assert_eq!(actual, valid);
    server.force_relay_now();
    server.reply(&route, &valid, 19);
    assert!(returned(&receive(&mut client)).is_empty());
}

#[test]
fn websocket_requests_do_not_enable_the_native_ipv6_path() {
    let server = Server::start(false, false);
    let peer = server.peer(true);
    let stream = TcpStream::connect(("127.0.0.1", server.port + 2)).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
    let (mut ws, _) = tungstenite::client(format!("ws://127.0.0.1:{}", server.port + 2), stream).unwrap();
    let valid = candidate("2001:db8::1", 41001);
    ws.write_message(tungstenite::Message::Binary(request("1.5.0", &valid, false))).unwrap();
    let (route, actual, _) = forwarded(udp_receive(&peer), false);
    assert!(actual.is_empty());
    server.reply(&route, &valid, 19);
    let response = RendezvousMessage::parse_from_bytes(&ws.read_message().unwrap().into_data()).unwrap();
    assert!(returned(&response).is_empty());
}

#[test]
fn signed_encrypted_api_login_signaling_preserves_ipv6_candidates() {
    use hbb_common::{bytes::BytesMut, tcp::Encrypt};
    use sodiumoxide::crypto::{box_, secretbox, sign};
    sodiumoxide::init().unwrap();
    let server = Server::start_with_key(false, false, "-");
    let peer = server.peer(false);
    let mut client = server.connect();
    let offer = receive(&mut client);
    let public = fs::read_to_string(server.dir.join("id_ed25519.pub")).unwrap();
    let pk = sign::PublicKey::from_slice(&base64::decode(public.trim()).unwrap()).unwrap();
    let signed = sign::verify(&offer.key_exchange().keys[0], &pk).unwrap();
    let server_box = box_::PublicKey::from_slice(&signed).unwrap();
    let (client_pk, client_sk) = box_::gen_keypair();
    let key = secretbox::gen_key();
    let sealed = box_::seal(&key.0, &box_::Nonce([0; box_::NONCEBYTES]), &server_box, &client_sk);
    let mut answer = RendezvousMessage::new();
    answer.set_key_exchange(KeyExchange {
        keys: vec![client_pk.0.to_vec().into(), sealed.into()], ..Default::default()
    });
    send(&mut client, &answer.write_to_bytes().unwrap());
    let mut cipher = Encrypt::new(key);
    let a = candidate("2001:db8::a", 41001);
    let b = candidate("2001:db8::b", 41002);
    send(&mut client, &cipher.enc(&request_with_key("1.5.0", &a, false, public.trim().as_bytes())));
    let (route, actual, _) = forwarded(udp_receive(&peer), false);
    assert_eq!(actual, a);
    server.reply(&route, &b, 10);
    let mut bytes = BytesMut::from(receive_bytes(&mut client).as_slice());
    cipher.dec(&mut bytes).unwrap();
    let response = RendezvousMessage::parse_from_bytes(&bytes).unwrap();
    assert_eq!(returned(&response), b);
    assert!(!response.punch_hole_response().pk.is_empty());
}

#[test]
fn unavailable_relays_do_not_block_direct_ipv6_but_forced_relay_fails_closed() {
    let server = Server::start_with_options(false, false, "", true);
    let peer = server.peer(false);
    let valid = candidate("2001:db8::1", 41001);
    let mut client = server.connect();
    send(&mut client, &request("1.5.0", &valid, false));
    let message = udp_receive(&peer);
    assert!(message.punch_hole().relay_server.is_empty());
    let (route, actual, forced) = forwarded(message, false);
    assert_eq!(actual, valid);
    assert!(!forced);
    server.reply(&route, &valid, 10);
    assert_eq!(returned(&receive(&mut client)), valid);

    let mut client = server.connect();
    send(&mut client, &request("1.5.0", &valid, true));
    let response = receive(&mut client);
    assert!(response.punch_hole_response().other_failure.contains("No healthy relay"));
    assert!(returned(&response).is_empty());
    peer.set_nonblocking(true).unwrap();
    assert_eq!(peer.recv(&mut [0; 256]).unwrap_err().kind(), std::io::ErrorKind::WouldBlock);
}
