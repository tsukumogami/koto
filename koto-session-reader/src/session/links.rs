//! The reserved area of a session's context, as the template layer checks
//! it. Links themselves are koto's `session::links`, which writes them.

/// The reserved area of a session's context. Only koto writes keys here.
pub const RESERVED_PREFIX: &str = "koto/";

/// The first component of every reserved key, compared case-insensitively.
const RESERVED_COMPONENT: &str = "koto";

/// Whether `key` lies in the reserved area: its first component is `koto`,
/// compared case-insensitively so `KOTO/links/...` can't reach the same
/// directory on a case-insensitive filesystem. The bare key `koto` counts
/// too, since a file of that name would block the reserved directory.
pub fn is_reserved_key(key: &str) -> bool {
    let first = key.split('/').next().unwrap_or("");
    first.eq_ignore_ascii_case(RESERVED_COMPONENT)
}
