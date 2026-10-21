//! Skill registry and audit.
//!
//! [`SkillRegistry`] (trait) + [`FileSkillRegistry`] (filesystem
//! implementation) + [`audit_skill_directory`] / [`SkillAuditOptions`]
//! (directory audit) form the runtime's skill injection surface.

#[doc(inline)]
pub use agent_contracts::SkillRegistry;

#[doc(inline)]
pub use xiaoo_core::EmptySkillRegistry;

#[doc(inline)]
pub use skill::{
    audit::{audit_skill_directory, SkillAuditOptions},
    FileSkillRegistry,
};
