# IPv6 peer negotiation on the 1.1.16 baseline

hbbs forwards the existing `socket_addr_v6` fields used by official RustDesk
1.4.9 and 1.5.0 clients. This does not introduce a new client transport or require
custom client packages. The negotiated IPv6 data path is UDP/KCP, independently
of whether the clients reach hbbs over IPv4 or IPv6.

The protocol submodule is pinned to a commit in `WILSONowo/hbb_common` based on
the dependency shipped with official Server 1.1.16. Only additive IPv6 fields
and `force_relay` flags are backported, using upstream field numbers. This is
not a dependency upgrade to the development version of Server 1.1.17.

## Behavior

- Forward candidates through `PunchHole` and `FetchLocalAddr`, and return them
  through `PunchHoleResponse` or `RelayResponse`.
- Preserve the existing IPv4 addresses, public-key exchange and relay selection.
- Treat absent or invalid candidates as unavailable; keep the legacy paths.
- `ALWAYS_USE_RELAY=Y`, the client's `force_relay`, and the existing LAN mask
  relay policy suppress IPv6 candidates. Forced requests bypass `FetchLocalAddr`
  because that message has no force-relay flag. `PunchHole` carries both the
  explicit flag and the legacy symmetric-NAT indication.
- Toggling `always-use-relay` changes only the policy, preserving the configured
  relay list. The former command accidentally replaced that list with `Y`/`N`.
- WebSocket requests do not enable the native IPv6 path. Per-request candidate
  state expires with the existing pending TCP/WebSocket connection; return
  candidates are accepted only if a valid offer was forwarded. The global
  relay policy is checked again when the response is sent.
- Validate the 18-byte address/port encoding and a nonzero port. Reject
  unspecified, loopback, multicast, IPv4-compatible/mapped and link-local
  addresses. Link-local scope IDs cannot be represented in this format. Routed
  ULA addresses remain usable. Do not equate a candidate with the control
  connection's source address.

## Diagnostics

Set `RUST_LOG=info,hbbs=debug` when starting hbbs. Look for `ipv6_candidate`:

```text
ipv6_candidate stage=punch_hole control=... result=forwarded
ipv6_candidate stage=punch_hole_response control=... result=omitted reason=invalid_length
ipv6_candidate stage=relay_response control=... result=omitted reason=policy_or_no_offer
```

Stages identify the outgoing message. Reasons also include `absent`, `zero_port`,
`unusable_address` and `no_pending_request`. The control endpoint allows request
and response logs to be correlated; the candidate payload is not logged.
`forwarded` means the signaling field was accepted, **not** that P2P connected.
hbbs cannot observe the subsequent direct client-to-client data path. Use client
logs and actual traffic to distinguish IPv6 success from relay fallback.

## Validation

With native build prerequisites installed, set `DATABASE_URL` to a SQLite build
schema containing the `peer` table (the repository's `db_v2.sqlite3` can be used):

```sh
export DATABASE_URL="sqlite://$PWD/db_v2.sqlite3"
cargo test --locked -p hbbs --all-targets -- --test-threads=1
```

`tests/ipv6_negotiation.rs` starts isolated hbbs processes and simulates official
wire messages over TCP/UDP/WebSocket, including the signed encrypted handshake.
It covers normal, LAN and relay responses, missing/malformed candidates, legacy
fallback fields, forced-relay policies and a policy change during negotiation.
The documentation-prefix IPv6 candidates in these tests are deliberately not
contacted: these tests verify signaling, not Internet reachability.

Before release, test real official clients with two working IPv6 networks,
only one IPv6 endpoint, blocked IPv6 UDP, and IPv4 failure with working IPv6.
Check forced relay, mobile network switching and normal hbbr fallback. No API
or Web business changes are needed; all-in-one images that bundle Server must
eventually pin the reviewed Server release.
