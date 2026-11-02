//! Search categories, ported from SearXNG's `settings_defaults.CATEGORIES_AS_TABS`
//! and the category names used by the engines.
//!
//! Upstream keeps categories as plain strings; we do the same (with named
//! constants) so that user supplied category names keep working.

pub const GENERAL: &str = "general";
pub const IMAGES: &str = "images";
pub const VIDEOS: &str = "videos";
pub const NEWS: &str = "news";
pub const MAP: &str = "map";
pub const MUSIC: &str = "music";
pub const IT: &str = "it";
pub const SCIENCE: &str = "science";
pub const FILES: &str = "files";
pub const SOCIAL_MEDIA: &str = "social media";

/// Extra categories that are not shown as tabs but are used by engines.
pub const REPOSITORIES: &str = "repositories";
pub const PACKAGES: &str = "packages";
pub const BLOGS: &str = "blogs";
pub const APPS: &str = "apps";
pub const QA: &str = "q&a";
pub const DICTIONARIES: &str = "dictionaries";
pub const SHOPPING: &str = "shopping";
pub const TRANSLATE: &str = "translate";
pub const SCIENTIFIC_PUBLICATIONS: &str = "scientific publications";
pub const CARGO: &str = "cargo";
pub const CODE: &str = "code";
pub const IMAGES_AND_VIDEOS: &str = "images and videos";

/// Categories rendered as tabs in the UI (SearXNG `categories_as_tabs`).
pub static TABS: &[&str] = &[
    GENERAL,
    IMAGES,
    VIDEOS,
    NEWS,
    MAP,
    MUSIC,
    IT,
    SCIENCE,
    FILES,
    SOCIAL_MEDIA,
];

/// Normalise a category name the way SearXNG's bang parser does.
///
/// `!social-media`, `!social_media` and `!social media` are the same bang.
pub fn normalize(value: &str) -> String {
    value
        .trim()
        .to_lowercase()
        .replace(['-', '_'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Map any category onto the tab it belongs to, or `None` when it is not a tab.
pub fn tab_of(category: &str) -> Option<&'static str> {
    let normalized = normalize(category);
    TABS.iter()
        .copied()
        .find(|tab| normalize(tab) == normalized)
}
