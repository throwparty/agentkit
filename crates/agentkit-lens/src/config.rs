use std::time::Duration;

use clap::Parser;

/// Configuration for the Lens MCP server, parsed from CLI arguments.
#[derive(Parser, Debug, Clone)]
#[command(name = "agentkit-lens", about = "Lens MCP server for web search and fetch")]
pub struct Config {
    /// Brave Search API key (required for search functionality).
    #[arg(long = "brave-search-api-key")]
    pub brave_search_api_key: Option<String>,

    /// Kagi Search API key (required for Kagi search functionality).
    #[arg(long = "kagi-search-api-key")]
    pub kagi_search_api_key: Option<String>,

    /// Cache TTL duration (optional, defaults to 1 hour).
    /// Supports human-readable format: 1s, 30m, 4h, 2d, 1w.
    #[arg(long = "cache-ttl", default_value = "1h")]
    pub cache_ttl: String,
}

/// Duration suffix multiplier.
enum DurationSuffix {
    Seconds(u64),
    Minutes(u64),
    Hours(u64),
    Days(u64),
    Weeks(u64),
}

impl Config {
    /// Parse the cache TTL string into a std::time::Duration.
    ///
    /// # Errors
    /// Returns an error if the format is invalid or the value overflows.
    pub fn parse_cache_ttl(&self) -> std::time::Duration {
        let s = &self.cache_ttl;

        if s.is_empty() {
            panic!("cache-ttl cannot be empty");
        }

        // Extract numeric part and suffix
        let (num_str, suffix) = match s.chars().last() {
            Some('w') => (&s[..s.len() - 1], DurationSuffix::Weeks(7 * 24 * 3600)),
            Some('d') => (&s[..s.len() - 1], DurationSuffix::Days(24 * 3600)),
            Some('h') => (&s[..s.len() - 1], DurationSuffix::Hours(3600)),
            Some('m') => (&s[..s.len() - 1], DurationSuffix::Minutes(60)),
            Some('s') => (&s[..s.len() - 1], DurationSuffix::Seconds(1)),
            _ => panic!("cache-ttl must end with a suffix (s/m/h/d/w)"),
        };

        let seconds: u64 = num_str
            .parse()
            .unwrap_or_else(|_| panic!("invalid cache-ttl value: {s}"));

        let multiplier = match suffix {
            DurationSuffix::Weeks(m) => m,
            DurationSuffix::Days(m) => m,
            DurationSuffix::Hours(m) => m,
            DurationSuffix::Minutes(m) => m,
            DurationSuffix::Seconds(m) => m,
        };

        let total = seconds
            .checked_mul(multiplier)
            .expect("cache-ttl overflow");

        // Duration::from_secs panics on overflow on some platforms, so check first
        if total > u64::MAX / 1_000_000_000 {
            panic!("cache-ttl exceeds maximum duration");
        }

        Duration::from_secs(total)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // =========================================================================
    // Unit tests: Duration parsing (N2)
    // =========================================================================

    #[test]
    #[should_panic(expected = "cannot be empty")]
    fn test_empty_ttl() {
        let config = Config {
            brave_search_api_key: None,
            kagi_search_api_key: None,
            cache_ttl: String::new(),
        };
        config.parse_cache_ttl();
    }

    #[test]
    #[should_panic(expected = "must end with a suffix")]
    fn test_no_suffix() {
        let config = Config {
            brave_search_api_key: None,
            kagi_search_api_key: None,
            cache_ttl: "100".to_string(),
        };
        config.parse_cache_ttl();
    }

    #[test]
    fn test_seconds() {
        let config = Config {
            brave_search_api_key: None,
            kagi_search_api_key: None,
            cache_ttl: "30s".to_string(),
        };
        assert_eq!(config.parse_cache_ttl().as_secs(), 30);
    }

    #[test]
    fn test_minutes() {
        let config = Config {
            brave_search_api_key: None,
            kagi_search_api_key: None,
            cache_ttl: "15m".to_string(),
        };
        assert_eq!(config.parse_cache_ttl().as_secs(), 15 * 60);
    }

    #[test]
    fn test_hours() {
        let config = Config {
            brave_search_api_key: None,
            kagi_search_api_key: None,
            cache_ttl: "2h".to_string(),
        };
        assert_eq!(config.parse_cache_ttl().as_secs(), 2 * 3600);
    }

    #[test]
    fn test_days() {
        let config = Config {
            brave_search_api_key: None,
            kagi_search_api_key: None,
            cache_ttl: "1d".to_string(),
        };
        assert_eq!(config.parse_cache_ttl().as_secs(), 24 * 3600);
    }

    #[test]
    fn test_weeks() {
        let config = Config {
            brave_search_api_key: None,
            kagi_search_api_key: None,
            cache_ttl: "2w".to_string(),
        };
        assert_eq!(config.parse_cache_ttl().as_secs(), 2 * 7 * 24 * 3600);
    }

    #[test]
    fn test_default_value() {
        let config = Config {
            brave_search_api_key: None,
            kagi_search_api_key: None,
            cache_ttl: "1h".to_string(),
        };
        assert_eq!(config.parse_cache_ttl().as_secs(), 3600);
    }

    #[test]
    #[should_panic(expected = "invalid cache-ttl value")]
    fn test_invalid_number() {
        let config = Config {
            brave_search_api_key: None,
            kagi_search_api_key: None,
            cache_ttl: "abcs".to_string(),
        };
        config.parse_cache_ttl();
    }

    #[test]
    fn test_single_digit() {
        let config = Config {
            brave_search_api_key: None,
            kagi_search_api_key: None,
            cache_ttl: "5m".to_string(),
        };
        assert_eq!(config.parse_cache_ttl().as_secs(), 5 * 60);
    }
}
