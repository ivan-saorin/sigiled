pub mod activity;
#[path = "../../shared/bounded_process.rs"]
pub mod bounded_process;
pub mod durable;
pub mod handoff;
pub mod process;
pub mod profile;

#[cfg(feature = "test-support")]
extern crate self as sigil_ide_agent;
#[cfg(feature = "test-support")]
#[allow(dead_code)]
#[path = "main.rs"]
pub mod test_support;
