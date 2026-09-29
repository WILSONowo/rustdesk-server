//! Opt-in relay selection without changing the official client protocol.
use crate::relay_telemetry::Snapshot;
use hbb_common::{
    bail,
    futures::future::join_all,
    log,
    tokio::{self, net::TcpStream, time::sleep},
    ResultType,
};
use ipnetwork::IpNetwork;
use serde_derive::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    net::IpAddr,
    path::Path,
    sync::{Arc, RwLock},
    time::{Duration, Instant},
};

fn setting(name: &str) -> Result<String, std::env::VarError> {
    // hbbs normalizes keys loaded from .env / -c to uppercase hyphenated names.
    match std::env::var(name.to_uppercase().replace('_', "-")) {
        Err(std::env::VarError::NotPresent) => std::env::var(name),
        value => value,
    }
}

fn relay_endpoint(address: &str) -> Option<(String, u16)> {
    if let Ok(ip) = address.parse::<IpAddr>() {
        return Some((ip.to_string(), 21117));
    }
    let url = reqwest::Url::parse(&format!("tcp://{address}")).ok()?;
    if !url.username().is_empty() || url.password().is_some() || !url.path().is_empty()
        || url.query().is_some() || url.fragment().is_some() {
        return None;
    }
    let host = url.host_str()?.trim_matches(['[', ']']).trim_end_matches('.').to_ascii_lowercase();
    let host = host.parse::<IpAddr>().map(|ip| ip.to_string()).unwrap_or(host);
    let port = url.port().unwrap_or(21117);
    (port > 0 && !host.is_empty()).then_some((host, port))
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Location {
    latitude: f64,
    longitude: f64,
}
impl Location {
    fn valid(self) -> bool {
        self.latitude.is_finite()
            && self.longitude.is_finite()
            && (-90.0..=90.0).contains(&self.latitude)
            && (-180.0..=180.0).contains(&self.longitude)
    }
    fn distance(self, other: Self) -> f64 {
        let (a, b) = (self.latitude.to_radians(), other.latitude.to_radians());
        let h = ((b - a) / 2.0).sin().powi(2)
            + a.cos()
                * b.cos()
                * ((other.longitude - self.longitude).to_radians() / 2.0)
                    .sin()
                    .powi(2);
        12742.0 * h.clamp(0.0, 1.0).sqrt().asin()
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Override {
    cidr: IpNetwork,
    location: Location,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Node {
    id: String,
    address: String,
    metrics_url: String,
    token_env: String,
    location: Location,
    max_sessions: usize,
    bandwidth_mbps: f64,
    #[serde(default)]
    drain: bool,
}
#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Config {
    poll_seconds: u64,
    timeout_ms: u64,
    stale_seconds: u64,
    recovery_successes: u32,
    candidate_slack_km: f64,
    geoip_database: Option<String>,
    location_overrides: Vec<Override>,
    nodes: Vec<Node>,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            poll_seconds: 3,
            timeout_ms: 1500,
            stale_seconds: 12,
            recovery_successes: 2,
            candidate_slack_km: 2500.0,
            geoip_database: None,
            location_overrides: vec![],
            nodes: vec![],
        }
    }
}
impl Config {
    fn validate(&self) -> ResultType<()> {
        if !(1..=60).contains(&self.poll_seconds)
            || !(100..=10000).contains(&self.timeout_ms)
            || self.stale_seconds < self.poll_seconds + (self.timeout_ms + 999) / 1000
            || self.stale_seconds > 300
            || !(2..=10).contains(&self.recovery_successes)
            || !self.candidate_slack_km.is_finite()
            || !(0.0..=40000.0).contains(&self.candidate_slack_km)
            || self.nodes.is_empty()
            || self.nodes.len() > 128
        {
            bail!("Invalid scheduler timing, candidate distance or node count");
        }
        let mut ids = HashSet::new();
        let mut addresses = HashSet::new();
        for n in &self.nodes {
            if n.id.is_empty()
                || !ids.insert(&n.id)
                || !addresses.insert(&n.address)
                || !n.location.valid()
                || n.max_sessions == 0
                || !n.bandwidth_mbps.is_finite()
                || n.bandwidth_mbps <= 0.0
            {
                bail!("Invalid or duplicate relay node: {}", n.id);
            }
            // An explicit port also makes comparisons with the client-selected address unambiguous.
            let addr = reqwest::Url::parse(&format!("tcp://{}", n.address))?;
            if addr.host_str().is_none()
                || addr.port().filter(|p| *p > 0).is_none()
                || addr.path() != ""
                || !addr.username().is_empty()
                || addr.password().is_some()
                || addr.query().is_some()
                || addr.fragment().is_some()
            {
                bail!("Relay {} needs host:port (IPv6: [address]:port)", n.id);
            }
            let url = reqwest::Url::parse(&n.metrics_url)?;
            if !matches!(url.scheme(), "http" | "https")
                || !url.username().is_empty()
                || url.password().is_some()
                || url.query().is_some()
                || url.fragment().is_some()
            {
                bail!("Invalid metrics URL for {}", n.id);
            }
            if url.scheme() == "http" {
                let ip = url
                    .host_str()
                    .unwrap_or_default()
                    .trim_matches(['[', ']'])
                    .parse::<IpAddr>();
                let private = match ip {
                    Ok(IpAddr::V4(ip)) => ip.is_private() || ip.is_loopback(),
                    Ok(IpAddr::V6(ip)) => ip.is_loopback() || (ip.segments()[0] & 0xfe00 == 0xfc00),
                    _ => false,
                };
                if !private {
                    bail!(
                        "Metrics for {} require HTTPS or a private/loopback IP",
                        n.id
                    );
                }
            }
        }
        if self.location_overrides.iter().any(|o| !o.location.valid()) {
            bail!("Invalid CIDR location override");
        }
        Ok(())
    }
}

#[derive(Default)]
struct Health {
    healthy: bool,
    successes: u32,
    last_success: Option<Instant>,
    sample: Option<Snapshot>,
    mbps: f64,
    // Short-lived reservations reduce bursts before the next telemetry sample.
    reservations: Vec<Instant>,
}
impl Health {
    fn update(&mut self, sample: Option<Snapshot>, now: Instant, required: u32) {
        let was_healthy = self.healthy;
        if let Some(sample) = sample {
            let same_boot = self
                .sample
                .as_ref()
                .map(|old| old.boot_id == sample.boot_id)
                .unwrap_or(false);
            if self
                .sample
                .as_ref()
                .map(|old| {
                    same_boot
                        && (sample.uptime_ms <= old.uptime_ms
                            || sample.forwarded_bytes < old.forwarded_bytes)
                })
                .unwrap_or(false)
            {
                self.healthy = false;
                self.successes = 0;
                return;
            }
            if !same_boot {
                self.successes = 0;
                self.healthy = false;
                self.reservations.clear();
            }
            self.mbps = match &self.sample {
                Some(old)
                    if same_boot
                        && sample.uptime_ms > old.uptime_ms
                        && sample.forwarded_bytes >= old.forwarded_bytes =>
                {
                    (sample.forwarded_bytes - old.forwarded_bytes) as f64 * 0.008
                        / (sample.uptime_ms - old.uptime_ms) as f64
                }
                _ => 0.0,
            };
            self.successes = self.successes.saturating_add(1);
            self.healthy = self.successes >= required;
            self.last_success = Some(now);
            self.sample = Some(sample);
        } else {
            // Fail closed on the first failed poll; recovery needs consecutive successes.
            self.healthy = false;
            self.successes = 0;
        }
        if was_healthy != self.healthy {
            log::debug!("Relay health transition: {}", self.healthy);
        }
    }
    fn available(&self, now: Instant, stale: Duration) -> bool {
        self.healthy
            && self
                .last_success
                .map(|t| now.saturating_duration_since(t) < stale)
                .unwrap_or(false)
    }
}

pub struct Scheduler {
    config: Config,
    tokens: Vec<String>,
    geoip: Option<maxminddb::Reader<Vec<u8>>>,
    client: reqwest::Client,
    health: RwLock<Vec<Health>>,
}

impl Scheduler {
    pub fn from_env() -> ResultType<Option<Arc<Self>>> {
        let path = match setting("RELAY_SCHEDULER_CONFIG") {
            Ok(path) => path,
            Err(std::env::VarError::NotPresent) => return Ok(None),
            Err(err) => return Err(err.into()),
        };
        let config: Config = serde_json::from_slice(&std::fs::read(&path)?)?;
        config.validate()?;
        let mut tokens = Vec::new();
        for node in &config.nodes {
            let token = setting(&node.token_env)?;
            if token.len() < 32 || !token.bytes().all(|b| b.is_ascii_graphic()) {
                bail!(
                    "Invalid metrics token for {} (environment variable {})",
                    node.id,
                    node.token_env
                );
            }
            tokens.push(token);
        }
        let geoip = match &config.geoip_database {
            Some(file) => {
                let file = Path::new(&path)
                    .parent()
                    .unwrap_or_else(|| Path::new("."))
                    .join(file);
                Some(maxminddb::Reader::open_readfile(file)?)
            }
            None => None,
        };
        if geoip.is_none() && config.location_overrides.is_empty() {
            log::warn!(
                "Relay scheduler has no GeoIP database or overrides; selecting by load only"
            );
        }
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_millis(config.timeout_ms))
            .build()?;
        let health = RwLock::new((0..config.nodes.len()).map(|_| Health::default()).collect());
        Ok(Some(Arc::new(Self {
            config,
            tokens,
            geoip,
            client,
            health,
        })))
    }

    pub fn start(self: &Arc<Self>) {
        let scheduler = self.clone();
        tokio::spawn(async move {
            loop {
                scheduler.poll().await;
                sleep(Duration::from_secs(scheduler.config.poll_seconds)).await;
            }
        });
    }

    async fn probe(&self, index: usize) -> ResultType<Snapshot> {
        let node = &self.config.nodes[index];
        let probe = async {
            // Metrics reachability alone does not prove the public relay port is listening.
            let _connection = TcpStream::connect(&node.address).await?;
            let mut response = self
                .client
                .get(&node.metrics_url)
                .bearer_auth(&self.tokens[index])
                .send()
                .await?
                .error_for_status()?;
            let mut bytes = Vec::new();
            while let Some(chunk) = response.chunk().await? {
                if bytes.len() + chunk.len() > 4096 {
                    bail!("Metrics response too large");
                }
                bytes.extend_from_slice(&chunk);
            }
            let sample: Snapshot = serde_json::from_slice(&bytes)?;
            if sample.schema != 1 || sample.node_id != node.id || sample.boot_id.is_empty() {
                bail!("Unexpected metrics identity or schema");
            }
            Ok(sample)
        };
        hbb_common::timeout(self.config.timeout_ms, probe).await?
    }

    async fn poll(&self) {
        let samples =
            join_all((0..self.config.nodes.len()).map(|i| async move { self.probe(i).await.ok() }))
                .await;
        let now = Instant::now();
        let mut health = self.health.write().unwrap_or_else(|p| p.into_inner());
        for (index, sample) in samples.into_iter().enumerate() {
            let old = health[index].healthy;
            health[index].update(sample, now, self.config.recovery_successes);
            if old != health[index].healthy {
                log::info!(
                    "Relay {} {}",
                    self.config.nodes[index].id,
                    if health[index].healthy {
                        "healthy"
                    } else {
                        "unavailable"
                    }
                );
            }
        }
    }

    fn locate(&self, ip: IpAddr) -> Option<Location> {
        let ip = match ip {
            IpAddr::V6(ip) => ip
                .to_ipv4_mapped()
                .map(IpAddr::V4)
                .unwrap_or(IpAddr::V6(ip)),
            other => other,
        };
        if let Some(rule) = self
            .config
            .location_overrides
            .iter()
            .filter(|o| o.cidr.contains(ip))
            .max_by_key(|o| o.cidr.prefix())
        {
            return Some(rule.location);
        }
        let city: maxminddb::geoip2::City = self.geoip.as_ref()?.lookup(ip).ok()?;
        let loc = city.location?;
        let location = Location {
            latitude: loc.latitude?,
            longitude: loc.longitude?,
        };
        location.valid().then_some(location)
    }

    pub fn select(&self, a: IpAddr, b: IpAddr) -> Option<String> {
        self.choose(a, b, true)
    }
    pub fn preview(&self, a: IpAddr, b: IpAddr) -> Option<String> {
        self.choose(a, b, false)
    }

    fn choose(&self, a: IpAddr, b: IpAddr, reserve: bool) -> Option<String> {
        let (a, b) = (self.locate(a), self.locate(b));
        let now = Instant::now();
        let stale = Duration::from_secs(self.config.stale_seconds);
        let reservation_ttl = Duration::from_secs(self.config.poll_seconds * 2 + 2);
        let mut states = self.health.write().unwrap_or_else(|p| p.into_inner());
        let mut candidates = Vec::new();
        for (i, node) in self.config.nodes.iter().enumerate() {
            let state = &mut states[i];
            state
                .reservations
                .retain(|t| now.saturating_duration_since(*t) < reservation_ttl);
            if node.drain || !state.available(now, stale) {
                continue;
            }
            let sessions = state
                .sample
                .as_ref()
                .map(|s| s.active_sessions)
                .unwrap_or(0)
                .saturating_add(state.reservations.len());
            let load =
                (sessions as f64 / node.max_sessions as f64).max(state.mbps / node.bandwidth_mbps);
            if load >= 1.0 {
                continue;
            }
            let da = a.map(|p| p.distance(node.location));
            let db = b.map(|p| p.distance(node.location));
            // A small worst-leg penalty breaks equal-total-distance ties across continents.
            let distance = da.unwrap_or(0.0)
                + db.unwrap_or(0.0)
                + 0.25 * da.unwrap_or(0.0).max(db.unwrap_or(0.0));
            candidates.push((i, distance, load));
        }
        let closest = candidates.iter().map(|(_, d, _)| *d).reduce(f64::min)?;
        let best = candidates
            .into_iter()
            .filter(|(_, d, _)| *d <= closest + self.config.candidate_slack_km)
            .min_by(|a, b| {
                let score = |c: &(usize, f64, f64)| {
                    c.2 + 0.20 * (c.1 - closest) / self.config.candidate_slack_km.max(1.0)
                };
                score(a).total_cmp(&score(b)).then(a.0.cmp(&b.0))
            })?
            .0;
        if reserve {
            states[best].reservations.push(now);
        }
        Some(self.config.nodes[best].address.clone())
    }

    // Do not reselect here: the official caller retains the address from its original request.
    pub fn permits_negotiation(&self, address: &str) -> bool {
        let Some(endpoint) = relay_endpoint(address) else { return false; };
        let states = self.health.read().unwrap_or_else(|p| p.into_inner());
        self.config.nodes.iter().enumerate().any(|(i, node)| {
            relay_endpoint(&node.address).as_ref() == Some(&endpoint)
                && states[i].available(
                    Instant::now(),
                    Duration::from_secs(self.config.stale_seconds),
                )
        })
    }

    pub fn status(&self) -> String {
        #[derive(Serialize)]
        struct Entry<'a> {
            id: &'a str,
            address: &'a str,
            healthy: bool,
            drain: bool,
            sessions: usize,
            mbps: f64,
        }
        let states = self.health.read().unwrap_or_else(|p| p.into_inner());
        let entries: Vec<_> = self
            .config
            .nodes
            .iter()
            .enumerate()
            .map(|(i, n)| Entry {
                id: &n.id,
                address: &n.address,
                healthy: states[i].available(
                    Instant::now(),
                    Duration::from_secs(self.config.stale_seconds),
                ),
                drain: n.drain,
                sessions: states[i]
                    .sample
                    .as_ref()
                    .map(|s| s.active_sessions)
                    .unwrap_or(0),
                mbps: states[i].mbps,
            })
            .collect();
        serde_json::to_string_pretty(&entries).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn location(longitude: f64) -> Location {
        Location {
            latitude: 0.0,
            longitude,
        }
    }
    fn sample(id: &str, time: u64, bytes: u64, sessions: usize) -> Snapshot {
        Snapshot {
            schema: 1,
            node_id: id.into(),
            boot_id: "boot-one".into(),
            uptime_ms: time,
            active_sessions: sessions,
            forwarded_bytes: bytes,
        }
    }
    fn scheduler() -> Scheduler {
        let nodes = [0.0, 30.0, 90.0]
            .into_iter()
            .enumerate()
            .map(|(i, longitude)| Node {
                id: format!("node{i}"),
                address: format!("relay{i}.example:21117"),
                metrics_url: format!("https://relay{i}.example/metrics"),
                token_env: format!("TOKEN{i}"),
                location: location(longitude),
                max_sessions: 100,
                bandwidth_mbps: 100.0,
                drain: false,
            })
            .collect();
        let config = Config {
            nodes,
            candidate_slack_km: 1000.0,
            location_overrides: vec![
                Override {
                    cidr: "192.0.2.0/24".parse().unwrap(),
                    location: location(0.0),
                },
                Override {
                    cidr: "198.51.100.0/24".parse().unwrap(),
                    location: location(90.0),
                },
                Override {
                    cidr: "2001:db8::/32".parse().unwrap(),
                    location: location(30.0),
                },
                Override {
                    cidr: "2001:db8:1::/48".parse().unwrap(),
                    location: location(90.0),
                },
            ],
            ..Config::default()
        };
        config.validate().unwrap();
        let states = config
            .nodes
            .iter()
            .map(|n| {
                let mut s = Health::default();
                s.update(Some(sample(&n.id, 1000, 0, 0)), Instant::now(), 2);
                s.update(Some(sample(&n.id, 2000, 0, 0)), Instant::now(), 2);
                s
            })
            .collect();
        Scheduler {
            config,
            tokens: vec![],
            geoip: None,
            client: reqwest::Client::new(),
            health: RwLock::new(states),
        }
    }

    #[test]
    fn both_endpoints_change_candidates_and_unknown_locations_use_load() {
        let s = scheduler();
        let a = "192.0.2.1".parse().unwrap();
        let b = "198.51.100.1".parse().unwrap();
        assert_eq!(s.preview(a, a).unwrap(), "relay0.example:21117");
        assert_eq!(s.preview(b, b).unwrap(), "relay2.example:21117");
        assert_eq!(s.preview(a, b).unwrap(), "relay1.example:21117");
        assert_eq!(s.preview(b, a), s.preview(a, b));
        assert_eq!(
            s.locate("::ffff:192.0.2.1".parse().unwrap())
                .unwrap()
                .longitude,
            0.0
        );
        assert_eq!(
            s.locate("2001:db8:1::1".parse().unwrap())
                .unwrap()
                .longitude,
            90.0
        );
        assert_eq!(
            s.locate("2001:db8:2::1".parse().unwrap())
                .unwrap()
                .longitude,
            30.0
        );
        s.health.write().unwrap()[0]
            .sample
            .as_mut()
            .unwrap()
            .active_sessions = 90;
        assert_eq!(
            s.preview("10.0.0.1".parse().unwrap(), "10.0.0.2".parse().unwrap())
                .unwrap(),
            "relay1.example:21117"
        );
    }

    #[test]
    fn capacity_bandwidth_drain_staleness_and_all_down_are_excluded() {
        let mut s = scheduler();
        let a = "192.0.2.1".parse().unwrap();
        s.health.write().unwrap()[0]
            .sample
            .as_mut()
            .unwrap()
            .active_sessions = 100;
        assert_eq!(s.preview(a, a).unwrap(), "relay1.example:21117");
        s.health.write().unwrap()[1].mbps = 100.0;
        assert_eq!(s.preview(a, a).unwrap(), "relay2.example:21117");
        s.config.nodes[2].drain = true;
        assert!(s.preview(a, a).is_none());
        // Existing negotiations are permitted after drain, while the relay stays healthy.
        assert!(s.permits_negotiation("relay2.example:21117"));
        assert!(!s.permits_negotiation("unconfigured.example:21117"));
        assert!(s.permits_negotiation("RELAY2.EXAMPLE."));
        assert!(!s.permits_negotiation("relay2.example:21118"));
        assert!(!s.permits_negotiation("user@relay2.example:21117"));
        assert_eq!(relay_endpoint("2001:db8::1"), relay_endpoint("[2001:db8::1]:21117"));
        s.health.write().unwrap()[2].last_success = Some(Instant::now() - Duration::from_secs(13));
        assert!(!s.permits_negotiation("relay2.example:21117"));
        for state in s.health.write().unwrap().iter_mut() {
            state.update(None, Instant::now(), 2);
        }
        assert!(s.preview(a, a).is_none());
    }

    #[test]
    fn counters_recovery_restarts_and_cached_metrics() {
        let mut h = Health::default();
        let now = Instant::now();
        h.update(Some(sample("node", 1000, 0, 2)), now, 2);
        assert!(!h.healthy);
        h.update(Some(sample("node", 2000, 1_000_000, 2)), now, 2);
        assert!(h.healthy);
        assert_eq!(h.mbps, 8.0);
        h.update(None, now, 2);
        assert!(!h.healthy);
        h.update(Some(sample("node", 3000, 1_000_000, 2)), now, 2);
        assert!(!h.healthy);
        h.update(Some(sample("node", 4000, 1_000_000, 2)), now, 2);
        assert!(h.healthy);
        h.update(Some(sample("node", 4000, 1_000_000, 2)), now, 2);
        assert!(!h.healthy, "replayed/cached telemetry must not look fresh");
        let mut reboot = sample("node", 20, 0, 0);
        reboot.boot_id = "boot-two".into();
        h.update(Some(reboot), now, 2);
        assert!(!h.healthy);
        assert_eq!(h.mbps, 0.0);
    }

    #[test]
    fn reservations_spread_bursts_but_preview_does_not_consume_capacity() {
        let mut s = scheduler();
        for n in &mut s.config.nodes {
            n.max_sessions = 1;
            n.location = location(0.0);
        }
        let a = "10.0.0.1".parse().unwrap();
        for _ in 0..5 {
            assert_eq!(s.preview(a, a).unwrap(), "relay0.example:21117");
        }
        for i in 0..3 {
            assert_eq!(s.select(a, a).unwrap(), format!("relay{i}.example:21117"));
        }
        assert!(s.select(a, a).is_none());
        for state in s.health.write().unwrap().iter_mut() {
            for t in &mut state.reservations {
                *t = Instant::now() - Duration::from_secs(9);
            }
        }
        assert!(s.select(a, a).is_some());
    }

    #[test]
    fn rejects_invalid_settings_and_supports_ipv6_addresses() {
        let mut s = scheduler();
        s.config.nodes[0].address = "[2001:db8::1]:21117".into();
        s.config.nodes[0].metrics_url = "http://[fd00::1]:21120/metrics".into();
        assert!(s.config.validate().is_ok());
        s.config.nodes[0].metrics_url = "http://203.0.113.1:21120/metrics".into();
        assert!(s.config.validate().is_err());
        s.config.nodes[0].metrics_url = "https://relay.example/metrics".into();
        s.config.nodes[0].address = s.config.nodes[1].address.clone();
        assert!(s.config.validate().is_err());
        s.config.nodes[0].address = "relay.example".into();
        assert!(s.config.validate().is_err());
        assert!(serde_json::from_str::<Config>(r#"{"nodez": []}"#).is_err());
    }
}
