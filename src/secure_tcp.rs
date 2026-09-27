//! Native TCP handshake used by official clients after API login.
use hbb_common::{
    bail, bytes::BytesMut, protobuf::Message,
    rendezvous_proto::{rendezvous_message, KeyExchange, RendezvousMessage},
    tcp::Encrypt, ResultType,
};
use sodiumoxide::crypto::{box_, secretbox, sign};

#[derive(Default)]
pub(crate) struct SecureTcp {
    pending_key: Option<box_::SecretKey>,
    decrypt: Option<Encrypt>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reply(offer: &RendezvousMessage, pk: &sign::PublicKey) -> (BytesMut, Encrypt) {
        let signed = &offer.key_exchange().keys[0];
        let verified = sign::verify(signed, pk).unwrap();
        let server = box_::PublicKey::from_slice(&verified).unwrap();
        let (client_pk, client_sk) = box_::gen_keypair();
        let key = secretbox::gen_key();
        let sealed = box_::seal(&key.0, &box_::Nonce([0; box_::NONCEBYTES]), &server, &client_sk);
        let mut msg = RendezvousMessage::new();
        msg.set_key_exchange(KeyExchange {
            keys: vec![client_pk.0.to_vec().into(), sealed.into()],
            ..Default::default()
        });
        (msg.write_to_bytes().unwrap().as_slice().into(), Encrypt::new(key))
    }

    fn request() -> Vec<u8> {
        let mut msg = RendezvousMessage::new();
        msg.set_test_nat_request(Default::default());
        msg.write_to_bytes().unwrap()
    }

    #[test]
    fn signed_offer_and_multiple_encrypted_frames() {
        let (pk, sk) = sign::gen_keypair();
        let (mut state, offer) = SecureTcp::offer(&sk);
        let (_, another_offer) = SecureTcp::offer(&sk);
        assert_ne!(offer.key_exchange().keys, another_offer.key_exchange().keys);
        let (wrong_pk, _) = sign::gen_keypair();
        assert!(sign::verify(&offer.key_exchange().keys[0], &wrong_pk).is_err());
        let (mut answer, mut client) = reply(&offer, &pk);
        let mut outbound = state.receive(&mut answer).unwrap().unwrap();
        for _ in 0..3 {
            let expected = request();
            let mut received = BytesMut::from(client.enc(&expected).as_slice());
            assert!(state.receive(&mut received).unwrap().is_none());
            assert_eq!(&received[..], &expected);
            let mut response = BytesMut::from(outbound.enc(&expected).as_slice());
            client.dec(&mut response).unwrap();
            assert_eq!(&response[..], &expected);
        }
    }

    #[test]
    fn malformed_exchanges_are_rejected_without_panics() {
        let (_, sk) = sign::gen_keypair();
        for keys in [vec![], vec![vec![0; 32]], vec![vec![0; 31], vec![0; 48]],
                     vec![vec![0; 32], vec![0; 47]], vec![vec![0; 32], vec![0; 48]],
                     vec![vec![0; 32], vec![0; 48], vec![]]] {
            let (mut state, _) = SecureTcp::offer(&sk);
            let mut msg = RendezvousMessage::new();
            msg.set_key_exchange(KeyExchange {
                keys: keys.into_iter().map(Into::into).collect(), ..Default::default()
            });
            let mut bytes = BytesMut::from(msg.write_to_bytes().unwrap().as_slice());
            assert!(state.receive(&mut bytes).is_err());
        }
    }

    #[test]
    fn legacy_plaintext_and_late_exchange() {
        let (pk, sk) = sign::gen_keypair();
        let (mut state, offer) = SecureTcp::offer(&sk);
        let mut plain = BytesMut::from(request().as_slice());
        assert!(state.receive(&mut plain).unwrap().is_none());
        let (mut answer, _) = reply(&offer, &pk);
        assert!(state.receive(&mut answer).is_err());
        assert!(SecureTcp::default().receive(&mut plain).unwrap().is_none());
    }

    #[test]
    fn encrypted_sessions_reject_replay_tampering_downgrade_and_rekey() {
        for attack in 0..5 {
            let (pk, sk) = sign::gen_keypair();
            let (mut state, offer) = SecureTcp::offer(&sk);
            let (mut answer, mut client) = reply(&offer, &pk);
            let exchange = answer.to_vec();
            state.receive(&mut answer).unwrap();
            let ciphertext = client.enc(&request());
            let mut first = BytesMut::from(ciphertext.as_slice());
            state.receive(&mut first).unwrap();
            let bad = match attack {
                0 => ciphertext,
                1 => { let mut b = client.enc(&request()); b[0] ^= 1; b },
                2 => request(),
                3 => client.enc(&exchange),
                _ => vec![0],
            };
            assert!(state.receive(&mut BytesMut::from(bad.as_slice())).is_err());
        }
    }
}

impl SecureTcp {
    pub fn offer(sk: &sign::SecretKey) -> (Self, RendezvousMessage) {
        // Each connection has its own ephemeral key; never log session keys.
        let (pk, secret) = box_::gen_keypair();
        let mut msg = RendezvousMessage::new();
        msg.set_key_exchange(KeyExchange {
            keys: vec![sign::sign(&pk.0, sk).into()],
            ..Default::default()
        });
        (Self { pending_key: Some(secret), decrypt: None }, msg)
    }

    // Some installs a separate outbound cipher. None delivers application data.
    pub fn receive(&mut self, bytes: &mut BytesMut) -> ResultType<Option<Encrypt>> {
        if let Some(cipher) = self.decrypt.as_mut() {
            if bytes.len() < secretbox::MACBYTES {
                bail!("Invalid encrypted TCP frame");
            }
            cipher.dec(bytes)?;
        }
        let msg = RendezvousMessage::parse_from_bytes(bytes)?;
        if let Some(rendezvous_message::Union::KeyExchange(exchange)) = msg.union {
            let Some(secret) = self.pending_key.take() else {
                bail!("Unexpected TCP key exchange");
            };
            if exchange.keys.len() != 2
                || exchange.keys[0].len() != box_::PUBLICKEYBYTES
                || exchange.keys[1].len() != secretbox::KEYBYTES + box_::MACBYTES
            {
                bail!("Invalid TCP key exchange");
            }
            let key = Encrypt::decode(&exchange.keys[1], &exchange.keys[0], &secret)?;
            self.decrypt = Some(Encrypt::new(key.clone()));
            return Ok(Some(Encrypt::new(key)));
        }
        // Official clients without API login skip the offer and use plaintext.
        // Once application traffic starts, a later handshake cannot reset state.
        self.pending_key = None;
        Ok(None)
    }
}
