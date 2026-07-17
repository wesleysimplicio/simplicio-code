//! pkg-c/nested/deep/module.rs
//! Deliberately nested several directories deep to exercise monorepo-style path resolution.
pub struct DeepConfig {
    pub enabled: bool,
    pub retries: u32,
}

impl Default for DeepConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            retries: 3,
        }
    }
}
