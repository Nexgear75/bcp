#![allow(dead_code)]

pub mod config;
pub mod error;
pub mod types;
pub mod copy;
pub mod atomic;
pub mod progress;
pub mod signals;
pub mod validate;

pub use config::Config;
pub use error::CopyError;

pub fn run(config: Config) -> anyhow::Result<()> {
    // TODO: Full implementation in Task 3
    let _ = config;
    Ok(())
}
