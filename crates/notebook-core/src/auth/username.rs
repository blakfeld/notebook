//! Usernames: case-insensitive ASCII, normalized to lowercase.

use std::fmt;

use super::AuthError;

const MIN_LEN: usize = 3;
const MAX_LEN: usize = 32;

/// A validated, lowercase username. Its bytes are the OPAQUE credential
/// identifier, so client and server must normalize identically; the server
/// parses again rather than trusting the client.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Username(String);

impl Username {
    /// Lowercases `text` and checks it is 3 to 32 of `a-z`, `0-9`, `.`, `_`,
    /// `-`, starting with a letter or digit. Anything else is
    /// [`AuthError::InvalidUsername`].
    pub fn parse(text: &str) -> Result<Self, AuthError> {
        let name = text.to_ascii_lowercase();
        let bytes = name.as_bytes();
        let valid = (MIN_LEN..=MAX_LEN).contains(&bytes.len())
            && bytes[0].is_ascii_alphanumeric()
            && bytes
                .iter()
                .all(|&b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-".contains(&b));
        if valid {
            Ok(Self(name))
        } else {
            Err(AuthError::InvalidUsername)
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Username {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folds_case() {
        assert_eq!(Username::parse("Alice").unwrap().as_str(), "alice");
        assert_eq!(
            Username::parse("ALICE").unwrap(),
            Username::parse("alice").unwrap()
        );
    }

    #[test]
    fn length_limits() {
        assert!(Username::parse("ab").is_err());
        assert!(Username::parse("abc").is_ok());
        assert!(Username::parse(&"a".repeat(32)).is_ok());
        assert!(Username::parse(&"a".repeat(33)).is_err());
    }

    #[test]
    fn rejects_bad_characters() {
        for name in [
            "al ice", "ålice", ".alice", "-alice", "_alice", "al/ice", "",
        ] {
            assert_eq!(
                Username::parse(name),
                Err(AuthError::InvalidUsername),
                "{name:?}"
            );
        }
    }

    #[test]
    fn accepts_punctuation_after_the_first_character() {
        for name in ["a.b", "a_b", "a-b", "0day", "j.r.r-t_k"] {
            assert!(Username::parse(name).is_ok(), "{name}");
        }
    }
}
