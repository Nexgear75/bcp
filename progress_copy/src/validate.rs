//! Option validation shared by `bcp` and `bmv`.

use crate::config::Config;

/// Reject option values that can't work.
///
/// # Errors
/// Zero threads, zero buffer size, empty backup suffix.
pub fn validate_flags(config: &Config) -> anyhow::Result<()> {
    if config.threads == Some(0) {
        anyhow::bail!("--threads must be at least 1");
    }
    if config.buffer_size == 0 {
        anyhow::bail!("--buffer-size must be at least 1 byte");
    }
    if config.backup_suffix.as_deref() == Some("") {
        anyhow::bail!("--suffix must not be empty");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_bad_values() {
        assert!(validate_flags(&Config::default()).is_ok());
        assert!(validate_flags(&Config { threads: Some(0), ..Default::default() }).is_err());
        assert!(validate_flags(&Config { buffer_size: 0, ..Default::default() }).is_err());
        assert!(validate_flags(&Config { backup_suffix: Some(String::new()), ..Default::default() }).is_err());
    }
}
