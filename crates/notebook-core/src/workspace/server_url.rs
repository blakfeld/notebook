//! The server address a user types at sign-in.

use std::fmt;

use crate::sync::is_loopback;

/// A validated server address: `https://host[:port]`, or `http://` for a
/// server on this machine. Always without a trailing slash.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ServerUrl(String);

/// Why an address was refused. The message names the problem.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerUrlError {
    /// Not of the form `scheme://host`.
    Malformed,
    /// No host after the scheme.
    MissingHost,
    /// A scheme other than `https` or `http`.
    UnsupportedScheme(String),
    /// `http` to a host that isn't this machine.
    HttpsRequired,
    /// A path, query, fragment, or user name after the host.
    UnexpectedPart,
    /// A port that isn't a number from 1 to 65535.
    InvalidPort,
    /// Whitespace or another character no host name contains.
    InvalidHost,
}

impl fmt::Display for ServerUrlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Malformed => f.write_str("the server address should look like https://host"),
            Self::MissingHost => f.write_str("the server address has no host name"),
            Self::UnsupportedScheme(scheme) => {
                write!(
                    f,
                    "the server address must start with https://, not {scheme}://"
                )
            }
            Self::HttpsRequired => f.write_str(
                "the server address must use HTTPS (plain HTTP works only for this computer)",
            ),
            Self::UnexpectedPart => {
                f.write_str("the server address should be only https://host, with no path")
            }
            Self::InvalidPort => f.write_str("the server address has an invalid port"),
            Self::InvalidHost => f.write_str("the server address has an invalid host name"),
        }
    }
}

impl std::error::Error for ServerUrlError {}

impl ServerUrl {
    /// Checks `input` (surrounding whitespace ignored). The scheme and host
    /// are lowercased.
    pub fn parse(input: &str) -> Result<Self, ServerUrlError> {
        let input = input.trim();
        let (scheme, rest) = input.split_once("://").ok_or(ServerUrlError::Malformed)?;
        let scheme = scheme.to_ascii_lowercase();
        if scheme != "https" && scheme != "http" {
            return Err(ServerUrlError::UnsupportedScheme(scheme));
        }
        // The sync and auth clients address routes from the root, so a path
        // would be silently ignored; refuse it instead.
        let authority = rest.strip_suffix('/').unwrap_or(rest);
        if authority.contains(['/', '?', '#', '@']) {
            return Err(ServerUrlError::UnexpectedPart);
        }
        let (host, port) = match authority.strip_prefix('[') {
            Some(v6) => {
                let (host, rest) = v6.split_once(']').ok_or(ServerUrlError::InvalidHost)?;
                if host.parse::<std::net::Ipv6Addr>().is_err() {
                    return Err(ServerUrlError::InvalidHost);
                }
                match rest {
                    "" => (host, None),
                    rest => (
                        host,
                        Some(rest.strip_prefix(':').ok_or(ServerUrlError::InvalidHost)?),
                    ),
                }
            }
            None => match authority.split_once(':') {
                Some((host, port)) => (host, Some(port)),
                None => (authority, None),
            },
        };
        if host.is_empty() {
            return Err(ServerUrlError::MissingHost);
        }
        if !host
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '.' | '-' | '_' | ':'))
        {
            return Err(ServerUrlError::InvalidHost);
        }
        if let Some(port) = port
            && !port.parse::<u16>().is_ok_and(|p| p > 0)
        {
            return Err(ServerUrlError::InvalidPort);
        }
        if scheme == "http" && !is_loopback(authority) {
            return Err(ServerUrlError::HttpsRequired);
        }
        Ok(Self(format!(
            "{scheme}://{}",
            authority.to_ascii_lowercase()
        )))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ServerUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(input: &str) -> String {
        ServerUrl::parse(input).unwrap().as_str().to_owned()
    }

    #[test]
    fn https_address_accepted() {
        assert_eq!(ok("https://notes.example.com"), "https://notes.example.com");
        assert_eq!(
            ok("https://notes.example.com:8443"),
            "https://notes.example.com:8443"
        );
    }

    #[test]
    fn plain_http_to_a_remote_host_rejected() {
        let err = ServerUrl::parse("http://notes.example.com").unwrap_err();
        assert_eq!(err, ServerUrlError::HttpsRequired);
        assert!(err.to_string().contains("HTTPS"));
    }

    #[test]
    fn plain_http_to_loopback_accepted() {
        assert_eq!(ok("http://127.0.0.1:8080"), "http://127.0.0.1:8080");
        assert_eq!(ok("http://[::1]"), "http://[::1]");
        assert_eq!(ok("http://[::1]:3000"), "http://[::1]:3000");
        assert_eq!(ok("http://localhost:8080"), "http://localhost:8080");
    }

    #[test]
    fn trailing_slash_and_case_normalized() {
        assert_eq!(
            ok("https://Notes.Example.com/"),
            "https://notes.example.com"
        );
        assert_eq!(
            ok("  HTTPS://notes.example.com  "),
            "https://notes.example.com"
        );
    }

    #[test]
    fn other_schemes_rejected() {
        let err = ServerUrl::parse("ftp://notes.example.com").unwrap_err();
        assert_eq!(err, ServerUrlError::UnsupportedScheme("ftp".into()));
        assert!(err.to_string().contains("ftp://"));
    }

    #[test]
    fn missing_host_rejected() {
        assert_eq!(
            ServerUrl::parse("https://").unwrap_err(),
            ServerUrlError::MissingHost
        );
        assert_eq!(
            ServerUrl::parse("https://:8080").unwrap_err(),
            ServerUrlError::MissingHost
        );
        assert_eq!(
            ServerUrl::parse("notes.example.com").unwrap_err(),
            ServerUrlError::Malformed
        );
    }

    #[test]
    fn bad_ports_and_hosts_rejected() {
        for (input, err) in [
            (
                "https://notes.example.com:99999",
                ServerUrlError::InvalidPort,
            ),
            ("https://notes.example.com:abc", ServerUrlError::InvalidPort),
            ("https://notes.example.com:", ServerUrlError::InvalidPort),
            ("https://notes.example.com:0", ServerUrlError::InvalidPort),
            ("https://exa mple.com", ServerUrlError::InvalidHost),
            ("http://[::1", ServerUrlError::InvalidHost),
            ("http://[nope]", ServerUrlError::InvalidHost),
            ("http://[::1]x", ServerUrlError::InvalidHost),
        ] {
            assert_eq!(ServerUrl::parse(input).unwrap_err(), err, "{input}");
        }
    }

    #[test]
    fn extra_parts_rejected() {
        for input in [
            "https://notes.example.com/app",
            "https://notes.example.com?x=1",
            "https://user@notes.example.com",
            // A user name can make an address look like loopback.
            "http://localhost@notes.example.com",
        ] {
            assert_eq!(
                ServerUrl::parse(input).unwrap_err(),
                ServerUrlError::UnexpectedPart,
                "{input}"
            );
        }
    }
}
