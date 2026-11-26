//! Field names of the Baidu OneAPI quota response.

/// Candidate JSON fields for `(used, limit, remaining)`, most specific first.
pub(crate) fn quota_fields() -> (
    &'static [&'static str],
    &'static [&'static str],
    &'static [&'static str],
) {
    (
        &["used_quota", "used", "usage"],
        &[
            "month_quota_limit",
            "quota_limit",
            "limit",
            "total",
            "quota",
        ],
        &["remaining_quota", "remaining", "available"],
    )
}

/// JSON pointers that hold the used amount of a Baidu OneAPI quota response.
pub(crate) fn usage_pointers() -> &'static [&'static str] {
    &["/data/used_quota"]
}
