use anyhow::{anyhow, Result};

pub fn format_bps_percent(bps: u16) -> String {
    let pct = (bps as f64) / 100.0;
    format!("{pct:.2}%")
}

pub fn parse_percent_to_bps(raw: &str, field: &str) -> Result<u16> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(anyhow!("{field} must not be empty"));
    }
    let numeric = trimmed.strip_suffix('%').unwrap_or(trimmed).trim();
    let pct = numeric
        .parse::<f64>()
        .map_err(|_| anyhow!("{field} must be a percent like 12.5%"))?;
    if !pct.is_finite() {
        return Err(anyhow!("{field} must be a finite number"));
    }
    if pct < 0.0 {
        return Err(anyhow!("{field} must be >= 0"));
    }
    let bps = pct * 100.0;
    if bps > u16::MAX as f64 {
        return Err(anyhow!("{field} is too large"));
    }
    Ok(bps.round() as u16)
}
