//! eBPF-backed sandbox observability and enforcement.

mod session;

pub use session::{EbpfAuditSession, EbpfAuditSessionConfig};
