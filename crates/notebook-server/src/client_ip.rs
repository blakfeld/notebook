//! Client address resolution: the TCP peer, or the forwarded address when the
//! peer is a trusted proxy.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use axum::extract::{ConnectInfo, Request, State};
use axum::http::{HeaderValue, header::HeaderName};
use axum::middleware::Next;
use axum::response::Response;

/// The resolved client address, inserted as a request extension.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClientIp(pub IpAddr);

static X_FORWARDED_FOR: HeaderName = HeaderName::from_static("x-forwarded-for");

/// Resolves the client address. Forwarding headers are read only when `peer`
/// is in `trusted`; then entries are walked right to left past trusted
/// proxies, and the first other entry is the client, because entries further
/// left are whatever the client claimed. If that entry doesn't parse (say a
/// proxy wrote `unknown`), the walk stops there and `peer` is used: reading
/// past it would trust client-supplied values. Blank entries are skipped.
pub fn resolve<'a>(
    peer: IpAddr,
    forwarded_for: impl DoubleEndedIterator<Item = &'a HeaderValue>,
    trusted: &[IpAddr],
) -> IpAddr {
    let peer = peer.to_canonical();
    if !trusted.contains(&peer) {
        return peer;
    }
    // Split raw bytes so one non-ASCII entry (client-supplied, to the left)
    // doesn't hide the entry the proxy appended.
    forwarded_for
        .rev()
        .flat_map(|v| v.as_bytes().rsplit(|&b| b == b','))
        .filter(|entry| !entry.trim_ascii().is_empty())
        .map(|entry| std::str::from_utf8(entry).ok().and_then(parse_entry))
        .find(|ip| !ip.is_some_and(|ip| trusted.contains(&ip)))
        .flatten()
        .unwrap_or(peer)
}

fn parse_entry(entry: &str) -> Option<IpAddr> {
    let entry = entry.trim();
    entry
        .parse::<IpAddr>()
        .or_else(|_| entry.parse::<SocketAddr>().map(|s| s.ip()))
        .ok()
        .map(|ip| ip.to_canonical())
}

/// Middleware inserting [`ClientIp`]. Requires serving with connect info;
/// without it the extractor rejects the request with `500`.
pub async fn middleware(
    State(trusted): State<Arc<[IpAddr]>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    mut request: Request,
    next: Next,
) -> Response {
    let ip = resolve(
        peer.ip(),
        request.headers().get_all(&X_FORWARDED_FOR).iter(),
        &trusted,
    );
    request.extensions_mut().insert(ClientIp(ip));
    next.run(request).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    fn headers(values: &[&str]) -> Vec<HeaderValue> {
        values
            .iter()
            .map(|v| HeaderValue::from_str(v).unwrap())
            .collect()
    }

    const PROXY: &str = "127.0.0.1";

    #[test]
    fn untrusted_peer_ignores_spoofed_header() {
        let h = headers(&["203.0.113.7"]);
        assert_eq!(
            resolve(ip("198.51.100.1"), h.iter(), &[ip(PROXY)]),
            ip("198.51.100.1")
        );
    }

    #[test]
    fn trusted_proxy_uses_forwarded_address() {
        let h = headers(&["203.0.113.7"]);
        assert_eq!(
            resolve(ip(PROXY), h.iter(), &[ip(PROXY)]),
            ip("203.0.113.7")
        );
    }

    #[test]
    fn right_most_untrusted_entry_wins() {
        // The client claimed 1.1.1.1; the proxy appended the real address.
        let h = headers(&["1.1.1.1, 203.0.113.7"]);
        assert_eq!(
            resolve(ip(PROXY), h.iter(), &[ip(PROXY)]),
            ip("203.0.113.7")
        );
    }

    #[test]
    fn chained_trusted_proxies_are_skipped() {
        let trusted = [ip(PROXY), ip("10.0.0.2")];
        let h = headers(&["1.1.1.1, 203.0.113.7", "10.0.0.2"]);
        assert_eq!(resolve(ip(PROXY), h.iter(), &trusted), ip("203.0.113.7"));
    }

    #[test]
    fn unparseable_client_entry_falls_back_to_peer() {
        // Never read past an entry that doesn't parse into client-supplied
        // values further left.
        for value in [
            "6.6.6.6, unknown",
            "6.6.6.6, unix:",
            "6.6.6.6, fe80::1%eth0",
        ] {
            let h = headers(&[value]);
            assert_eq!(
                resolve(ip(PROXY), h.iter(), &[ip(PROXY)]),
                ip(PROXY),
                "{value}"
            );
        }

        // Blank entries are not addresses and are skipped.
        let h = headers(&["6.6.6.6, 203.0.113.7, ,"]);
        assert_eq!(
            resolve(ip(PROXY), h.iter(), &[ip(PROXY)]),
            ip("203.0.113.7")
        );

        // Garbage to the left of the proxy's entry is never reached.
        let h = [HeaderValue::from_bytes(b"caf\xc3\xa9, 203.0.113.7").unwrap()];
        assert_eq!(
            resolve(ip(PROXY), h.iter(), &[ip(PROXY)]),
            ip("203.0.113.7")
        );

        let h = headers(&["junk"]);
        assert_eq!(resolve(ip(PROXY), h.iter(), &[ip(PROXY)]), ip(PROXY));
    }

    #[test]
    fn missing_header_falls_back_to_peer() {
        assert_eq!(resolve(ip(PROXY), [].iter(), &[ip(PROXY)]), ip(PROXY));
    }

    #[test]
    fn ports_and_mapped_addresses_are_normalized() {
        let h = headers(&["203.0.113.7:4444"]);
        assert_eq!(
            resolve(ip("::ffff:127.0.0.1"), h.iter(), &[ip(PROXY)]),
            ip("203.0.113.7")
        );
        let h = headers(&["[2001:db8::1]:443"]);
        assert_eq!(
            resolve(ip(PROXY), h.iter(), &[ip(PROXY)]),
            ip("2001:db8::1")
        );
    }
}
