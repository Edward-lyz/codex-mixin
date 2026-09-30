use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use anyhow::{Context, bail, ensure};
use base64::Engine;
use hickory_proto::op::{Message, MessageType, Query, ResponseCode};
use hickory_proto::rr::rdata::svcb::{SvcParamKey, SvcParamValue};
use hickory_proto::rr::{Name, RData, Record, RecordType};
use reqwest::Client;

use super::PLUS_DOH;

const DNS_TIMEOUT: Duration = Duration::from_secs(8);
const MAX_DNS_BYTES: usize = 65_535;
const MAX_ALIAS_HOPS: usize = 8;

pub(super) struct Resolution {
    pub(super) addresses: Vec<SocketAddr>,
    pub(super) ech: Vec<u8>,
    pub(super) alpn: Vec<Vec<u8>>,
    pub(super) ttl: u32,
}

pub(super) async fn resolve(host: &str) -> anyhow::Result<Resolution> {
    let client = Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(DNS_TIMEOUT)
        .build()?;
    let (services, service_ttl) = query(&client, host, RecordType::HTTPS).await?;
    let record = services
        .iter()
        .filter(|record| matches!(record.data(), RData::HTTPS(https) if https.svc_priority() > 0))
        .min_by_key(|record| match record.data() {
            RData::HTTPS(https) => https.svc_priority(),
            _ => u16::MAX,
        })
        .context("DoH returned no HTTPS ServiceMode record")?;
    let RData::HTTPS(service) = record.data() else {
        bail!("invalid HTTPS record")
    };
    let (ech, alpn) = service_parameters(service.svc_params())?;
    let target = if service.target_name().is_root() {
        record.name().to_utf8()
    } else {
        service.target_name().to_utf8()
    };
    // The advertised /plus relay is IPv4-only. Do not bypass it via CDN IPv6.
    let (records, address_ttl) = query(&client, &target, RecordType::A).await?;
    let mut addresses = Vec::new();
    for record in records {
        if let RData::A(address) = record.data() {
            let ip = address.0;
            ensure!(is_public_v4(ip), "DoH returned a non-public IPv4 address");
            let socket = SocketAddr::new(IpAddr::V4(ip), 443);
            if !addresses.contains(&socket) {
                addresses.push(socket);
            }
        }
    }
    ensure!(
        !addresses.is_empty(),
        "DoH returned no IPv4 addresses for ECH"
    );
    ensure!(
        addresses.len() <= 16,
        "DoH returned too many connection addresses"
    );
    Ok(Resolution {
        addresses,
        ech,
        alpn,
        ttl: service_ttl.min(address_ttl),
    })
}

fn is_public_v4(ip: std::net::Ipv4Addr) -> bool {
    let [a, b, _, _] = ip.octets();
    !ip.is_private()
        && !ip.is_loopback()
        && !ip.is_link_local()
        && !ip.is_broadcast()
        && !ip.is_documentation()
        && a != 0
        && a < 224
        && !(a == 100 && (64..=127).contains(&b))
        && !(a == 198 && matches!(b, 18 | 19))
}

fn service_parameters(
    params: &[(SvcParamKey, SvcParamValue)],
) -> anyhow::Result<(Vec<u8>, Vec<Vec<u8>>)> {
    for (_, value) in params {
        if let SvcParamValue::Mandatory(required) = value {
            for key in &required.0 {
                ensure!(
                    matches!(
                        key,
                        SvcParamKey::Alpn
                            | SvcParamKey::NoDefaultAlpn
                            | SvcParamKey::Port
                            | SvcParamKey::EchConfigList
                    ),
                    "unsupported mandatory HTTPS parameter: {key}"
                );
                ensure!(
                    params.iter().any(|(present, _)| present == key),
                    "missing mandatory HTTPS parameter: {key}"
                );
            }
        }
        if let SvcParamValue::Port(port) = value {
            ensure!(*port == 443, "ECH endpoint must use port 443");
        }
    }
    let ech = params
        .iter()
        .find_map(|(_, value)| match value {
            SvcParamValue::EchConfigList(config) => Some(config.0.clone()),
            _ => None,
        })
        .filter(|config| !config.is_empty())
        .context("DoH HTTPS record has no ECH configuration")?;
    let mut alpn = params
        .iter()
        .find_map(|(_, value)| match value {
            SvcParamValue::Alpn(protocols) => Some(
                protocols
                    .0
                    .iter()
                    .filter(|protocol| matches!(protocol.as_str(), "h2" | "http/1.1"))
                    .map(|protocol| protocol.as_bytes().to_vec())
                    .collect::<Vec<_>>(),
            ),
            _ => None,
        })
        .unwrap_or_default();
    if !params
        .iter()
        .any(|(key, _)| *key == SvcParamKey::NoDefaultAlpn)
        && !alpn.iter().any(|protocol| protocol == b"http/1.1")
    {
        alpn.push(b"http/1.1".to_vec());
    }
    ensure!(
        !alpn.is_empty(),
        "DoH HTTPS record offers no supported TCP protocol"
    );
    Ok((ech, alpn))
}

async fn query(
    client: &Client,
    host: &str,
    kind: RecordType,
) -> anyhow::Result<(Vec<Record>, u32)> {
    let mut name = Name::from_ascii(host)?;
    name.set_fqdn(true);
    let mut visited = Vec::new();
    let mut ttl = u32::MAX;
    for _ in 0..MAX_ALIAS_HOPS {
        ensure!(!visited.contains(&name), "DNS alias loop");
        visited.push(name.clone());
        let question = Query::query(name.clone(), kind);
        let mut message = Message::new();
        message
            .set_recursion_desired(true)
            .add_query(question.clone());
        let wire = message.to_vec()?;
        let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(wire);
        let mut response = client
            .get(PLUS_DOH)
            .query(&[("dns", encoded)])
            .header(reqwest::header::ACCEPT, "application/dns-message")
            .send()
            .await
            .context("query /plus DoH directly")?
            .error_for_status()?;
        ensure!(
            response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|header| header.to_str().ok())
                .is_some_and(|value| value.split(';').next() == Some("application/dns-message")),
            "DoH returned an invalid Content-Type"
        );
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            ensure!(
                bytes.len() + chunk.len() <= MAX_DNS_BYTES,
                "DoH response exceeds DNS message limit"
            );
            bytes.extend_from_slice(&chunk);
        }
        let response = parse_response(&bytes, &question)?;
        // Only follow records reachable from the question, never unrelated answers.
        for _ in 0..MAX_ALIAS_HOPS {
            let records = response
                .answers()
                .iter()
                .filter(|record| record.name() == &name)
                .collect::<Vec<_>>();
            let mut matching = Vec::new();
            let mut alias = None;
            for record in records {
                ttl = ttl.min(record.ttl());
                match record.data() {
                    RData::CNAME(target) => alias = Some(target.0.clone()),
                    RData::HTTPS(service)
                        if kind == RecordType::HTTPS && service.svc_priority() == 0 =>
                    {
                        ensure!(
                            !service.target_name().is_root(),
                            "invalid HTTPS AliasMode target"
                        );
                        alias = Some(service.target_name().clone());
                    }
                    _ if record.record_type() == kind => matching.push(record.clone()),
                    _ => {}
                }
            }
            if !matching.is_empty() {
                ensure!(
                    alias.is_none(),
                    "DNS returned conflicting alias and service records"
                );
                return Ok((matching, ttl));
            }
            let Some(target) = alias else { break };
            ensure!(!visited.contains(&target), "DNS alias loop");
            name = target;
        }
        // A missing address is a legitimate empty result; ECH requires one above.
        if visited.last() == Some(&name) {
            return Ok((Vec::new(), ttl));
        }
    }
    bail!("DNS alias chain exceeds {MAX_ALIAS_HOPS} hops")
}

fn parse_response(bytes: &[u8], question: &Query) -> anyhow::Result<Message> {
    let response = Message::from_vec(bytes).context("parse DoH DNS message")?;
    ensure!(
        response.message_type() == MessageType::Response && response.id() == 0,
        "invalid DoH response header"
    );
    ensure!(
        !response.truncated(),
        "DoH returned a truncated DNS message"
    );
    ensure!(
        response.response_code() == ResponseCode::NoError,
        "DoH returned DNS error {}",
        response.response_code()
    );
    let mut expected = question.clone();
    let mut name = expected.name().clone();
    name.set_fqdn(true);
    expected.set_name(name);
    ensure!(
        response.queries() == [expected],
        "DoH response does not match the question"
    );
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;
    use hickory_proto::rr::rdata::svcb::{Alpn, EchConfigList, Mandatory};

    #[test]
    fn accepts_real_plus_https_records() {
        for (host, bytes) in [
            (
                "chatgpt.com",
                include_bytes!("../../tests/fixtures/ech/chatgpt-https.dns").as_slice(),
            ),
            (
                "api.openai.com",
                include_bytes!("../../tests/fixtures/ech/openai-https.dns").as_slice(),
            ),
        ] {
            let question = Query::query(Name::from_ascii(host).unwrap(), RecordType::HTTPS);
            let message = parse_response(bytes, &question).unwrap();
            let service = message
                .answers()
                .iter()
                .find_map(|record| match record.data() {
                    RData::HTTPS(service) => Some(service),
                    _ => None,
                })
                .unwrap();
            let (ech, alpn) = service_parameters(service.svc_params()).unwrap();
            let tls = super::super::tls_config(&ech, alpn).unwrap();
            assert!(tls.alpn_protocols.iter().any(|protocol| protocol == b"h2"));
        }
    }

    #[test]
    fn rejects_missing_ech_and_unsupported_mandatory() {
        assert!(service_parameters(&[]).is_err());
        let mut params = vec![(
            SvcParamKey::EchConfigList,
            SvcParamValue::EchConfigList(EchConfigList(vec![1])),
        )];
        assert_eq!(
            service_parameters(&params).unwrap().1,
            vec![b"http/1.1".to_vec()]
        );
        params.push((
            SvcParamKey::Mandatory,
            SvcParamValue::Mandatory(Mandatory(vec![SvcParamKey::Unknown(123)])),
        ));
        assert!(service_parameters(&params).is_err());
    }

    #[test]
    fn rejects_quic_only_and_nonpublic_addresses() {
        let params = vec![
            (
                SvcParamKey::EchConfigList,
                SvcParamValue::EchConfigList(EchConfigList(vec![1])),
            ),
            (
                SvcParamKey::Alpn,
                SvcParamValue::Alpn(Alpn(vec!["h3".to_owned()])),
            ),
            (SvcParamKey::NoDefaultAlpn, SvcParamValue::NoDefaultAlpn),
        ];
        assert!(service_parameters(&params).is_err());
        for address in [
            "127.0.0.1",
            "10.0.0.1",
            "169.254.169.254",
            "100.64.0.1",
            "0.0.0.0",
            "224.0.0.1",
        ] {
            assert!(!is_public_v4(address.parse().unwrap()));
        }
        assert!(is_public_v4("172.64.146.83".parse().unwrap()));
    }

    #[test]
    fn validates_dns_question_and_truncation() {
        let question = Query::query(Name::from_ascii("chatgpt.com").unwrap(), RecordType::HTTPS);
        let mut message = Message::new();
        message
            .set_message_type(MessageType::Response)
            .add_query(question.clone());
        assert!(parse_response(&message.to_vec().unwrap(), &question).is_ok());
        message.set_truncated(true);
        assert!(parse_response(&message.to_vec().unwrap(), &question).is_err());
        message.set_truncated(false).set_id(1);
        assert!(parse_response(&message.to_vec().unwrap(), &question).is_err());
        let other = Query::query(Name::from_ascii("evil.test").unwrap(), RecordType::A);
        assert!(parse_response(&message.to_vec().unwrap(), &other).is_err());
    }
}
