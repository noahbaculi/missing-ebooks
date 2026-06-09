//! Router configuration, read from environment variables with defaults that
//! match the demo spec (20 minute idle, 50 sandboxes, 2 per IP, mixed-forest).

use std::time::Duration;

/// Every knob the router reads at startup. Values come from the environment in
/// `load_from_env`; this struct carries already-parsed, validated values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Where the router's own HTTP listener binds.
    pub bind: String,
    /// Inclusive low and high ends of the loopback port range for sandboxes.
    pub port_low: u16,
    pub port_high: u16,
    /// Hard ceiling on live sandboxes across all visitors.
    pub max_sandboxes: usize,
    /// Ceiling on concurrent sandboxes for one client IP.
    pub max_per_ip: usize,
    /// How long a sandbox may sit idle before the reaper kills it.
    pub idle: Duration,
    /// How long to wait for a freshly spawned sandbox to answer before giving up.
    pub ready_timeout: Duration,
    /// Ceiling on a single proxied request to a sandbox, so a wedged sandbox
    /// surfaces as a 502 instead of hanging the visitor's request forever.
    pub forward_timeout: Duration,
    /// Ceiling on a buffered sandbox response. The banner splice needs the whole
    /// HTML page in memory, so the body is buffered; this bounds how large that
    /// buffer can grow before the response is refused.
    pub max_response_bytes: usize,
    /// The scenario name passed to `explore`.
    pub scenario: String,
    /// Path to the compiled `explore` binary.
    pub explore_bin: String,
    /// Name of the session cookie.
    pub cookie_name: String,
}

/// Read one variable, falling back to `default` when it is unset or empty.
fn var_or(name: &str, default: &str) -> String {
    match std::env::var(name) {
        Ok(value) if !value.trim().is_empty() => value,
        _ => default.to_string(),
    }
}

/// Parse a `"low-high"` port range such as `"9000-9100"`.
pub fn parse_port_range(value: &str) -> anyhow::Result<(u16, u16)> {
    let (low, high) = value
        .split_once('-')
        .ok_or_else(|| anyhow::anyhow!("port range {value:?} is not in low-high form"))?;
    let low: u16 = low.trim().parse()?;
    let high: u16 = high.trim().parse()?;
    if low >= high {
        anyhow::bail!("port range low {low} must be below high {high}");
    }
    Ok((low, high))
}

impl Config {
    /// Build a config from the environment, applying spec defaults for anything
    /// unset.
    pub fn load_from_env() -> anyhow::Result<Self> {
        let (port_low, port_high) =
            parse_port_range(&var_or("ROUTER_PORT_RANGE", "9000-9100"))?;
        Ok(Self {
            bind: var_or("ROUTER_BIND", "127.0.0.1:8080"),
            port_low,
            port_high,
            max_sandboxes: var_or("ROUTER_MAX_SANDBOXES", "50").parse()?,
            max_per_ip: var_or("ROUTER_MAX_PER_IP", "2").parse()?,
            idle: Duration::from_secs(var_or("ROUTER_IDLE_SECS", "1200").parse()?),
            ready_timeout: Duration::from_secs(var_or("ROUTER_READY_TIMEOUT_SECS", "10").parse()?),
            forward_timeout: Duration::from_secs(
                var_or("ROUTER_FORWARD_TIMEOUT_SECS", "30").parse()?,
            ),
            max_response_bytes: var_or("ROUTER_MAX_RESPONSE_BYTES", "16777216").parse()?,
            scenario: var_or("ROUTER_SCENARIO", "mixed-forest"),
            explore_bin: var_or("ROUTER_EXPLORE_BIN", "/usr/local/bin/explore"),
            cookie_name: var_or("ROUTER_COOKIE_NAME", "me_demo_sid"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_valid_port_range() {
        assert_eq!(parse_port_range("9000-9100").unwrap(), (9000, 9100));
    }

    #[test]
    fn rejects_a_backwards_or_malformed_range() {
        assert!(parse_port_range("9100-9000").is_err());
        assert!(parse_port_range("9000").is_err());
        assert!(parse_port_range("a-b").is_err());
    }
}
