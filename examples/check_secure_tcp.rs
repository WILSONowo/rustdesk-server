//! Read-only protocol probe: no peer registration, account mutation, or secrets.
use hbb_common::{
    bail, protobuf::Message, rendezvous_proto::*, tcp::FramedStream, tokio, ResultType,
};
use sodiumoxide::crypto::{box_, secretbox, sign};

async fn next(conn: &mut FramedStream) -> ResultType<RendezvousMessage> {
    match conn.next_timeout(10_000).await {
        Some(Ok(bytes)) => Ok(RendezvousMessage::parse_from_bytes(&bytes)?),
        Some(Err(err)) => Err(err.into()),
        None => bail!("Timed out or connection closed"),
    }
}

async fn connect(addr: &str, public_key: &str, secure: bool) -> ResultType<FramedStream> {
    let mut conn = FramedStream::new(addr, None, 10_000).await?;
    let offer = next(&mut conn).await?;
    let exchange = match offer.union {
        Some(rendezvous_message::Union::KeyExchange(exchange)) if exchange.keys.len() == 1 => exchange,
        _ => bail!("Expected signed KeyExchange offer"),
    };
    let decoded = base64::decode(public_key)?;
    let Some(sign_pk) = sign::PublicKey::from_slice(&decoded) else {
        bail!("Invalid configured public key");
    };
    let verified = match sign::verify(&exchange.keys[0], &sign_pk) {
        Ok(pk) => pk,
        Err(_) => bail!("Server signature mismatch"),
    };
    let Some(server_pk) = box_::PublicKey::from_slice(&verified) else {
        bail!("Invalid ephemeral public key");
    };
    if secure {
        let (client_pk, client_sk) = box_::gen_keypair();
        let key = secretbox::gen_key();
        let sealed = box_::seal(&key.0, &box_::Nonce([0; box_::NONCEBYTES]), &server_pk, &client_sk);
        let mut reply = RendezvousMessage::new();
        reply.set_key_exchange(KeyExchange {
            keys: vec![client_pk.0.to_vec().into(), sealed.into()],
            ..Default::default()
        });
        conn.send(&reply).await?;
        conn.set_key(key);
    }
    Ok(conn)
}

#[tokio::main]
async fn main() -> ResultType<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 {
        bail!("Usage: check_secure_tcp HOST:PORT PUBLIC_KEY");
    }
    let (addr, key) = (&args[1], &args[2]);
    for secure in [false, true] {
        let mut conn = connect(addr, key, secure).await?;
        let mut request = RendezvousMessage::new();
        request.set_test_nat_request(Default::default());
        conn.send(&request).await?;
        if !matches!(next(&mut conn).await?.union, Some(rendezvous_message::Union::TestNatResponse(_))) {
            bail!("Unexpected NAT response");
        }
    }
    let mut conn = connect(addr, key, true).await?;
    let mut request = RendezvousMessage::new();
    request.set_punch_hole_request(PunchHoleRequest {
        id: format!("deployment-probe-{}", uuid::Uuid::new_v4()),
        licence_key: key.clone(),
        ..Default::default()
    });
    conn.send(&request).await?;
    match next(&mut conn).await?.union {
        Some(rendezvous_message::Union::PunchHoleResponse(response))
            if response.failure.enum_value().ok() == Some(punch_hole_response::Failure::ID_NOT_EXIST) => {}
        _ => bail!("Unexpected encrypted peer lookup response"),
    }
    println!("{addr}: signature verified; plaintext NAT, encrypted NAT and encrypted peer lookup OK");
    Ok(())
}
