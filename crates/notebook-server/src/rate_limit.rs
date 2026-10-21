//! Per-client token-bucket rate limiting.
//!
//! The global limiter guards every route. Endpoints needing stricter quotas
//! build their own [`KeyedLimiter`] and attach [`middleware`] with
//! `route_layer`.

use std::net::IpAddr;
use std::num::NonZeroU32;
use std::sync::{Arc, Weak};
use std::time::Duration;

use axum::extract::{Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use governor::clock::Clock;
use governor::{DefaultKeyedRateLimiter, Quota, RateLimiter};

use crate::client_ip::ClientIp;

/// How often idle buckets are dropped from the key map.
const RETAIN_INTERVAL: Duration = Duration::from_secs(60);

/// Bucket key: an IPv4 address, or an IPv6 address's /64 prefix.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum IpKey {
    V4(u32),
    V6(u64),
}

impl From<IpAddr> for IpKey {
    fn from(ip: IpAddr) -> Self {
        match ip.to_canonical() {
            IpAddr::V4(v4) => IpKey::V4(v4.to_bits()),
            IpAddr::V6(v6) => IpKey::V6((v6.to_bits() >> 64) as u64),
        }
    }
}

/// Wider IPv6 prefixes that also get a bucket, as (prefix length, quota
/// multiplier). One allocation holds 256 /64s per /56 and 65,536 per /48, so
/// without these, rotating source /64s would multiply the limit that much.
const V6_AGGREGATES: [(u32, u32); 2] = [(56, 16), (48, 64)];

/// A token bucket per [`IpKey`], plus buckets for the wider IPv6 prefixes in
/// [`V6_AGGREGATES`]. A request must pass every bucket that applies.
pub struct KeyedLimiter {
    per_client: DefaultKeyedRateLimiter<IpKey>,
    v6_prefixes: Vec<(u32, DefaultKeyedRateLimiter<u64>)>,
}

impl KeyedLimiter {
    pub fn new(per_second: NonZeroU32, burst: NonZeroU32) -> Self {
        let quota = |multiplier: u32| {
            let multiplier = NonZeroU32::new(multiplier).unwrap_or(NonZeroU32::MIN);
            Quota::per_second(per_second.saturating_mul(multiplier))
                .allow_burst(burst.saturating_mul(multiplier))
        };
        KeyedLimiter {
            per_client: RateLimiter::keyed(quota(1)),
            v6_prefixes: V6_AGGREGATES
                .iter()
                .map(|&(len, multiplier)| (len, RateLimiter::keyed(quota(multiplier))))
                .collect(),
        }
    }

    /// Takes a token for `ip`, or returns how long until one is available.
    pub fn check(&self, ip: IpAddr) -> Result<(), Duration> {
        let ip = ip.to_canonical();
        self.per_client
            .check_key(&IpKey::from(ip))
            .map_err(|not_until| not_until.wait_time_from(self.per_client.clock().now()))?;
        if let IpAddr::V6(v6) = ip {
            for (len, limiter) in &self.v6_prefixes {
                let prefix = (v6.to_bits() >> (128 - len)) as u64;
                limiter
                    .check_key(&prefix)
                    .map_err(|not_until| not_until.wait_time_from(limiter.clock().now()))?;
            }
        }
        Ok(())
    }

    /// Drops buckets that have refilled completely.
    pub fn retain_recent(&self) {
        self.per_client.retain_recent();
        for (_, limiter) in &self.v6_prefixes {
            limiter.retain_recent();
        }
    }
}

/// Calls [`KeyedLimiter::retain_recent`] every minute until the limiter is
/// dropped.
pub fn spawn_retain_task(limiter: &Arc<KeyedLimiter>) -> tokio::task::JoinHandle<()> {
    let weak: Weak<KeyedLimiter> = Arc::downgrade(limiter);
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(RETAIN_INTERVAL);
        interval.tick().await;
        loop {
            interval.tick().await;
            match weak.upgrade() {
                Some(limiter) => limiter.retain_recent(),
                None => break,
            }
        }
    })
}

/// Middleware rejecting over-limit requests with `429` and `Retry-After`.
/// Runs after [`crate::client_ip::middleware`].
pub async fn middleware(
    State(limiter): State<Arc<KeyedLimiter>>,
    request: Request,
    next: Next,
) -> Response {
    let Some(ClientIp(ip)) = request.extensions().get::<ClientIp>().copied() else {
        tracing::error!("rate limiter ran without a resolved client address");
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    match limiter.check(ip) {
        Ok(()) => next.run(request).await,
        Err(wait) => too_many_requests(wait),
    }
}

fn too_many_requests(wait: Duration) -> Response {
    let secs = wait.as_secs() + u64::from(wait.subsec_nanos() > 0);
    let mut response = StatusCode::TOO_MANY_REQUESTS.into_response();
    response
        .headers_mut()
        .insert(header::RETRY_AFTER, HeaderValue::from(secs.max(1)));
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ipv6_keys_share_a_64_prefix() {
        let a: IpAddr = "2001:db8:1:2::1".parse().unwrap();
        let b: IpAddr = "2001:db8:1:2:ffff::9".parse().unwrap();
        let c: IpAddr = "2001:db8:1:3::1".parse().unwrap();
        assert_eq!(IpKey::from(a), IpKey::from(b));
        assert_ne!(IpKey::from(a), IpKey::from(c));
    }

    #[test]
    fn mapped_ipv4_keys_as_ipv4() {
        let mapped: IpAddr = "::ffff:192.0.2.1".parse().unwrap();
        let v4: IpAddr = "192.0.2.1".parse().unwrap();
        assert_eq!(IpKey::from(mapped), IpKey::from(v4));
    }

    #[test]
    fn retry_after_rounds_up() {
        let r = too_many_requests(Duration::from_millis(1200));
        assert_eq!(r.headers()[header::RETRY_AFTER], "2");
        let r = too_many_requests(Duration::from_millis(0));
        assert_eq!(r.headers()[header::RETRY_AFTER], "1");
    }

    fn tight() -> KeyedLimiter {
        KeyedLimiter::new(NonZeroU32::new(1).unwrap(), NonZeroU32::new(1).unwrap())
    }

    #[test]
    fn rotating_64s_share_the_56_limit() {
        let limiter = tight();
        // Distinct /64s inside 2001:db8:0:100::/56; that /56 allows 16.
        let passed = (0..32u16)
            .filter(|i| {
                let ip: IpAddr = format!("2001:db8:0:1{i:02x}::1").parse().unwrap();
                limiter.check(ip).is_ok()
            })
            .count();
        assert_eq!(passed, 16);
    }

    #[test]
    fn rotating_56s_share_the_48_limit() {
        let limiter = tight();
        // One /64 in each of 128 distinct /56s inside 2001:db8:0::/48.
        let passed = (0..128u16)
            .filter(|i| {
                let ip: IpAddr = format!("2001:db8:0:{i:02x}00::1").parse().unwrap();
                limiter.check(ip).is_ok()
            })
            .count();
        assert_eq!(passed, 64);

        // Another /48 is unaffected.
        assert!(limiter.check("2001:db8:1::1".parse().unwrap()).is_ok());
    }

    #[test]
    fn burst_then_reject() {
        let limiter = KeyedLimiter::new(NonZeroU32::new(1).unwrap(), NonZeroU32::new(2).unwrap());
        let ip: IpAddr = "192.0.2.1".parse().unwrap();
        assert!(limiter.check(ip).is_ok());
        assert!(limiter.check(ip).is_ok());
        let wait = limiter.check(ip).unwrap_err();
        assert!(wait > Duration::ZERO && wait <= Duration::from_secs(1));
    }
}
