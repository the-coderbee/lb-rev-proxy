//! Settings for running the proxy.
use std::time::Duration;

/// How long to wait for a backend connection before giving up.
///
/// Connecting within the same datacenter normally takes well under 1 ms,
/// so 300 ms leaves ample headroom while still failing fast when a backend
/// silently drops packets.
pub const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_millis(300);

/// Settings that control the proxy's behaviour, fixed at startup.
///
/// Use `ProxyConfig::default()` and override individual fields as needed.
#[derive(Debug, Clone, Copy)]
pub struct ProxyConfig {
    /// How long to wait for a connection to a backend before trying the
    /// next one. Defaults to [`DEFAULT_CONNECT_TIMEOUT`].
    pub connect_timeout: Duration,
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self {
            connect_timeout: DEFAULT_CONNECT_TIMEOUT,
        }
    }
}
