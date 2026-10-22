//! Log subscriber setup: an `EnvFilter` directive and text or JSON lines.

use std::io::IsTerminal;

use tracing::Subscriber;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt::MakeWriter;

use crate::config::LogFormat;

/// Builds a subscriber writing to `writer`, with ANSI colours in text mode only
/// when `ansi` is set. An invalid `level` falls back to `info`;
/// [`crate::config::Config`] has already validated it.
pub fn subscriber<W>(
    level: &str,
    format: LogFormat,
    ansi: bool,
    writer: W,
) -> Box<dyn Subscriber + Send + Sync>
where
    W: for<'w> MakeWriter<'w> + Send + Sync + 'static,
{
    // tungstenite traces whole WebSocket frames (access tokens, payloads)
    // through `log`. Nothing bridges `log` today; this keeps it off if
    // something ever does.
    let filter = EnvFilter::try_new(level)
        .unwrap_or_else(|_| EnvFilter::new("info"))
        .add_directive("tungstenite=off".parse().expect("valid directive"));
    let builder = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_ansi(ansi)
        .with_writer(writer);
    match format {
        LogFormat::Text => Box::new(builder.finish()),
        LogFormat::Json => Box::new(builder.json().finish()),
    }
}

/// Installs the process-wide subscriber, writing to stdout, coloured only
/// when stdout is a terminal.
pub fn init(level: &str, format: LogFormat) {
    let ansi = std::io::stdout().is_terminal();
    // Fails only if a subscriber is already set, which is harmless.
    let _ =
        tracing::subscriber::set_global_default(subscriber(level, format, ansi, std::io::stdout));
}

#[cfg(test)]
pub(crate) mod test_writer {
    use std::io;
    use std::sync::{Arc, Mutex};

    use tracing_subscriber::fmt::MakeWriter;

    /// An in-memory log sink for asserting on output.
    #[derive(Clone, Default)]
    pub struct Captured(Arc<Mutex<Vec<u8>>>);

    impl Captured {
        pub fn contents(&self) -> String {
            String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
        }
    }

    impl io::Write for Captured {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl<'a> MakeWriter<'a> for Captured {
        type Writer = Captured;

        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_writer::Captured;
    use super::*;

    #[test]
    fn json_mode_emits_one_object_per_line() {
        let out = Captured::default();
        let sub = subscriber("info", LogFormat::Json, false, out.clone());
        tracing::subscriber::with_default(sub, || {
            tracing::info!(answer = 42, "first");
            tracing::warn!("second\nwith a newline");
            tracing::debug!("filtered out");
        });
        let text = out.contents();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2, "{text}");
        for line in lines {
            let value: serde_json::Value = serde_json::from_str(line).unwrap();
            assert!(value.is_object(), "{line}");
        }
    }
}
