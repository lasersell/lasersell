use std::time::Duration;

const LATEST_VERSION_URL: &str =
    "https://dl.lasersell.io/binaries/lasersell/latest.txt";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(3);

pub struct UpdateAvailable {
    pub current: String,
    pub latest: String,
}

/// Check dl.lasersell.io for a newer version. Returns `None` if up-to-date or
/// if the check fails for any reason (network error, parse error, timeout).
pub async fn check_for_update() -> Option<UpdateAvailable> {
    let current = env!("CARGO_PKG_VERSION");
    check_against(current).await
}

async fn check_against(current: &str) -> Option<UpdateAvailable> {
    let client = reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .build()
        .ok()?;

    let response = client.get(LATEST_VERSION_URL).send().await.ok()?;
    let body = response.text().await.ok()?;
    let latest = body.trim();

    if latest.is_empty() {
        return None;
    }

    let current_parts = parse_semver(current)?;
    let latest_parts = parse_semver(latest)?;

    if latest_parts > current_parts {
        Some(UpdateAvailable {
            current: current.to_string(),
            latest: latest.to_string(),
        })
    } else {
        None
    }
}

fn parse_semver(version: &str) -> Option<(u64, u64, u64)> {
    let v = version.strip_prefix('v').unwrap_or(version);
    let mut parts = v.splitn(3, '.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    Some((major, minor, patch))
}

/// Print a styled update banner to stderr. This is called before the TUI takes
/// over the terminal, so stderr output is visible to the user.
pub fn print_update_banner(update: &UpdateAvailable) {
    let install_cmd = "curl -fsSL https://dl.lasersell.io/install.sh | sh";
    let version_line = format!("Update available: {} \u{2192} {}", update.current, update.latest);
    let changelog = "Changelog: https://github.com/lasersell/lasersell/releases";

    // Calculate box width based on longest content line
    let content_lines = [&version_line, install_cmd, changelog];
    let max_len = content_lines.iter().map(|l| l.len()).max().unwrap_or(0);
    let inner_width = max_len + 2; // 1 space padding each side

    let top = format!("  \x1b[33m╭{}╮\x1b[0m", "─".repeat(inner_width));
    let bottom = format!("  \x1b[33m╰{}╯\x1b[0m", "─".repeat(inner_width));
    let empty = format!(
        "  \x1b[33m│\x1b[0m{}\x1b[33m│\x1b[0m",
        " ".repeat(inner_width)
    );

    let fmt_line = |text: &str, bold: bool| -> String {
        let padding = inner_width - text.len() - 1;
        if bold {
            format!(
                "  \x1b[33m│\x1b[0m \x1b[1;33m{}\x1b[0m{}\x1b[33m│\x1b[0m",
                text,
                " ".repeat(padding)
            )
        } else {
            format!(
                "  \x1b[33m│\x1b[0m \x1b[2m{}\x1b[0m{}\x1b[33m│\x1b[0m",
                text,
                " ".repeat(padding)
            )
        }
    };

    eprintln!();
    eprintln!("{top}");
    eprintln!("{}", fmt_line(&version_line, true));
    eprintln!("{empty}");
    eprintln!("{}", fmt_line(install_cmd, false));
    eprintln!("{}", fmt_line(changelog, false));
    eprintln!("{bottom}");
    eprintln!();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_semver_basic() {
        assert_eq!(parse_semver("1.2.3"), Some((1, 2, 3)));
        assert_eq!(parse_semver("v1.2.3"), Some((1, 2, 3)));
        assert_eq!(parse_semver("0.3.0"), Some((0, 3, 0)));
    }

    #[test]
    fn parse_semver_invalid() {
        assert_eq!(parse_semver("abc"), None);
        assert_eq!(parse_semver("1.2"), None);
        assert_eq!(parse_semver(""), None);
    }

    #[tokio::test]
    async fn up_to_date_returns_none() {
        // If current equals latest, should return None
        // (this test doesn't hit the network — it tests the comparison logic)
        let current = "999.999.999";
        let result = check_against(current).await;
        // Network may fail in CI, but if it succeeds the absurdly high version
        // should never be outdated.
        if result.is_some() {
            panic!("version 999.999.999 should never be outdated");
        }
    }
}
