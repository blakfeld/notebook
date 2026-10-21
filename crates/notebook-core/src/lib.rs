//! Platform-independent core for Notebook.
//!
//! This crate must build for native targets and `wasm32-unknown-unknown`.

#![forbid(unsafe_code)]

/// Returns the version of `notebook-core`.
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_matches_crate_version() {
        assert!(!version().is_empty());
        assert_eq!(version(), env!("CARGO_PKG_VERSION"));
    }
}
