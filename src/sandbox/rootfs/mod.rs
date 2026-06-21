//! Rootfs lifecycle, split by concern:
//! - [`download`]: fetch, verify, and extract the Ubuntu base image (Linux).
//! - [`provision`]: build the provisioning script and configure the rootfs.
//! - [`cache`]: the monolithic and layered provision caches (Linux).
//! - [`claude_md`]: CLAUDE.md generation for the macOS/Lima backend.
//!
//! Each submodule's public API is re-exported here so callers use `rootfs::*`.

#[cfg(target_os = "linux")]
mod cache;
#[cfg(target_os = "macos")]
mod claude_md;
#[cfg(target_os = "linux")]
mod download;
mod provision;

#[cfg(target_os = "linux")]
pub use cache::*;
#[cfg(target_os = "macos")]
pub use claude_md::build_claude_md;
#[cfg(target_os = "linux")]
pub use download::*;
pub use provision::*;
