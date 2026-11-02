//! All HTTP engine and result URLs use the public-address boundary.
use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;

pub const MAX_BODY_BYTES: usize = 8 * 1024 * 1024;

pub fn public_address(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(ip) => {
            let [a, b, c, _] = ip.octets();
            !(a == 0
                || a == 10
                || a == 127
                || a >= 224
                || (a == 100 && (64..=127).contains(&b))
                || (a == 169 && b == 254)
                || (a == 172 && (16..=31).contains(&b))
                || (a == 192 && b == 168)
                // Only the IETF protocol-assignment block 192.0.0.0/24 and the
                // documentation block 192.0.2.0/24. The rest of 192.0.0.0/16 is
                // ordinary public space (e.g. wordpress.com on 192.0.78.0/24).
                || (a == 192 && b == 0 && (c == 0 || c == 2))
                || (a == 192 && b == 88 && c == 99)
                || (a == 198 && (b == 18 || b == 19))
                || ip.is_documentation()
                || ip == Ipv4Addr::BROADCAST)
        }
        IpAddr::V6(ip) => {
            let s = ip.segments();
            // Global unicast only. Exclude tunnelling, documentation and special use.
            (s[0] & 0xe000) == 0x2000
                && s[0] != 0x2002
                && !(s[0] == 0x2001 && (s[1] <= 0x01ff || s[1] == 0x0db8))
                && s[0] != 0x3fff
        }
    }
}

pub fn public_url(url: &url::Url) -> bool {
    if !matches!(url.scheme(), "https" | "http")
        || !url.username().is_empty()
        || url.password().is_some()
        || !matches!(url.port_or_known_default(), Some(80 | 443))
    {
        return false;
    }
    match url.host() {
        Some(url::Host::Ipv4(ip)) => public_address(ip.into()),
        Some(url::Host::Ipv6(ip)) => public_address(ip.into()),
        Some(url::Host::Domain(name)) => {
            let name = name.trim_end_matches('.').to_ascii_lowercase();
            name.contains('.')
                && name != "localhost"
                && !name.ends_with(".localhost")
                && !name.ends_with(".local")
                && !name.ends_with(".internal")
                && !name.ends_with(".home.arpa")
        }
        None => false,
    }
}

#[derive(Debug)]
struct PublicResolver;
impl Resolve for PublicResolver {
    fn resolve(&self, name: Name) -> Resolving {
        Box::pin(async move {
            let addresses: Vec<_> = tokio::net::lookup_host((name.as_str(), 0)).await?.collect();
            if addresses.is_empty() || addresses.iter().any(|a| !public_address(a.ip())) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "public_dns_required",
                )
                .into());
            }
            // These are the same addresses reqwest connects to: no second DNS lookup.
            Ok(Box::new(addresses.into_iter()) as Addrs)
        })
    }
}

pub fn client() -> Result<reqwest::Client, reqwest::Error> {
    reqwest::Client::builder()
        .no_proxy()
        .referer(false)
        .dns_resolver(Arc::new(PublicResolver))
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() >= 5 {
                attempt.error("redirect_limit")
            } else if !public_url(attempt.url()) {
                attempt.error("public_redirect_required")
            } else {
                attempt.follow()
            }
        }))
        .connect_timeout(std::time::Duration::from_secs(3))
        .timeout(std::time::Duration::from_secs(8))
        .user_agent("RustXNG/0.26 local-research")
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_special_addresses_and_ambiguous_urls() {
        for value in [
            "http://127.1",
            "http://2130706433",
            "http://[::1]",
            "http://[::ffff:127.0.0.1]",
            "http://[fc00::1]",
            "http://[fe80::1]",
            "http://100.64.0.1",
            "http://198.19.0.1",
            "http://169.254.169.254",
            "http://10.0.0.1",
            "http://192.168.1.1",
            "http://localhost.",
            "http://service.internal",
            "file:///etc/hosts",
            "https://user:pass@example.com",
            "http://example.com:8080",
            "http://[2002:7f00:1::]",
            "http://[2001:db8::1]",
        ] {
            assert!(!public_url(&url::Url::parse(value).unwrap()), "{value}");
        }
        for value in [
            "https://example.com",
            "http://8.8.8.8",
            "https://[2606:4700:4700::1111]",
        ] {
            assert!(public_url(&url::Url::parse(value).unwrap()), "{value}");
        }
    }
    #[test]
    fn only_the_two_special_192_0_blocks_are_refused() {
        for blocked in ["192.0.0.1", "192.0.0.255", "192.0.2.1", "192.0.2.254"] {
            let ip: IpAddr = blocked.parse().unwrap();
            assert!(!public_address(ip), "{blocked} must stay refused");
        }
        // wordpress.com / jetpack.com live in 192.0.78.0/24 and are public.
        for allowed in [
            "192.0.78.24",
            "192.0.78.17",
            "192.0.1.1",
            "192.0.3.1",
            "192.0.255.1",
        ] {
            let ip: IpAddr = allowed.parse().unwrap();
            assert!(public_address(ip), "{allowed} is public space");
        }
        assert!(public_url(&url::Url::parse("http://192.0.78.24").unwrap()));
        assert!(!public_url(&url::Url::parse("http://192.0.2.10").unwrap()));
    }
    #[tokio::test]
    async fn refuses_private_dns_results() {
        let name: Name = "localhost".parse().unwrap();
        assert!(PublicResolver.resolve(name).await.is_err());
    }
}
