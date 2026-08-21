//! Network policy enforcement subsystem.
//!
//! Provides fine-grained network access control for sandboxed processes using
//! eBPF-based event monitoring and policy matching.

pub mod cidr;
pub mod enforcer;
pub mod matcher;
pub mod resolver;

pub use cidr::{Cidr, CidrError};
pub use enforcer::{EnforceResult, NetworkEnforcer};
pub use matcher::{MatchResult, NetworkPolicyMatcher};
pub use resolver::{DnsResolver, ResolveError};
