//! CalDAV 资源同步；旧 provider 的只读抓取仍留在 calendar.rs。
pub mod dav;
pub mod ical;
pub(crate) mod projection;
#[cfg(test)]
mod projection_tests;
pub mod sync;
