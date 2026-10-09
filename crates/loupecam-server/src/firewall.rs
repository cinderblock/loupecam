//! Windows Firewall: notice when it will stop other machines from reaching the server.
//!
//! The first time a program listens on the network, Windows asks whether to allow it.
//! Dismissing that dialog silently adds a *block* rule for the executable, and Windows
//! never asks again: the server runs, but nothing else on the LAN can connect. This
//! reads the firewall policy and says so, with the commands that fix it.
//!
//! Rules are matched by executable path, so allowing a program once covers later
//! updates installed in place.

// Only `check` is used off Windows (where it finds nothing); the rest is tested everywhere.
#![cfg_attr(not(windows), allow(dead_code))]

/// Warnings for a server on `port` (and mDNS on UDP 5353 when `mdns`). Empty when
/// nothing is blocked, or the policy can't be read.
pub fn check(port: u16, mdns: bool) -> Vec<String> {
    #[cfg(windows)]
    {
        let exe = match std::env::current_exe() {
            Ok(p) => p.to_string_lossy().trim_start_matches(r"\\?\").to_string(),
            Err(_) => return Vec::new(),
        };
        match imp::read(&exe) {
            Ok((profiles, rules)) => warnings(&exe, port, mdns, &profiles, &rules),
            Err(e) => {
                tracing::debug!("reading the Windows Firewall policy: {e}");
                Vec::new()
            }
        }
    }
    #[cfg(not(windows))]
    {
        let _ = (port, mdns);
        Vec::new()
    }
}

const TCP: i32 = 6;
const UDP: i32 = 17;
const ANY: i32 = 256;

/// An active network profile (Domain, Private or Public) with the firewall on.
#[derive(Debug, Clone)]
struct Profile {
    name: &'static str,
    bit: i32,
    block_all: bool,
    /// Windows won't ask about programs without a rule.
    silent: bool,
    default_allow: bool,
}

/// An enabled inbound rule for this executable.
#[derive(Debug, Clone)]
struct Rule {
    name: String,
    allow: bool,
    protocol: i32,
    ports: String,
    profiles: i32,
}

fn warnings(exe: &str, port: u16, mdns: bool, profiles: &[Profile], rules: &[Rule]) -> Vec<String> {
    let mut wants = vec![(TCP, port, "TCP", "the server")];
    if mdns {
        wants.push((UDP, 5353, "UDP", "mDNS discovery"));
    }
    let mut out = Vec::new();
    for p in profiles {
        for &(proto, port, proto_name, what) in &wants {
            let applies: Vec<&Rule> = rules
                .iter()
                .filter(|r| {
                    r.profiles & p.bit != 0
                        && (r.protocol == ANY || r.protocol == proto)
                        && (r.ports.is_empty() || port_matches(&r.ports, port))
                })
                .collect();
            let blockers: Vec<&str> = applies.iter().filter(|r| !r.allow).map(|r| r.name.as_str()).collect();
            // Block rules win over allow rules.
            let why = if p.block_all {
                "the profile blocks all incoming connections".to_string()
            } else if !blockers.is_empty() {
                format!("blocked by rule \"{}\"", blockers.join("\", \""))
            } else if applies.is_empty() && p.silent && !p.default_allow {
                "no rule allows it, and Windows is set not to ask".to_string()
            } else {
                continue;
            };
            out.push(format!(
                "Windows Firewall ({} network) stops other machines reaching {what} ({proto_name} {port}): {why}. \
                 To allow it, run in an elevated PowerShell:\n  \
                 Get-NetFirewallApplicationFilter -Program '{exe}' | Get-NetFirewallRule | Where-Object Action -eq Block | Remove-NetFirewallRule\n  \
                 New-NetFirewallRule -DisplayName LoupeCam -Direction Inbound -Program '{exe}' -Action Allow -Profile {}",
                p.name, p.name
            ));
        }
    }
    out
}

/// Does a Windows Firewall port list (`*`, `80,443`, `5000-5010`) include `port`?
fn port_matches(list: &str, port: u16) -> bool {
    list.split(',').map(str::trim).any(|item| match item.split_once('-') {
        _ if item == "*" => true,
        Some((a, b)) => matches!((a.parse::<u16>(), b.parse::<u16>()), (Ok(a), Ok(b)) if (a..=b).contains(&port)),
        None => item.parse() == Ok(port),
    })
}

/// Expand `%VAR%` references (rules may store paths like `%ProgramFiles%\…`).
fn expand_env(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(start) = rest.find('%') {
        let Some(len) = rest[start + 1..].find('%') else { break };
        let name = &rest[start + 1..start + 1 + len];
        out.push_str(&rest[..start]);
        match std::env::var(name) {
            Ok(v) if !name.is_empty() => out.push_str(&v),
            _ => out.push_str(&rest[start..start + len + 2]),
        }
        rest = &rest[start + len + 2..];
    }
    out.push_str(rest);
    out
}

#[cfg(windows)]
mod imp {
    use super::{Profile, Rule, expand_env};
    use windows::Win32::NetworkManagement::WindowsFirewall::*;
    use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize, IDispatch};
    use windows::Win32::System::Ole::IEnumVARIANT;
    use windows::Win32::System::Variant::{VARIANT, VT_DISPATCH, VariantClear};
    use windows::core::{Interface, Result};

    const PROFILES: [(NET_FW_PROFILE_TYPE2, &str); 3] =
        [(NET_FW_PROFILE2_DOMAIN, "Domain"), (NET_FW_PROFILE2_PRIVATE, "Private"), (NET_FW_PROFILE2_PUBLIC, "Public")];

    /// The active profiles with the firewall on, and the enabled inbound rules for `exe`.
    pub(super) fn read(exe: &str) -> Result<(Vec<Profile>, Vec<Rule>)> {
        unsafe {
            let init = CoInitializeEx(None, COINIT_MULTITHREADED);
            let r = (|| {
                let policy: INetFwPolicy2 = CoCreateInstance(&NetFwPolicy2, None, CLSCTX_INPROC_SERVER)?;
                let active = policy.CurrentProfileTypes()?;
                let mut profiles = Vec::new();
                for (p, name) in PROFILES {
                    if active & p.0 != 0 && policy.get_FirewallEnabled(p)?.as_bool() {
                        profiles.push(Profile {
                            name,
                            bit: p.0,
                            block_all: policy.get_BlockAllInboundTraffic(p)?.as_bool(),
                            silent: policy.get_NotificationsDisabled(p)?.as_bool(),
                            default_allow: policy.get_DefaultInboundAction(p)? == NET_FW_ACTION_ALLOW,
                        });
                    }
                }
                Ok((profiles, rules_for(&policy, &exe.to_lowercase())?))
            })();
            if init.is_ok() {
                CoUninitialize();
            }
            r
        }
    }

    /// Enabled inbound rules for the executable `exe` (lowercase).
    unsafe fn rules_for(policy: &INetFwPolicy2, exe: &str) -> Result<Vec<Rule>> {
        let mut out = Vec::new();
        unsafe {
            let items: IEnumVARIANT = policy.Rules()?._NewEnum()?.cast()?;
            loop {
                let mut v = [VARIANT::default()];
                let mut fetched = 0;
                if items.Next(&mut v, &mut fetched).is_err() || fetched == 0 {
                    break;
                }
                let inner = &v[0].Anonymous.Anonymous;
                let disp: Option<IDispatch> = if inner.vt == VT_DISPATCH { (*inner.Anonymous.pdispVal).clone() } else { None };
                let _ = VariantClear(&mut v[0]);
                let Some(rule) = disp.and_then(|d| d.cast::<INetFwRule>().ok()) else { continue };
                let app = rule.ApplicationName().map(|s| s.to_string()).unwrap_or_default();
                if app.is_empty()
                    || expand_env(&app).to_lowercase() != exe
                    || rule.Direction()? != NET_FW_RULE_DIR_IN
                    || !rule.Enabled()?.as_bool()
                {
                    continue;
                }
                out.push(Rule {
                    name: rule.Name()?.to_string(),
                    allow: rule.Action()? == NET_FW_ACTION_ALLOW,
                    protocol: rule.Protocol()?,
                    ports: rule.LocalPorts().map(|s| s.to_string()).unwrap_or_default(),
                    profiles: rule.Profiles()?,
                });
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ports() {
        assert!(port_matches("*", 8080));
        assert!(port_matches("80, 8080", 8080));
        assert!(port_matches("8000-8100", 8080));
        assert!(!port_matches("80,443", 8080));
        assert!(!port_matches("RPC", 8080));
    }

    #[test]
    fn env() {
        // SAFETY: nothing else reads this variable.
        unsafe { std::env::set_var("LOUPECAM_TEST_DIR", r"C:\Apps") };
        assert_eq!(expand_env(r"%LOUPECAM_TEST_DIR%\x.exe"), r"C:\Apps\x.exe");
        assert_eq!(expand_env(r"%NOPE_NOT_SET%\x.exe"), r"%NOPE_NOT_SET%\x.exe");
        assert_eq!(expand_env("100%"), "100%");
    }

    fn profile(name: &'static str, bit: i32) -> Profile {
        Profile { name, bit, block_all: false, silent: false, default_allow: false }
    }

    fn rule(name: &str, allow: bool, protocol: i32, profiles: i32) -> Rule {
        Rule { name: name.into(), allow, protocol, ports: String::new(), profiles }
    }

    #[test]
    fn verdicts() {
        let private = [profile("Private", 2)];
        // What dismissing the prompt leaves behind: block rules for TCP and UDP.
        let dismissed = [rule("loupecam.exe", false, TCP, 2 | 4), rule("loupecam.exe", false, UDP, 2 | 4)];
        let w = warnings(r"C:\x\loupecam.exe", 8080, true, &private, &dismissed);
        assert_eq!(w.len(), 2);
        assert!(w[0].contains("TCP 8080") && w[0].contains("blocked by rule \"loupecam.exe\""));
        assert!(w[1].contains("UDP 5353"));
        assert!(w[0].contains(r"-Program 'C:\x\loupecam.exe'") && w[0].contains("-Profile Private"));

        // Allowed, or blocked only on a profile that isn't active.
        let allowed = [rule("a", true, ANY, 2), rule("b", false, ANY, 4)];
        assert!(warnings("x", 8080, true, &private, &allowed).is_empty());

        // A block for another port doesn't apply; a block wins over an allow.
        let mut other_port = rule("p", false, TCP, 2);
        other_port.ports = "9000".into();
        assert!(warnings("x", 8080, false, &private, &[other_port]).is_empty());
        assert_eq!(warnings("x", 8080, false, &private, &[rule("a", true, TCP, 2), rule("b", false, TCP, 2)]).len(), 1);

        // No rule: Windows asks (fine), unless it's set not to.
        assert!(warnings("x", 8080, false, &private, &[]).is_empty());
        let silent = [Profile { silent: true, ..profile("Private", 2) }];
        assert!(warnings("x", 8080, false, &silent, &[])[0].contains("set not to ask"));

        let shields_up = [Profile { block_all: true, ..profile("Public", 4) }];
        assert!(warnings("x", 8080, false, &shields_up, &allowed)[0].contains("blocks all incoming"));
    }

    /// Reads the real policy and prints what it finds for `LOUPECAM_FIREWALL_EXE` (e.g.
    /// a `loupecam.exe` that has prompted before), or this test binary.
    #[cfg(windows)]
    #[test]
    #[ignore = "reads the local firewall policy"]
    fn read_policy() {
        let exe = std::env::var("LOUPECAM_FIREWALL_EXE")
            .unwrap_or_else(|_| std::env::current_exe().unwrap().to_string_lossy().into_owned());
        let (profiles, rules) = imp::read(&exe).unwrap();
        println!("active profiles: {profiles:?}
{exe}: {rules:?}");
        for w in warnings(&exe, 8080, true, &profiles, &rules) {
            println!("{w}");
        }
    }
}
