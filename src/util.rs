use chrono::{DateTime, Local, NaiveDateTime};
use std::collections::HashSet;

/// Python datetime.now().isoformat() — local time, microseconds, no timezone.
pub fn now_iso() -> String {
    Local::now().format("%Y-%m-%dT%H:%M:%S%.6f").to_string()
}

/// Parse an ISO timestamp the way datetime.fromisoformat does (date or datetime,
/// optional fractional seconds, optional timezone offset). Returns local-naive DateTime.
pub fn parse_iso(s: &str) -> Option<DateTime<Local>> {
    let s = s.trim();
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Some(dt.with_timezone(&Local));
    }
    // fromisoformat accepts 'YYYY-MM-DD' and 'YYYY-MM-DDTHH:MM:SS[.ffffff]'
    let formats = [
        "%Y-%m-%dT%H:%M:%S%.f",
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%dT%H:%M",
        "%Y-%m-%d",
    ];
    for f in formats {
        if let Ok(nd) = NaiveDateTime::parse_from_str(s, f) {
            return nd.and_local_timezone(Local).single();
        }
    }
    // date-only produces NaiveDate; handle via NaiveDateTime parse failure above
    if let Ok(nd) = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        return nd.and_hms_opt(0, 0, 0)?.and_local_timezone(Local).single();
    }
    None
}

/// Weight applied to stale facts in search/routing (#3 quality lever).
/// Default 1.0: stale facts stay fully visible and rank by relevance — "stale" is a
/// freshness flag, not a correctness judgment, and ~72% of the base was stale and
/// hidden. Search still favors fresh facts via its recency term. Set <1 to down-weight
/// stale (e.g. 0.5), or 0.0 to restore the previous hard-exclude behavior.
pub fn stale_weight() -> f64 {
    std::env::var("RLM_STALE_WEIGHT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1.0)
}

/// Python str slicing by characters (not bytes).
pub fn char_slice(s: &str, start: usize, len: usize) -> String {
    s.chars().skip(start).take(len).collect()
}

pub fn char_len(s: &str) -> usize {
    s.chars().count()
}

/// Truncate like Python content[:100] + "..." if longer.
pub fn truncate(s: &str, n: usize) -> String {
    if char_len(s) > n {
        format!("{}...", char_slice(s, 0, n))
    } else {
        s.to_string()
    }
}

/// Noise filter from #33/#23.
pub fn is_noise_content(content: &str) -> bool {
    let c = content.trim();
    c.starts_with("__FINGERPRINT__:") || c.contains("is a Unknown project") || c == "Unknown project"
}

/// Cosine similarity in f64 (matches numpy float64 math of the Python server).
pub fn cosine(a: &[f32], b: &[f32]) -> f64 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let mut dot = 0f64;
    let mut na = 0f64;
    let mut nb = 0f64;
    for i in 0..a.len() {
        let x = a[i] as f64;
        let y = b[i] as f64;
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    if na <= 0.0 || nb <= 0.0 {
        return 0.0;
    }
    dot / (na.sqrt() * nb.sqrt())
}

/// Jaccard over whitespace-split lowercased tokens (Python set(a.lower().split())).
pub fn jaccard(a: &str, b: &str) -> f64 {
    let ta: HashSet<&str> = a.split_whitespace().collect();
    let tb: HashSet<&str> = b.split_whitespace().collect();
    let inter = ta.intersection(&tb).count();
    let union = ta.union(&tb).count();
    if union == 0 {
        0.0
    } else {
        inter as f64 / union as f64
    }
}

/// FTS query tokens: re.findall(r"[0-9a-zA-Z_\-.]{2,}", query.lower())[:10]
pub fn query_tokens(query: &str) -> Vec<String> {
    let re = regex::Regex::new(r"[0-9a-zA-Z_\-.]{2,}").unwrap();
    re.find_iter(&query.to_lowercase())
        .take(10)
        .map(|m| m.as_str().to_string())
        .collect()
}

pub fn level_name(level: i64) -> &'static str {
    match level {
        0 => "L0_PROJECT",
        1 => "L1_DOMAIN",
        2 => "L2_MODULE",
        3 => "L3_CODE",
        _ => "L0_PROJECT",
    }
}

/// Python str.title() for single-word type names ("nodejs" -> "Nodejs").
pub fn title_case(s: &str) -> String {
    let mut out = String::new();
    let mut prev_alpha = false;
    for c in s.chars() {
        if c.is_alphabetic() {
            if prev_alpha {
                out.extend(c.to_lowercase());
            } else {
                out.extend(c.to_uppercase());
            }
            prev_alpha = true;
        } else {
            out.push(c);
            prev_alpha = false;
        }
    }
    out
}

/// Python f"{n:,}" thousands separator.
pub fn thousands(n: i64) -> String {
    let s = n.abs().to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    if n < 0 {
        format!("-{}", out)
    } else {
        out
    }
}

/// Python round(x, n) — banker's rounding on the decimal representation.
pub fn py_round(x: f64, digits: u32) -> f64 {
    let factor = 10f64.powi(digits as i32);
    let scaled = x * factor;
    let r = scaled.round_ties_even();
    r / factor
}

/// Fact line freshness prefix date: created_at[:10]
pub fn fact_date(created_at: &str) -> String {
    char_slice(created_at, 0, 10)
}

pub fn date_prefix_re() -> regex::Regex {
    regex::Regex::new(r"^\s*\[\d{4}-\d{2}-\d{2}\]").unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_char_slice() {
        assert_eq!(char_slice("привет мир", 0, 6), "привет");
        assert_eq!(truncate("abcdef", 3), "abc...");
        assert_eq!(truncate("abc", 3), "abc");
    }

    #[test]
    fn test_title() {
        assert_eq!(title_case("unknown"), "Unknown");
        assert_eq!(title_case("nodejs"), "Nodejs");
        assert_eq!(title_case("python"), "Python");
    }

    #[test]
    fn test_thousands() {
        assert_eq!(thousands(1234567), "1,234,567");
        assert_eq!(thousands(500), "500");
    }

    #[test]
    fn test_jaccard() {
        assert!((jaccard("a b c", "a b d") - 0.5).abs() < 1e-9);
    }
}