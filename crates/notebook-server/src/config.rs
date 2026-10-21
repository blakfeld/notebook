//! Server configuration: built-in defaults, then an optional TOML file, then
//! `NOTEBOOK_`-prefixed environment variables, each overriding the last.

use std::fmt;
use std::net::{IpAddr, SocketAddr};
use std::num::{NonZeroU32, NonZeroU64, NonZeroUsize};
use std::path::{Path, PathBuf};
use std::str::FromStr;

use serde::Deserialize;

/// Validated server settings.
#[derive(Clone, Debug)]
pub struct Config {
    pub bind: SocketAddr,
    pub database_path: PathBuf,
    /// The origin clients reach the server at, `http://` or `https://`.
    pub public_url: String,
    pub static_dir: Option<PathBuf>,
    /// Exact proxy addresses whose `X-Forwarded-For` is believed.
    pub trusted_proxies: Vec<IpAddr>,
    pub max_body_bytes: NonZeroUsize,
    pub request_timeout_secs: NonZeroU64,
    pub rate_limit_per_second: NonZeroU32,
    pub rate_limit_burst: NonZeroU32,
    pub log_format: LogFormat,
    /// A `tracing` filter directive, such as `info` or `notebook_server=debug`.
    pub log_level: String,
}

impl Config {
    /// True when `public_url` uses `https`, so HSTS applies.
    pub fn is_https(&self) -> bool {
        self.public_url.starts_with("https://")
    }

    /// Loads from the optional file at `path` and the process environment.
    pub fn load(path: Option<&Path>) -> Result<Self, ConfigError> {
        let file = match path {
            Some(path) => {
                let text = std::fs::read_to_string(path).map_err(|e| ConfigError::Read {
                    path: path.to_owned(),
                    message: e.to_string(),
                })?;
                Some((path, text))
            }
            None => None,
        };
        Self::from_sources(file.as_ref().map(|(p, t)| (*p, t.as_str())), |name| {
            std::env::var(name).ok()
        })
    }

    /// Builds a config from file contents (with the path, for messages) and
    /// an environment lookup.
    pub fn from_sources(
        file: Option<(&Path, &str)>,
        env: impl Fn(&str) -> Option<String>,
    ) -> Result<Self, ConfigError> {
        let file: FileConfig = match file {
            Some((path, text)) => toml::from_str(text).map_err(|e| ConfigError::File {
                path: path.to_owned(),
                message: e.to_string(),
            })?,
            None => FileConfig::default(),
        };

        let public_url = env_value(&env, "public_url")?
            .or(file.public_url)
            .ok_or(ConfigError::Missing { key: "public_url" })?;
        validate_public_url(&public_url)?;

        let trusted_proxies = match env_raw(&env, "trusted_proxies") {
            Some(raw) => raw
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| {
                    s.parse()
                        .map_err(|e| invalid("NOTEBOOK_TRUSTED_PROXIES", s, e))
                })
                .collect::<Result<_, _>>()?,
            None => file.trusted_proxies.unwrap_or_default(),
        };

        let log_level = env_value(&env, "log_level")?
            .or(file.log_level)
            .unwrap_or_else(|| "info".to_owned());
        if let Err(e) = tracing_subscriber::EnvFilter::try_new(&log_level) {
            return Err(invalid("log_level", &log_level, e));
        }

        // An empty path would mean a temporary database, or serving the
        // working directory, so reject it outright.
        let database_path: PathBuf = env_value(&env, "database_path")?
            .or(file.database_path)
            .unwrap_or_else(|| PathBuf::from("notebook.db"));
        if database_path.as_os_str().is_empty() {
            return Err(invalid("database_path", "", "must not be empty"));
        }
        let static_dir: Option<PathBuf> = env_value(&env, "static_dir")?.or(file.static_dir);
        if static_dir
            .as_ref()
            .is_some_and(|p| p.as_os_str().is_empty())
        {
            return Err(invalid("static_dir", "", "must not be empty"));
        }

        Ok(Config {
            bind: env_value(&env, "bind")?
                .or(file.bind)
                .unwrap_or(SocketAddr::from(([127, 0, 0, 1], 8080))),
            database_path,
            public_url,
            static_dir,
            // Peers are compared in canonical form (IPv4-mapped IPv6 as IPv4).
            trusted_proxies: trusted_proxies
                .into_iter()
                .map(|ip: IpAddr| ip.to_canonical())
                .collect(),
            max_body_bytes: env_value(&env, "max_body_bytes")?
                .or(file.max_body_bytes)
                .unwrap_or(NonZeroUsize::new(1024 * 1024).unwrap()),
            request_timeout_secs: env_value(&env, "request_timeout_secs")?
                .or(file.request_timeout_secs)
                .unwrap_or(NonZeroU64::new(30).unwrap()),
            rate_limit_per_second: env_value(&env, "rate_limit_per_second")?
                .or(file.rate_limit_per_second)
                .unwrap_or(NonZeroU32::new(20).unwrap()),
            rate_limit_burst: env_value(&env, "rate_limit_burst")?
                .or(file.rate_limit_burst)
                .unwrap_or(NonZeroU32::new(100).unwrap()),
            log_format: env_value(&env, "log_format")?
                .or(file.log_format)
                .unwrap_or(LogFormat::Text),
            log_level,
        })
    }
}

/// Log output format.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogFormat {
    #[default]
    Text,
    Json,
}

impl FromStr for LogFormat {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "text" => Ok(LogFormat::Text),
            "json" => Ok(LogFormat::Json),
            _ => Err("expected `text` or `json`".to_owned()),
        }
    }
}

/// Why configuration could not be loaded. Every variant names its source.
#[derive(Debug)]
pub enum ConfigError {
    Read { path: PathBuf, message: String },
    File { path: PathBuf, message: String },
    Missing { key: &'static str },
    Invalid { key: String, message: String },
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::Read { path, message } => {
                write!(f, "cannot read config file {}: {message}", path.display())
            }
            ConfigError::File { path, message } => {
                write!(f, "invalid config file {}: {message}", path.display())
            }
            ConfigError::Missing { key } => write!(
                f,
                "missing required setting `{key}` (set it in the config file or {})",
                env_name(key)
            ),
            ConfigError::Invalid { key, message } => write!(f, "invalid `{key}`: {message}"),
        }
    }
}

impl std::error::Error for ConfigError {}

/// The file layer: every key optional, unknown keys rejected.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileConfig {
    bind: Option<SocketAddr>,
    database_path: Option<PathBuf>,
    public_url: Option<String>,
    static_dir: Option<PathBuf>,
    trusted_proxies: Option<Vec<IpAddr>>,
    max_body_bytes: Option<NonZeroUsize>,
    request_timeout_secs: Option<NonZeroU64>,
    rate_limit_per_second: Option<NonZeroU32>,
    rate_limit_burst: Option<NonZeroU32>,
    log_format: Option<LogFormat>,
    log_level: Option<String>,
}

fn env_name(key: &str) -> String {
    format!("NOTEBOOK_{}", key.to_ascii_uppercase())
}

fn env_raw(env: &impl Fn(&str) -> Option<String>, key: &str) -> Option<String> {
    env(&env_name(key))
}

fn env_value<T>(env: &impl Fn(&str) -> Option<String>, key: &str) -> Result<Option<T>, ConfigError>
where
    T: FromStr,
    T::Err: fmt::Display,
{
    let name = env_name(key);
    env(&name)
        .map(|raw| raw.parse().map_err(|e| invalid(&name, &raw, e)))
        .transpose()
}

fn invalid(key: &str, value: &str, error: impl fmt::Display) -> ConfigError {
    ConfigError::Invalid {
        key: key.to_owned(),
        message: format!("{value:?}: {error}"),
    }
}

fn validate_public_url(url: &str) -> Result<(), ConfigError> {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .ok_or_else(|| invalid("public_url", url, "must start with http:// or https://"))?;
    let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
    if host.is_empty() {
        return Err(invalid("public_url", url, "missing host"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env(vars: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |k| map.get(k).cloned()
    }

    fn load(file: Option<&str>, vars: &[(&str, &str)]) -> Result<Config, ConfigError> {
        Config::from_sources(file.map(|t| (Path::new("notebook.toml"), t)), env(vars))
    }

    const URL: (&str, &str) = ("NOTEBOOK_PUBLIC_URL", "https://notes.example.com");

    #[test]
    fn env_overrides_file() {
        let file = "public_url = \"http://localhost\"\nbind = \"127.0.0.1:9000\"\n";
        let config = load(Some(file), &[("NOTEBOOK_BIND", "0.0.0.0:7000")]).unwrap();
        assert_eq!(config.bind, "0.0.0.0:7000".parse().unwrap());
        assert_eq!(config.public_url, "http://localhost");
    }

    #[test]
    fn defaults_apply_when_unset() {
        let config = load(None, &[URL]).unwrap();
        assert_eq!(config.bind, "127.0.0.1:8080".parse().unwrap());
        assert_eq!(config.database_path, PathBuf::from("notebook.db"));
        assert_eq!(config.static_dir, None);
        assert!(config.trusted_proxies.is_empty());
        assert_eq!(config.max_body_bytes.get(), 1_048_576);
        assert_eq!(config.request_timeout_secs.get(), 30);
        assert_eq!(config.rate_limit_per_second.get(), 20);
        assert_eq!(config.rate_limit_burst.get(), 100);
        assert_eq!(config.log_format, LogFormat::Text);
        assert_eq!(config.log_level, "info");
    }

    #[test]
    fn no_file_with_env_only() {
        let config = load(None, &[URL, ("NOTEBOOK_LOG_FORMAT", "json")]).unwrap();
        assert!(config.is_https());
        assert_eq!(config.log_format, LogFormat::Json);
    }

    #[test]
    fn missing_public_url_names_it() {
        let err = load(None, &[]).unwrap_err().to_string();
        assert!(err.contains("public_url"), "{err}");
        assert!(err.contains("NOTEBOOK_PUBLIC_URL"), "{err}");
    }

    #[test]
    fn unknown_file_key_names_it() {
        let file = "public_url = \"http://localhost\"\nmax_body_byts = 5\n";
        let err = load(Some(file), &[]).unwrap_err().to_string();
        assert!(err.contains("max_body_byts"), "{err}");
    }

    #[test]
    fn unparseable_env_value_names_it() {
        let err = load(None, &[URL, ("NOTEBOOK_MAX_BODY_BYTES", "lots")])
            .unwrap_err()
            .to_string();
        assert!(err.contains("NOTEBOOK_MAX_BODY_BYTES"), "{err}");
    }

    #[test]
    fn bad_public_url_scheme_rejected() {
        let err = load(None, &[("NOTEBOOK_PUBLIC_URL", "ftp://x")])
            .unwrap_err()
            .to_string();
        assert!(err.contains("public_url"), "{err}");
    }

    #[test]
    fn trusted_proxies_from_comma_separated_env() {
        let config = load(
            None,
            &[
                URL,
                ("NOTEBOOK_TRUSTED_PROXIES", "127.0.0.1, ::1,,10.0.0.2"),
            ],
        )
        .unwrap();
        let expected: Vec<IpAddr> = vec![
            "127.0.0.1".parse().unwrap(),
            "::1".parse().unwrap(),
            "10.0.0.2".parse().unwrap(),
        ];
        assert_eq!(config.trusted_proxies, expected);

        let err = load(None, &[URL, ("NOTEBOOK_TRUSTED_PROXIES", "127.0.0.1,nope")])
            .unwrap_err()
            .to_string();
        assert!(err.contains("NOTEBOOK_TRUSTED_PROXIES"), "{err}");
    }

    #[test]
    fn empty_paths_rejected() {
        for (var, key) in [
            ("NOTEBOOK_DATABASE_PATH", "database_path"),
            ("NOTEBOOK_STATIC_DIR", "static_dir"),
        ] {
            let err = load(None, &[URL, (var, "")]).unwrap_err().to_string();
            assert!(err.contains(key), "{err}");
            let file = format!("public_url = \"http://localhost\"\n{key} = \"\"\n");
            let err = load(Some(&file), &[]).unwrap_err().to_string();
            assert!(err.contains(key), "{err}");
        }
    }

    #[test]
    fn mapped_trusted_proxies_are_canonicalized() {
        let config = load(
            None,
            &[URL, ("NOTEBOOK_TRUSTED_PROXIES", "::ffff:127.0.0.1")],
        )
        .unwrap();
        assert_eq!(config.trusted_proxies, vec![IpAddr::from([127, 0, 0, 1])]);
    }

    #[test]
    fn trusted_proxies_from_file_list() {
        let file = "public_url = \"http://localhost\"\ntrusted_proxies = [\"127.0.0.1\"]\n";
        let config = load(Some(file), &[]).unwrap();
        assert_eq!(config.trusted_proxies, vec![IpAddr::from([127, 0, 0, 1])]);
    }
}
