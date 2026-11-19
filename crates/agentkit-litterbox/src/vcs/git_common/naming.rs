//! Branch naming shared by the Git backend and the jj bridge.

/// The branch a sandbox's snapshots live on.
pub fn branch_name_for(slug: &str) -> String {
    format!("litterbox/{}", slug)
}
