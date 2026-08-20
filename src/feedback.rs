//! Bug-report URL construction. Privacy by design: the prefilled issue
//! contains ONLY the app version and platform — no paths, identifiers, or
//! anything from the user's machine — and the report is submitted by the
//! user in their browser, never by the app itself.

/// Percent-encoding for URL query components (RFC 3986 unreserved set kept).
pub fn percent_encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

pub const REPO_URL: &str = "https://github.com/gregnazario/worktree-ui";

fn platform_label() -> &'static str {
    match std::env::consts::OS {
        "macos" => "macOS",
        "windows" => "Windows",
        "freebsd" => "FreeBSD",
        _ => "Linux",
    }
}

pub fn app_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Body template the user completes in the browser. Deliberately minimal.
pub fn issue_body() -> String {
    format!(
        "### What happened\n\n\n### Steps to reproduce\n\n1. \n2. \n\n### Expected\n\n\n---\nworktree-tool {} on {} ({})",
        app_version(),
        platform_label(),
        std::env::consts::ARCH
    )
}

/// GitHub's prefilled new-issue URL.
pub fn report_bug_url() -> String {
    format!(
        "{REPO_URL}/issues/new?title={}&body={}",
        percent_encode("Bug: "),
        percent_encode(&issue_body())
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_encoding_keeps_unreserved_and_encodes_the_rest() {
        assert_eq!(percent_encode("abcXYZ09-_.~"), "abcXYZ09-_.~");
        assert_eq!(percent_encode("a b"), "a%20b");
        assert_eq!(percent_encode("a/b?d&e=f"), "a%2Fb%3Fd%26e%3Df");
        assert_eq!(percent_encode("héllo"), "h%C3%A9llo");
        assert_eq!(percent_encode(""), "");
    }

    #[test]
    fn report_url_targets_repo_new_issue_with_prefill() {
        let url = report_bug_url();
        assert!(url.starts_with("https://github.com/gregnazario/worktree-ui/issues/new?"));
        assert!(url.contains("title=Bug%3A%20"));
        assert!(url.contains(&percent_encode(&format!(
            "worktree-tool {} on",
            app_version()
        ))));
        // round-trip: encoded body must not contain raw characters that
        // would break the query string
        assert!(!url.contains(' ') && !url.contains('\n'));
    }

    #[test]
    fn issue_body_carries_version_and_platform_only() {
        let body = issue_body();
        assert!(body.contains(app_version()));
        assert!(body.contains(platform_label()));
        assert!(body.contains(std::env::consts::ARCH));
        assert!(body.contains("### What happened"));
    }
}
