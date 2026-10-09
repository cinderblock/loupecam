//! mDNS / DNS-SD: announce the server on the local network, and find other servers.
//!
//! A server listening on a non-loopback address registers itself as
//! `_loupecam._tcp` (for LoupeCam-aware clients, `loupecam discover`) and, when it
//! serves the web UI, as `_http._tcp` too, so generic browsers (Bonjour/Avahi
//! browsers, `dns-sd -B _http._tcp`, Home Assistant) list it with no extra software.
//!
//! TXT records (never secrets):
//!
//! | Key | |
//! | --- | --- |
//! | `txtvers` | `1` |
//! | `version` | LoupeCam version |
//! | `path` | `/` (where the web UI is, per the `_http._tcp` convention) |
//! | `ui` | `1` if the web UI is served, else `0` |
//! | `auth` | `token` if a token is required, else `none` |
//! | `model`, `serial` | the connected camera, when there is one |

use crate::service::{DeviceSummary, Shared};
use mdns_sd::{DaemonEvent, IfKind, ServiceDaemon, ServiceEvent, ServiceInfo};
use std::collections::BTreeMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The LoupeCam DNS-SD service type.
pub const SERVICE_TYPE: &str = "_loupecam._tcp.local.";
const HTTP_SERVICE_TYPE: &str = "_http._tcp.local.";

/// What to announce.
#[derive(Debug, Clone, Default)]
pub struct Announce {
    /// Instance name shown in browsers. Default: "LoupeCam on <hostname>".
    pub name: Option<String>,
}

/// A running announcement. [`stop`](Self::stop) sends goodbyes so browsers drop the
/// entry immediately instead of waiting for it to expire.
pub struct Announcer {
    daemon: ServiceDaemon,
    fullnames: Vec<String>,
    task: tokio::task::JoinHandle<()>,
}

struct Template {
    name: String,
    host: String,
    addr: SocketAddr,
    interfaces: Vec<IfKind>,
    web_ui: bool,
    token: bool,
}

impl Template {
    fn infos(&self, device: Option<&DeviceSummary>) -> mdns_sd::Result<Vec<ServiceInfo>> {
        let mut txt = BTreeMap::from([
            ("txtvers", "1".to_string()),
            ("version", env!("CARGO_PKG_VERSION").to_string()),
            ("path", "/".to_string()),
            ("ui", if self.web_ui { "1" } else { "0" }.to_string()),
            ("auth", if self.token { "token" } else { "none" }.to_string()),
        ]);
        if let Some(d) = device {
            txt.insert("model", d.model.clone());
            txt.insert("serial", d.serial.clone());
        }
        let txt: Vec<_> = txt.into_iter().collect();
        let types: &[&str] = if self.web_ui { &[SERVICE_TYPE, HTTP_SERVICE_TYPE] } else { &[SERVICE_TYPE] };
        types
            .iter()
            .map(|ty| {
                let ip = self.addr.ip();
                let mut info = if ip.is_unspecified() {
                    ServiceInfo::new(ty, &self.name, &self.host, (), self.addr.port(), &txt[..])?.enable_addr_auto()
                } else {
                    ServiceInfo::new(ty, &self.name, &self.host, ip, self.addr.port(), &txt[..])?
                };
                info.set_interfaces(self.interfaces.clone());
                // No conflict probing: on networks that reflect mDNS between VLANs (common
                // with UniFi and similar), a multi-homed host hears its own records back
                // from its other interfaces, with those interfaces' addresses, and the
                // probe takes that for a conflict, announcing a second "... (2)" instance.
                // The names here are already specific to this host.
                info.set_requires_probe(false);
                Ok(info)
            })
            .collect()
    }
}

/// Interfaces a listener on `ip` can actually be reached through.
fn interfaces(ip: IpAddr) -> Vec<IfKind> {
    match ip {
        IpAddr::V4(v4) if v4.is_unspecified() => vec![IfKind::IPv4],
        // Windows sockets bound to `::` are IPv6-only; elsewhere they are dual-stack.
        IpAddr::V6(v6) if v6.is_unspecified() => vec![if cfg!(windows) { IfKind::IPv6 } else { IfKind::All }],
        ip => vec![IfKind::Addr(ip)],
    }
}

/// `host` as a DNS label: lowercase letters, digits and hyphens.
fn dns_label(host: &str) -> String {
    let label: String = host
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' })
        .take(63)
        .collect();
    let label = label.trim_matches('-');
    if label.is_empty() { "loupecam".into() } else { label.into() }
}

impl Announcer {
    /// Announce a server bound to `addr`. Returns `None` (after logging why) when
    /// there is nothing to announce or mDNS isn't available.
    pub fn start(cfg: &Announce, addr: SocketAddr, web_ui: bool, token: bool, shared: Arc<Shared>) -> Option<Announcer> {
        if addr.ip().is_loopback() {
            tracing::debug!("not announcing over mDNS: listening on loopback only");
            return None;
        }
        let hostname = gethostname::gethostname().to_string_lossy().into_owned();
        let template = Template {
            name: cfg.name.clone().unwrap_or_else(|| format!("LoupeCam on {hostname}")),
            // Not `<hostname>.local.`: the OS's own responder (Windows, mDNSResponder,
            // Avahi) owns that name and may announce a different set of addresses.
            host: format!("loupecam-{}.local.", dns_label(&hostname)),
            addr,
            interfaces: interfaces(addr.ip()),
            web_ui,
            token,
        };
        let daemon = match ServiceDaemon::new() {
            Ok(d) => d,
            Err(e) => {
                tracing::warn!("mDNS unavailable, not announcing: {e}");
                return None;
            }
        };
        let mut state = shared.state.subscribe();
        let device = state.borrow_and_update().device.clone();
        let infos = match template.infos(device.as_ref()) {
            Ok(i) => i,
            Err(e) => {
                tracing::warn!("mDNS: {e}");
                let _ = daemon.shutdown();
                return None;
            }
        };
        let fullnames = infos.iter().map(|i| i.get_fullname().to_string()).collect();
        for info in infos {
            if let Err(e) = daemon.register(info) {
                tracing::warn!("mDNS register: {e}");
            }
        }
        tracing::info!("announcing \"{}\" over mDNS ({SERVICE_TYPE})", template.name);
        if let Ok(events) = daemon.monitor() {
            std::thread::Builder::new()
                .name("loupecam-mdns-events".into())
                .spawn(move || {
                    // Ends when the daemon shuts down.
                    while let Ok(ev) = events.recv() {
                        match ev {
                            DaemonEvent::NameChange(c) => {
                                tracing::warn!("mDNS name conflict: {} is now {} on {} ({:?})", c.original, c.new_name, c.intf_name, c.rr_type)
                            }
                            DaemonEvent::Error(e) => tracing::warn!("mDNS: {e}"),
                            ev => tracing::debug!("mDNS: {ev:?}"),
                        }
                    }
                })
                .ok();
        }

        // Keep the camera's model and serial in the TXT records current.
        let d = daemon.clone();
        let task = tokio::spawn(async move {
            let mut last = device;
            while state.changed().await.is_ok() {
                let device = state.borrow_and_update().device.clone();
                let key = |d: &Option<DeviceSummary>| d.as_ref().map(|d| (d.model.clone(), d.serial.clone()));
                if key(&device) == key(&last) {
                    continue;
                }
                last = device;
                match template.infos(last.as_ref()) {
                    // Re-registering the same name replaces and re-announces its records.
                    Ok(infos) => {
                        for info in infos {
                            if let Err(e) = d.register(info) {
                                tracing::warn!("mDNS register: {e}");
                            }
                        }
                    }
                    Err(e) => tracing::warn!("mDNS: {e}"),
                }
            }
        });
        Some(Announcer { daemon, fullnames, task })
    }

    /// Withdraw the announcement (best effort, bounded wait).
    pub async fn stop(self) {
        self.task.abort();
        let Announcer { daemon, fullnames, .. } = self;
        let _ = tokio::task::spawn_blocking(move || {
            for name in &fullnames {
                if let Ok(rx) = daemon.unregister(name) {
                    let _ = rx.recv_timeout(Duration::from_secs(1));
                }
            }
            if let Ok(rx) = daemon.shutdown() {
                let _ = rx.recv_timeout(Duration::from_secs(1));
            }
        })
        .await;
    }
}

/// A LoupeCam server found on the network.
#[derive(Debug, Clone)]
pub struct Found {
    pub name: String,
    pub host: String,
    pub port: u16,
    pub addrs: Vec<IpAddr>,
    pub txt: BTreeMap<String, String>,
}

impl Found {
    /// Base URLs to reach it, by mDNS host name first, then by address.
    pub fn urls(&self) -> Vec<String> {
        let host = self.host.trim_end_matches('.');
        std::iter::once(format!("http://{host}:{}/", self.port))
            .chain(self.addrs.iter().map(|a| format!("http://{}/", SocketAddr::new(*a, self.port))))
            .collect()
    }
}

/// Browse for LoupeCam servers for `timeout`. Blocking.
pub fn discover(timeout: Duration) -> mdns_sd::Result<Vec<Found>> {
    let daemon = ServiceDaemon::new()?;
    let rx = daemon.browse(SERVICE_TYPE)?;
    let deadline = Instant::now() + timeout;
    let mut found = BTreeMap::<String, Found>::new();
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        match rx.recv_timeout(left) {
            Ok(ServiceEvent::ServiceResolved(s)) => {
                let fullname = s.get_fullname().to_string();
                let name = fullname.strip_suffix(&format!(".{SERVICE_TYPE}")).unwrap_or(&fullname).replace("\\", "");
                let mut addrs: Vec<IpAddr> = s.get_addresses().iter().map(|a| a.to_ip_addr()).collect();
                addrs.sort_by_key(|a| (a.is_ipv6(), *a));
                addrs.dedup();
                found.insert(
                    fullname,
                    Found {
                        name,
                        host: s.get_hostname().to_string(),
                        port: s.get_port(),
                        addrs,
                        txt: s.get_properties().iter().map(|p| (p.key().to_string(), p.val_str().to_string())).collect(),
                    },
                );
            }
            Ok(ServiceEvent::ServiceRemoved(_, fullname)) => {
                found.remove(&fullname);
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }
    let _ = daemon.shutdown();
    Ok(found.into_values().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_labels() {
        assert_eq!(dns_label("DESKTOP-AB12"), "desktop-ab12");
        assert_eq!(dns_label("my_pi.lan"), "my-pi-lan");
        assert_eq!(dns_label("--"), "loupecam");
    }

    fn device() -> DeviceSummary {
        DeviceSummary {
            model: "MU1803-HS".into(),
            serial: "TEST123".into(),
            firmware_version: String::new(),
            hardware_version: String::new(),
            fpga_version: String::new(),
            production_date: None,
            resolutions: vec![],
            pixel_size_um: 1.25,
            max_bit_depth: 12,
            max_gain: 16.0,
        }
    }

    /// Next resolved/removed event for `fullname`.
    fn next_event(rx: &mdns_sd::Receiver<ServiceEvent>, fullname: &str) -> ServiceEvent {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let left = deadline.checked_duration_since(Instant::now()).expect("timed out waiting for mDNS");
            match rx.recv_timeout(left).expect("timed out waiting for mDNS") {
                ServiceEvent::ServiceResolved(s) if s.get_fullname() == fullname => return ServiceEvent::ServiceResolved(s),
                ServiceEvent::ServiceRemoved(ty, name) if name == fullname => return ServiceEvent::ServiceRemoved(ty, name),
                _ => {}
            }
        }
    }

    /// Announces on the real network, so it needs working multicast: `cargo test -- --ignored`.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "needs a network with multicast"]
    async fn announce_update_withdraw() {
        use crate::service::{LiveStats, State, Status};
        use tokio::sync::watch;

        let name = format!("LoupeCam test {}", std::process::id());
        let shared = Arc::new(Shared {
            state: watch::Sender::new(State { status: Status::Searching(None), device: None, settings: Default::default() }),
            stats: watch::Sender::new(LiveStats::default()),
            frame: watch::Sender::new(None),
        });
        let addr: SocketAddr = "0.0.0.0:48123".parse().unwrap();
        let announcer = Announcer::start(&Announce { name: Some(name.clone()) }, addr, true, true, shared.clone()).unwrap();

        let browser = ServiceDaemon::new().unwrap();
        let rx = browser.browse(SERVICE_TYPE).unwrap();
        // A second type browsed on the same daemon doesn't always resolve; use another.
        let http_browser = ServiceDaemon::new().unwrap();
        let http = http_browser.browse(HTTP_SERVICE_TYPE).unwrap();
        let fullname = format!("{name}.{SERVICE_TYPE}");

        let ServiceEvent::ServiceResolved(s) = next_event(&rx, &fullname) else { panic!("removed before resolved") };
        assert_eq!(s.get_port(), 48123);
        assert_eq!(s.get_property_val_str("auth"), Some("token"));
        assert_eq!(s.get_property_val_str("ui"), Some("1"));
        assert_eq!(s.get_property_val_str("model"), None);
        assert!(matches!(next_event(&http, &format!("{name}.{HTTP_SERVICE_TYPE}")), ServiceEvent::ServiceResolved(_)));

        // A camera attaches: its model and serial appear.
        shared.state.send_modify(|s| s.device = Some(device()));
        loop {
            let ServiceEvent::ServiceResolved(s) = next_event(&rx, &fullname) else { panic!("removed on update") };
            if s.get_property_val_str("serial") == Some("TEST123") {
                assert_eq!(s.get_property_val_str("model"), Some("MU1803-HS"));
                break;
            }
        }

        // Stopping says goodbye.
        announcer.stop().await;
        assert!(matches!(next_event(&rx, &fullname), ServiceEvent::ServiceRemoved(..)));
        let _ = browser.shutdown();
        let _ = http_browser.shutdown();
    }
}
