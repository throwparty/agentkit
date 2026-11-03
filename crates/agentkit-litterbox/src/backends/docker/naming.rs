//! Docker's container names.

pub fn container_name_for_slug(repo_prefix: &str, slug: &str) -> String {
    format!("litterbox-{}-{}", repo_prefix, slug)
}
