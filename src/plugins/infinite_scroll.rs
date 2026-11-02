//! Infinite scroll plugin, ported from upstream's
//! 'searx/plugins/infinite_scroll.py'.
//!
//! Upstream's plugin is a pure UI toggle: the client script of the result page
//! loads the next page when the user scrolls to the bottom.  The Rust UI pages
//! with 'pageno' links instead and has no such script, so this plugin has no
//! implementation - it only exists so that the plugin shows up in the
//! preferences page and in '/config', exactly like upstream.
//!
//! Note: upstream uses the id 'infiniteScroll'; this port uses the snake case
//! form 'infinite_scroll' (the id requested for this crate).
//!
//! The Lead declares 'pub mod infinite_scroll;' in 'src/plugins/mod.rs' and adds
//! 'Box::new(InfiniteScroll)' to 'src/plugins/builtin.rs' ('all()').

use super::{Plugin, PluginContext};
use crate::aggregator::SearchResponse;

/// The upstream plugin id (kept for documentation / preferences migration).
pub const UPSTREAM_ID: &str = "infiniteScroll";

/// The plugin (upstream 'infiniteScroll').
pub struct InfiniteScroll;

impl Plugin for InfiniteScroll {
    fn id(&self) -> &'static str {
        "infinite_scroll"
    }
    fn name(&self) -> &'static str {
        "Infinite scroll"
    }
    fn description(&self) -> &'static str {
        "Automatically loads the next page when scrolling to bottom of the current page"
    }
    fn default_enabled(&self) -> bool {
        false
    }
    fn pre_search(&self, _query: &mut crate::query::Query, _context: &PluginContext) {
        // No-op: the UI setting is handled by the result page.
    }
    fn post_search(&self, _response: &mut SearchResponse, _context: &PluginContext) {
        // No-op: the UI setting is handled by the result page.
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aggregator::SearchResponse;
    use crate::engine::EngineRegistry;
    use crate::query::Query;
    use crate::settings::Settings;

    fn context() -> PluginContext {
        PluginContext {
            base_url: "http://localhost:8888".into(),
            ..Default::default()
        }
    }

    #[test]
    fn plugin_metadata_matches_upstream() {
        let plugin = InfiniteScroll;
        assert_eq!(plugin.id(), "infinite_scroll");
        assert_eq!(UPSTREAM_ID, "infiniteScroll");
        assert_eq!(plugin.name(), "Infinite scroll");
        assert_eq!(
            plugin.description(),
            "Automatically loads the next page when scrolling to bottom of the current page"
        );
        // The plugin is offered on the preferences page but off by default.
        assert!(!plugin.default_enabled());
    }

    #[test]
    fn the_plugin_never_touches_the_results() {
        let registry = EngineRegistry::from_settings(&Settings::defaults()).unwrap();
        let mut response = SearchResponse::new(&Query::parse("rust", &registry, &[]));
        response.answers.push("kept".to_string());
        let mut query = Query::parse("rust", &registry, &[]);
        InfiniteScroll.pre_search(&mut query, &context());
        InfiniteScroll.post_search(&mut response, &context());
        assert_eq!(response.answers, vec!["kept".to_string()]);
        assert_eq!(response.results.len(), 0);
    }
}
