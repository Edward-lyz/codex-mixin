//! Proxy selection shared by HTTP and WebSocket clients.
//!
//! Order: `NO_PROXY` goes direct, then the scheme's environment variable,
//! `ALL_PROXY`, the operating system's static proxy (never for loopback or its
//! own bypass list), and finally a direct connection. PAC, WPAD and system SOCKS discovery are out of scope.
//! Callers opt in explicitly; no client gains this behavior implicitly.
use std::net::{IpAddr, Ipv4Addr};

#[derive(Clone, Debug, Default)]
pub(crate) struct ProxyEnv {
    http_proxy: Option<String>,
    https_proxy: Option<String>,
    all_proxy: Option<String>,
    no_proxy: Option<String>,
}

impl ProxyEnv {
    pub(crate) fn from_lookup(env_lookup: &dyn Fn(&str) -> Option<String>) -> Self {
        Self {
            http_proxy: first_env(env_lookup, &["HTTP_PROXY", "http_proxy"]),
            https_proxy: first_env(env_lookup, &["HTTPS_PROXY", "https_proxy"]),
            all_proxy: first_env(env_lookup, &["ALL_PROXY", "all_proxy"]),
            no_proxy: first_env(env_lookup, &["NO_PROXY", "no_proxy"]),
        }
    }
}

fn first_env(env_lookup: &dyn Fn(&str) -> Option<String>, names: &[&str]) -> Option<String> {
    names
        .iter()
        .find_map(|name| env_lookup(name))
        .filter(|value| !value.trim().is_empty())
}

/// Static proxy settings configured in the operating system.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct SystemProxy {
    http: Option<String>,
    https: Option<String>,
    bypass: Vec<String>,
}

impl SystemProxy {
    /// Reads the current OS settings; unsupported platforms and unreadable
    /// settings yield `None`, which means a direct connection.
    pub(crate) fn current() -> Option<Self> {
        read_system_proxy().filter(|proxy| proxy.http.is_some() || proxy.https.is_some())
    }
}

/// Resolved proxy policy for one process lifetime.
#[derive(Clone, Debug, Default)]
pub(crate) struct ProxySelection {
    env: ProxyEnv,
    system: Option<SystemProxy>,
}

impl ProxySelection {
    pub(crate) fn from_lookup(env_lookup: &dyn Fn(&str) -> Option<String>) -> Self {
        Self {
            env: ProxyEnv::from_lookup(env_lookup),
            system: SystemProxy::current(),
        }
    }

    pub(crate) fn from_process() -> Self {
        Self::from_lookup(&|name| std::env::var(name).ok())
    }

    #[cfg(test)]
    fn with_system(env: ProxyEnv, system: Option<SystemProxy>) -> Self {
        Self { env, system }
    }

    /// Returns the proxy URL to use, or `None` for a direct connection.
    /// `scheme` is the target scheme (`http`, `https`, `ws`, `wss`).
    pub(crate) fn select(&self, scheme: &str, host: &str, port: u16) -> Option<String> {
        let host = host.trim_start_matches('[').trim_end_matches(']');
        let secure = matches!(scheme, "https" | "wss");
        if let Some(no_proxy) = self.env.no_proxy.as_deref()
            && matches_bypass_list(no_proxy.split(','), host, port)
        {
            return None;
        }
        let from_env = if secure {
            self.env
                .https_proxy
                .as_deref()
                .or(self.env.http_proxy.as_deref())
        } else {
            self.env.http_proxy.as_deref()
        }
        .or(self.env.all_proxy.as_deref());
        if let Some(proxy) = from_env {
            return Some(proxy.to_owned());
        }
        // Loopback never follows the OS proxy; explicit environment settings
        // keep their historical behavior and are limited by `NO_PROXY`.
        if is_loopback(host) {
            return None;
        }
        let system = self.system.as_ref()?;
        if matches_bypass_list(system.bypass.iter().map(String::as_str), host, port) {
            return None;
        }
        if secure {
            system.https.clone().or_else(|| system.http.clone())
        } else {
            system.http.clone()
        }
    }

    /// Routes a `reqwest` client through this policy. The closure answers
    /// every request, so reqwest's own environment handling is never reached
    /// for a request that this policy resolves.
    pub(crate) fn apply(self, builder: reqwest::ClientBuilder) -> reqwest::ClientBuilder {
        builder.proxy(reqwest::Proxy::custom(move |url| {
            let host = url.host_str()?;
            let port = url.port_or_known_default()?;
            self.select(url.scheme(), host, port)
        }))
    }
}

/// Applies the process environment and OS static proxy to a `reqwest` client.
pub fn auto_proxy(builder: reqwest::ClientBuilder) -> reqwest::ClientBuilder {
    ProxySelection::from_process().apply(builder)
}

fn is_loopback(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

fn matches_bypass_list<'a>(
    mut entries: impl Iterator<Item = &'a str>,
    host: &str,
    port: u16,
) -> bool {
    entries.any(|entry| matches_bypass_entry(entry.trim(), host, port))
}

fn matches_bypass_entry(entry: &str, host: &str, port: u16) -> bool {
    if entry.is_empty() {
        return false;
    }
    if entry == "*" {
        return true;
    }
    // The Windows `<local>` token covers hosts without a dot.
    if entry.eq_ignore_ascii_case("<local>") {
        return !host.contains('.') && !host.contains(':');
    }
    if let Some((network, prefix)) = entry.split_once('/') {
        return matches_ipv4_range(network, prefix, host);
    }
    let (entry_host, entry_port) = match entry.rsplit_once(':') {
        Some((name, port)) if !entry.starts_with('[') && !name.contains(':') => {
            (name, port.parse::<u16>().ok())
        }
        _ => (entry, None),
    };
    if entry_port.is_some_and(|entry_port| entry_port != port) {
        return false;
    }
    let entry_host = entry_host
        .trim_start_matches('[')
        .trim_end_matches(']')
        .trim_start_matches("*.");
    let host = host.to_ascii_lowercase();
    let entry_host = entry_host.to_ascii_lowercase();
    if let Some(suffix) = entry_host.strip_prefix('.') {
        return host == suffix || host.ends_with(&format!(".{suffix}"));
    }
    host == entry_host || host.ends_with(&format!(".{entry_host}"))
}

/// Handles macOS-style `169.254/16` entries as well as full dotted quads.
fn matches_ipv4_range(network: &str, prefix: &str, host: &str) -> bool {
    let Ok(prefix) = prefix.parse::<u32>() else {
        return false;
    };
    let Ok(IpAddr::V4(target)) = host.parse::<IpAddr>() else {
        return false;
    };
    let mut octets = [0u8; 4];
    let parts: Vec<&str> = network.split('.').collect();
    if parts.is_empty() || parts.len() > 4 || prefix > 32 {
        return false;
    }
    for (slot, part) in octets.iter_mut().zip(parts) {
        let Ok(value) = part.parse::<u8>() else {
            return false;
        };
        *slot = value;
    }
    let mask = u32::MAX.checked_shl(32 - prefix).unwrap_or(0);
    u32::from(Ipv4Addr::from(octets)) & mask == u32::from(target) & mask
}

#[cfg(any(windows, test))]
fn parse_windows_proxy(server: &str, overrides: &str) -> SystemProxy {
    let mut proxy = SystemProxy {
        bypass: overrides
            .split(';')
            .map(str::trim)
            .filter(|entry| !entry.is_empty())
            .map(str::to_owned)
            .collect(),
        ..SystemProxy::default()
    };
    for entry in server.split(';').map(str::trim) {
        if entry.is_empty() {
            continue;
        }
        match entry.split_once('=') {
            Some(("http", address)) => proxy.http = http_proxy_url(address),
            Some(("https", address)) => proxy.https = http_proxy_url(address),
            // ftp and socks entries are outside the supported surface.
            Some(_) => {}
            None => {
                let url = http_proxy_url(entry);
                proxy.http = url.clone();
                proxy.https = url;
            }
        }
    }
    proxy
}

#[cfg(any(windows, test))]
fn http_proxy_url(address: &str) -> Option<String> {
    let address = address.trim();
    if address.is_empty() {
        return None;
    }
    if address.contains("://") {
        return Some(address.to_owned());
    }
    Some(format!("http://{address}"))
}

#[cfg(target_os = "macos")]
fn read_system_proxy() -> Option<SystemProxy> {
    use system_configuration::core_foundation::array::CFArray;
    use system_configuration::core_foundation::base::{CFType, TCFType, TCFTypeRef};
    use system_configuration::core_foundation::dictionary::CFDictionary;
    use system_configuration::core_foundation::number::CFNumber;
    use system_configuration::core_foundation::propertylist::{
        create_data, kCFPropertyListXMLFormat_v1_0,
    };
    use system_configuration::core_foundation::string::CFString;
    use system_configuration::dynamic_store::SCDynamicStoreBuilder;

    fn endpoint(
        settings: &CFDictionary<CFString, CFType>,
        enable: &'static str,
        host: &'static str,
        port: &'static str,
    ) -> Option<String> {
        let enabled = settings
            .find(CFString::from_static_string(enable))?
            .downcast::<CFNumber>()?
            .to_i32()?;
        if enabled != 1 {
            return None;
        }
        let host = settings
            .find(CFString::from_static_string(host))?
            .downcast::<CFString>()?
            .to_string();
        let port = settings
            .find(CFString::from_static_string(port))
            .and_then(|value| value.downcast::<CFNumber>())
            .and_then(|value| value.to_i32());
        Some(match port {
            Some(port) => format!("http://{host}:{port}"),
            None => format!("http://{host}"),
        })
    }

    let settings = SCDynamicStoreBuilder::new("codex-mixin")
        .build()?
        .get_proxies()?;
    // The crate exposes untyped array elements only through unsafe code, so
    // serialize the list to an XML property list and read its strings.
    let bypass = settings
        .find(CFString::from_static_string("ExceptionsList"))
        .and_then(|value| value.downcast::<CFArray>())
        .and_then(|list| {
            create_data(
                list.as_concrete_TypeRef().as_void_ptr(),
                kCFPropertyListXMLFormat_v1_0,
            )
            .ok()
        })
        .map(|data| plist_strings(&String::from_utf8_lossy(data.bytes())))
        .unwrap_or_default();
    Some(SystemProxy {
        http: endpoint(&settings, "HTTPEnable", "HTTPProxy", "HTTPPort"),
        https: endpoint(&settings, "HTTPSEnable", "HTTPSProxy", "HTTPSPort"),
        bypass,
    })
}

#[cfg(any(target_os = "macos", test))]
fn plist_strings(xml: &str) -> Vec<String> {
    xml.split("<string>")
        .skip(1)
        .filter_map(|part| part.split_once("</string>"))
        .map(|(value, _)| {
            value
                .replace("&lt;", "<")
                .replace("&gt;", ">")
                .replace("&amp;", "&")
        })
        .collect()
}

#[cfg(windows)]
fn read_system_proxy() -> Option<SystemProxy> {
    let key = windows_registry::CURRENT_USER
        .open(r"Software\Microsoft\Windows\CurrentVersion\Internet Settings")
        .ok()?;
    if key.get_u32("ProxyEnable").ok()? != 1 {
        return None;
    }
    let server = key.get_string("ProxyServer").ok()?;
    let overrides = key.get_string("ProxyOverride").unwrap_or_default();
    Some(parse_windows_proxy(&server, &overrides))
}

#[cfg(not(any(target_os = "macos", windows)))]
fn read_system_proxy() -> Option<SystemProxy> {
    None
}

#[cfg(test)]
mod tests;
