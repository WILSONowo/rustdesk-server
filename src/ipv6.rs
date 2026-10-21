use hbb_common::{bytes::Bytes, log};
use std::net::{Ipv6Addr, SocketAddr};

// AddrMangle's IPv6 wire format is 16 address bytes followed by a LE port.
// Validate before decoding: its legacy IPv4 decoder is not an input validator.
fn invalid_reason(raw: &[u8]) -> Option<&'static str> {
    if raw.len() != 18 {
        return Some("invalid_length");
    }
    if u16::from_le_bytes([raw[16], raw[17]]) == 0 {
        return Some("zero_port");
    }
    let mut octets = [0; 16];
    octets.copy_from_slice(&raw[..16]);
    let ip = Ipv6Addr::from(octets);
    // Link-local addresses require a scope ID, which this wire format cannot
    // carry. ULA is allowed: peers can legitimately share a routed private LAN.
    if ip.is_unspecified() || ip.is_loopback() || ip.is_multicast()
        || (ip.segments()[0] & 0xffc0) == 0xfe80
        || ip.to_ipv4().is_some()
    {
        return Some("unusable_address");
    }
    None
}

pub(crate) fn candidate(raw: Bytes, allowed: bool, stage: &str, control: SocketAddr) -> Bytes {
    let control = hbb_common::try_into_v4(control);
    let reason = if raw.is_empty() {
        Some("absent")
    } else if !allowed {
        Some("policy_or_no_offer")
    } else {
        invalid_reason(&raw)
    };
    if let Some(reason) = reason {
        log::debug!("ipv6_candidate stage={stage} control={control} result=omitted reason={reason}");
        Bytes::new()
    } else {
        // A forwarded candidate does not prove that the endpoints connected.
        // Do not log the candidate payload or claim P2P success at hbbs.
        log::debug!("ipv6_candidate stage={stage} control={control} result=forwarded");
        raw
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hbb_common::AddrMangle;

    #[test]
    fn rejects_malformed_and_unusable_candidates_but_allows_routed_ipv6() {
        for size in [0, 1, 16, 17, 19, 1024] {
            assert_eq!(invalid_reason(&vec![1; size]), Some("invalid_length"));
        }
        for addr in ["[::]:1234", "[::1]:1234", "[ff02::1]:1234", "[fe80::1]:1234",
            "[::ffff:192.0.2.1]:1234", "[::192.0.2.1]:1234", "[2001:db8::1]:0"] {
            let addr: SocketAddr = addr.parse().unwrap();
            // Construct the raw IPv6 representation even for mapped IPv4.
            let SocketAddr::V6(addr) = addr else { panic!("IPv6 fixture") };
            let mut raw = addr.ip().octets().to_vec();
            raw.extend(addr.port().to_le_bytes());
            assert!(invalid_reason(&raw).is_some(), "{addr}");
        }
        for addr in ["[2001:db8::1]:1234", "[fd12:3456::1]:65535"] {
            let raw = AddrMangle::encode(addr.parse().unwrap());
            assert_eq!(invalid_reason(&raw), None);
        }
    }
}
