use std::net::{Ipv4Addr, Ipv6Addr};

use url::{Host, Url};

use crate::decider::GlobalMode;

/// Keys allowed in project config (.koto/config.toml).
/// Credential keys are intentionally excluded to prevent secrets from being
/// committed to version control.
const PROJECT_ALLOWLIST: &[&str] = &[
    "session.backend",
    "session.cloud.endpoint",
    "session.cloud.bucket",
    "session.cloud.region",
    "decider.mode",
];

/// Validate that a key is allowed in project config.
/// Returns an error message if the key is blocked.
pub fn validate_project_key(key: &str) -> Result<(), String> {
    if PROJECT_ALLOWLIST.contains(&key) {
        Ok(())
    } else if key == "session.cloud.access_key"
        || key == "session.cloud.secret_key"
        || key == "decider.api_key"
    {
        Err(format!(
            "key '{}' contains credentials and cannot be stored in project config (use user config or env vars instead)",
            key
        ))
    } else if key == "decider.endpoint" {
        Err(format!(
            "key '{}' can only be set in user config (--user) or KOTO_DECIDER_ENDPOINT",
            key
        ))
    } else {
        Err(format!("key '{}' is not allowed in project config", key))
    }
}

// ---------------------------------------------------------------------------
// Decider endpoint rules
// ---------------------------------------------------------------------------

/// Whether a URL host is loopback, decided by literal only.
///
/// True for an IPv4 literal in 127.0.0.0/8, the IPv6 literal `::1`, and
/// the exact name `localhost`. No name is ever resolved, so a DNS record
/// can't make a remote host count as loopback. This is the single copy of
/// the rule: the plain-`http` exception and the transport's proxy bypass
/// both call it.
pub fn is_loopback_host(host: &Host<&str>) -> bool {
    match host {
        Host::Ipv4(ip) => ip.is_loopback(),
        Host::Ipv6(ip) => *ip == Ipv6Addr::LOCALHOST,
        Host::Domain(name) => *name == "localhost",
    }
}

/// [`is_loopback_host`] for a host string as written in a URL
/// (`127.0.0.1`, `[::1]`, `::1`, or `localhost`). Anything that isn't an
/// IP literal or exactly `localhost` is not loopback.
pub fn is_loopback_host_str(host: &str) -> bool {
    let unbracketed = host
        .strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(host);
    if let Ok(ip) = unbracketed.parse::<Ipv4Addr>() {
        return ip.is_loopback();
    }
    if let Ok(ip) = unbracketed.parse::<Ipv6Addr>() {
        return ip == Ipv6Addr::LOCALHOST;
    }
    host == "localhost"
}

/// Whether a parsed URL points at a loopback host (by literal only).
pub fn is_loopback_url(url: &Url) -> bool {
    url.host().map(|h| is_loopback_host(&h)).unwrap_or(false)
}

/// Describe an endpoint for a message using only its scheme, host, and
/// path. Userinfo, port, query, and fragment are never included.
pub fn describe_endpoint(url: &Url) -> String {
    format!(
        "{}://{}{}",
        url.scheme(),
        url.host_str().unwrap_or(""),
        url.path()
    )
}

/// Why a decider endpoint was refused. `Display` never echoes userinfo,
/// query, or fragment: at most scheme, host, and path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EndpointError {
    /// The value doesn't parse as an absolute URL (nothing is echoed).
    Unparseable,
    /// The URL carries a username or password.
    Userinfo,
    /// Plain `http` to a host that isn't a loopback literal.
    InsecureHttp { described: String },
    /// A scheme other than `https` or loopback `http`.
    Scheme { described: String },
}

impl std::fmt::Display for EndpointError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EndpointError::Unparseable => write!(f, "decider endpoint is not a valid URL"),
            EndpointError::Userinfo => write!(
                f,
                "decider endpoint must not embed credentials before '@' in the URL"
            ),
            EndpointError::InsecureHttp { described } => write!(
                f,
                "decider endpoint {} uses plain http; http is only allowed for 127.0.0.0/8, ::1, or localhost (use https)",
                described
            ),
            EndpointError::Scheme { described } => write!(
                f,
                "decider endpoint {} must use https",
                described
            ),
        }
    }
}

/// True when the authority section of a raw URL string contains `@`,
/// whether or not the URL parser keeps an empty user name.
fn raw_has_userinfo(raw: &str) -> bool {
    let after_scheme = match raw.find("://") {
        Some(i) => &raw[i + 3..],
        None => return false,
    };
    let end = after_scheme
        .find(['/', '?', '#'])
        .unwrap_or(after_scheme.len());
    after_scheme[..end].contains('@')
}

/// Apply the scheme, loopback, and userinfo rules to a decider endpoint.
///
/// Accepts `https` to any host, and plain `http` only when the host is a
/// loopback literal (see [`is_loopback_host`]). Rejects userinfo, other
/// schemes, and anything that doesn't parse. Performs no DNS lookup.
pub fn check_decider_endpoint(raw: &str) -> Result<Url, EndpointError> {
    let url = Url::parse(raw.trim()).map_err(|_| EndpointError::Unparseable)?;
    if !url.username().is_empty() || url.password().is_some() || raw_has_userinfo(raw) {
        return Err(EndpointError::Userinfo);
    }
    if url.host().is_none() {
        return Err(EndpointError::Unparseable);
    }
    match url.scheme() {
        "https" => Ok(url),
        "http" if is_loopback_url(&url) => Ok(url),
        "http" => Err(EndpointError::InsecureHttp {
            described: describe_endpoint(&url),
        }),
        _ => Err(EndpointError::Scheme {
            described: describe_endpoint(&url),
        }),
    }
}

/// Render a configured endpoint for `koto config get` / `list`.
///
/// A clean URL is shown as written. One carrying userinfo, a query, or a
/// fragment is shown as scheme, host, and path only, and one that doesn't
/// parse is shown as `<invalid>`, so a credential pasted into the URL
/// never reaches stdout.
pub fn display_decider_endpoint(raw: &str) -> String {
    match Url::parse(raw.trim()) {
        Ok(url) => {
            let dirty = !url.username().is_empty()
                || url.password().is_some()
                || raw_has_userinfo(raw)
                || url.query().is_some()
                || url.fragment().is_some();
            if dirty {
                describe_endpoint(&url)
            } else {
                raw.to_string()
            }
        }
        Err(_) => "<invalid>".to_string(),
    }
}

// ---------------------------------------------------------------------------
// `koto config set` value checks for decider keys
// ---------------------------------------------------------------------------

/// Upper bound on `decider.timeout_ms`.
pub const DECIDER_TIMEOUT_MAX_MS: u64 = 10_000;
/// Default `decider.timeout_ms`.
pub const DECIDER_TIMEOUT_DEFAULT_MS: u64 = 2_000;

/// Validate and normalize a `decider.mode` value for `koto config set`.
pub fn validate_decider_mode_value(value: &str) -> Result<&'static str, String> {
    if value.trim().eq_ignore_ascii_case("never") {
        return Err(
            "decider.mode 'never' is template-only; use off, shadow, or auto in config".to_string(),
        );
    }
    GlobalMode::parse(value)
        .map(|m| m.as_str())
        .ok_or_else(|| "decider.mode must be one of: off, shadow, auto".to_string())
}

/// Validate a `decider.timeout_ms` value for `koto config set`.
pub fn validate_decider_timeout_value(value: &str) -> Result<i64, String> {
    match value.trim().parse::<u64>() {
        Ok(n) if (1..=DECIDER_TIMEOUT_MAX_MS).contains(&n) => Ok(n as i64),
        _ => Err(format!(
            "decider.timeout_ms must be an integer from 1 to {}",
            DECIDER_TIMEOUT_MAX_MS
        )),
    }
}

/// Validate a `decider.endpoint` value for `koto config set`.
pub fn validate_decider_endpoint_value(value: &str) -> Result<(), String> {
    check_decider_endpoint(value)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Validate a `decider.api_key` value for `koto config set`. The error
/// never includes the value.
pub fn validate_decider_api_key_value(value: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        Err("decider.api_key must not be empty".to_string())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_allowed_keys() {
        assert!(validate_project_key("session.backend").is_ok());
        assert!(validate_project_key("session.cloud.endpoint").is_ok());
        assert!(validate_project_key("session.cloud.bucket").is_ok());
        assert!(validate_project_key("session.cloud.region").is_ok());
        assert!(validate_project_key("decider.mode").is_ok());
    }

    #[test]
    fn test_blocked_credential_keys() {
        let err = validate_project_key("session.cloud.access_key").unwrap_err();
        assert!(err.contains("credentials"));
        assert!(err.contains("cannot be stored in project config"));

        let err = validate_project_key("session.cloud.secret_key").unwrap_err();
        assert!(err.contains("credentials"));

        let err = validate_project_key("decider.api_key").unwrap_err();
        assert!(err.contains("credentials"));
        assert!(err.contains("cannot be stored in project config"));
    }

    #[test]
    fn test_decider_endpoint_and_timeout_blocked_in_project() {
        let err = validate_project_key("decider.endpoint").unwrap_err();
        assert!(err.contains("user config"), "{err}");
        assert!(err.contains("KOTO_DECIDER_ENDPOINT"), "{err}");

        let err = validate_project_key("decider.timeout_ms").unwrap_err();
        assert!(err.contains("not allowed in project config"), "{err}");
    }

    #[test]
    fn test_unknown_key_blocked() {
        let err = validate_project_key("some.random.key").unwrap_err();
        assert!(err.contains("not allowed in project config"));
    }

    #[test]
    fn loopback_by_literal_only() {
        for h in [
            "127.0.0.1",
            "127.1.2.3",
            "127.255.255.255",
            "::1",
            "[::1]",
            "localhost",
        ] {
            assert!(is_loopback_host_str(h), "{h} should be loopback");
        }
        for h in [
            "localhost.example.com",
            "localhost.",
            "LOCALHOST.evil",
            "10.0.0.1",
            "128.0.0.1",
            "0.0.0.0",
            "::",
            "[::2]",
            "example.com",
            "",
        ] {
            assert!(!is_loopback_host_str(h), "{h} should not be loopback");
        }
    }

    #[test]
    fn loopback_url_helper() {
        let u = Url::parse("http://127.0.0.1:8080/x").unwrap();
        assert!(is_loopback_url(&u));
        let u = Url::parse("http://[::1]:8080/x").unwrap();
        assert!(is_loopback_url(&u));
        let u = Url::parse("http://localhost:1/x").unwrap();
        assert!(is_loopback_url(&u));
        let u = Url::parse("http://localhost.example.com/x").unwrap();
        assert!(!is_loopback_url(&u));
    }

    #[test]
    fn endpoint_https_accepted() {
        assert!(check_decider_endpoint("https://api.example.com/v1/decide").is_ok());
    }

    #[test]
    fn endpoint_http_loopback_only() {
        for ok in [
            "http://127.0.0.1:9000/decide",
            "http://127.0.0.5/decide",
            "http://[::1]:9000/decide",
            "http://localhost:9000/decide",
        ] {
            assert!(check_decider_endpoint(ok).is_ok(), "{ok}");
        }
        for bad in [
            "http://example.com/decide",
            "http://localhost.example.com/decide",
            "http://10.0.0.1/decide",
        ] {
            assert!(
                matches!(
                    check_decider_endpoint(bad),
                    Err(EndpointError::InsecureHttp { .. })
                ),
                "{bad}"
            );
        }
    }

    #[test]
    fn endpoint_other_schemes_rejected() {
        for bad in [
            "ftp://example.com/x",
            "file:///etc/passwd",
            "ws://localhost/x",
        ] {
            assert!(check_decider_endpoint(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn endpoint_unparseable_rejected() {
        assert_eq!(
            check_decider_endpoint("not a url"),
            Err(EndpointError::Unparseable)
        );
        assert_eq!(check_decider_endpoint(""), Err(EndpointError::Unparseable));
    }

    #[test]
    fn endpoint_userinfo_rejected_without_echo() {
        for bad in [
            "https://user:pass@host.example/decide",
            "https://token@host.example/decide",
            "https://@host.example/decide",
            "http://user:pass@127.0.0.1/decide",
        ] {
            let err = check_decider_endpoint(bad).unwrap_err();
            assert_eq!(err, EndpointError::Userinfo, "{bad}");
            let msg = err.to_string();
            assert!(!msg.contains("user:"), "{msg}");
            assert!(!msg.contains("pass"), "{msg}");
            assert!(!msg.contains("token"), "{msg}");
        }
    }

    #[test]
    fn endpoint_messages_print_only_scheme_host_path() {
        let err =
            check_decider_endpoint("http://example.com:8443/decide?secret=q1#frag9").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("http://example.com/decide"), "{msg}");
        assert!(!msg.contains("secret"), "{msg}");
        assert!(!msg.contains("q1"), "{msg}");
        assert!(!msg.contains("frag9"), "{msg}");
        assert!(!msg.contains("8443"), "{msg}");
    }

    #[test]
    fn display_endpoint_strips_credentials() {
        assert_eq!(
            display_decider_endpoint("https://api.example.com/v1/decide"),
            "https://api.example.com/v1/decide"
        );
        let shown = display_decider_endpoint("https://user:pass@api.example.com/d?t=1");
        assert_eq!(shown, "https://api.example.com/d");
        assert_eq!(display_decider_endpoint("::nope::"), "<invalid>");
    }

    #[test]
    fn mode_value_checks() {
        assert_eq!(validate_decider_mode_value("off"), Ok("off"));
        assert_eq!(validate_decider_mode_value(" Shadow "), Ok("shadow"));
        assert_eq!(validate_decider_mode_value("auto"), Ok("auto"));
        let err = validate_decider_mode_value("never").unwrap_err();
        assert!(err.contains("template-only"), "{err}");
        assert!(validate_decider_mode_value("bogus").is_err());
    }

    #[test]
    fn timeout_value_checks() {
        assert_eq!(validate_decider_timeout_value("1"), Ok(1));
        assert_eq!(validate_decider_timeout_value("10000"), Ok(10_000));
        assert!(validate_decider_timeout_value("0").is_err());
        assert!(validate_decider_timeout_value("10001").is_err());
        assert!(validate_decider_timeout_value("-5").is_err());
        assert!(validate_decider_timeout_value("fast").is_err());
    }

    #[test]
    fn api_key_value_error_does_not_echo() {
        assert!(validate_decider_api_key_value("abc").is_ok());
        assert!(validate_decider_api_key_value("  ").is_err());
    }
}
