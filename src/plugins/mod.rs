//! Plugin framework, ported from `searx/plugins/__init__.py`.
//!
//! Plugins can rewrite the query before the search, decorate single results
//! and add answers after the search. The bundled set is a subset of upstream's
//! (see `docs/PORTING.md`): `hostnames`, `tracker_url_remover`,
//! `self_info`, `time_zone`, `calculator`, `hash_plugin` and
//! `oa_doi_rewrite`.

pub mod ahmia_filter;
pub mod builtin;
pub mod infinite_scroll;
pub mod tor_check;
pub mod unit_converter;

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::aggregator::SearchResponse;
use crate::query::Query;
use crate::results::SearchResult;
use crate::settings::Settings;

/// Information about the requesting user (upstream `request`).
#[derive(Debug, Clone, Default)]
pub struct UserInfo {
    pub ip: String,
    pub user_agent: String,
    pub headers: BTreeMap<String, String>,
}

/// Context handed to the plugins.
#[derive(Debug, Clone, Default)]
pub struct PluginContext {
    /// The search terms, without the bangs.
    pub search_query: String,
    /// The UI locale.
    pub locale: String,
    /// The instance base URL.
    pub base_url: String,
    pub user: UserInfo,
}

/// Metadata shown on the preferences page.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginInfo {
    pub id: String,
    pub name: String,
    pub description: String,
    /// Whether the plugin is enabled unless the user opts out.
    pub default_enabled: bool,
}

/// The plugin interface.
pub trait Plugin: Send + Sync {
    fn id(&self) -> &'static str;
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str {
        ""
    }
    /// Enabled for everyone unless disabled in the preferences.
    fn default_enabled(&self) -> bool {
        true
    }
    /// Called once before the engines run.
    fn pre_search(&self, _query: &mut Query, _context: &PluginContext) {}
    /// Called for every merged result.
    fn on_result(&self, _result: &mut SearchResult, _context: &PluginContext) {}
    /// Called once after the search finished.
    fn post_search(&self, _response: &mut SearchResponse, _context: &PluginContext) {}
}

/// Holds every bundled plugin and answers which ones run for a request.
pub struct PluginManager {
    plugins: Vec<Box<dyn Plugin>>,
    enabled: BTreeSet<String>,
    disabled: BTreeSet<String>,
}

impl Default for PluginManager {
    fn default() -> Self {
        Self::new()
    }
}

impl PluginManager {
    /// All bundled plugins.
    pub fn new() -> Self {
        Self {
            plugins: builtin::all(),
            enabled: BTreeSet::new(),
            disabled: BTreeSet::new(),
        }
    }

    /// Build from the settings (`plugins.enabled` / `plugins.disabled`).
    pub fn from_settings(settings: &Settings) -> Self {
        Self {
            plugins: builtin::all(),
            enabled: settings.plugins.enabled.iter().cloned().collect(),
            disabled: settings.plugins.disabled.iter().cloned().collect(),
        }
    }

    pub fn infos(&self) -> Vec<PluginInfo> {
        self.plugins
            .iter()
            .map(|plugin| PluginInfo {
                id: plugin.id().to_string(),
                name: plugin.name().to_string(),
                description: plugin.description().to_string(),
                default_enabled: plugin.default_enabled(),
            })
            .collect()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Box<dyn Plugin>> {
        self.plugins.iter()
    }

    /// Whether a plugin runs for a request: the request preferences win over
    /// the settings.
    pub fn is_enabled(
        &self,
        id: &str,
        request_enabled: &[String],
        request_disabled: &[String],
    ) -> bool {
        if request_disabled.iter().any(|value| value == id) {
            return false;
        }
        let plugin = self.plugins.iter().find(|plugin| plugin.id() == id);
        let default_enabled = plugin
            .map(|plugin| plugin.default_enabled())
            .unwrap_or(false);
        if request_enabled.iter().any(|value| value == id) {
            return true;
        }
        if self.disabled.contains(id) {
            return false;
        }
        if self.enabled.contains(id) {
            return true;
        }
        default_enabled
    }

    pub fn pre_search(
        &self,
        query: &mut Query,
        context: &PluginContext,
        enabled: &[String],
        disabled: &[String],
    ) {
        for plugin in &self.plugins {
            if self.is_enabled(plugin.id(), enabled, disabled) {
                plugin.pre_search(query, context);
            }
        }
    }

    pub fn on_result(
        &self,
        result: &mut SearchResult,
        context: &PluginContext,
        enabled: &[String],
        disabled: &[String],
    ) {
        for plugin in &self.plugins {
            if self.is_enabled(plugin.id(), enabled, disabled) {
                plugin.on_result(result, context);
            }
        }
    }

    pub fn post_search(
        &self,
        response: &mut SearchResponse,
        context: &PluginContext,
        enabled: &[String],
        disabled: &[String],
    ) {
        for plugin in &self.plugins {
            if self.is_enabled(plugin.id(), enabled, disabled) {
                plugin.post_search(response, context);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Settings;

    #[test]
    fn bundled_plugins_are_listed() {
        let manager = PluginManager::new();
        let ids: Vec<String> = manager.infos().into_iter().map(|info| info.id).collect();
        for expected in [
            "hostnames",
            "tracker_url_remover",
            "self_info",
            "time_zone",
            "calculator",
            "hash_plugin",
            "oa_doi_rewrite",
        ] {
            assert!(ids.contains(&expected.to_string()), "missing {expected}");
        }
    }

    #[test]
    fn request_preferences_win_over_settings() {
        let manager = PluginManager::new();
        assert!(manager.is_enabled("hostnames", &[], &[]));
        assert!(!manager.is_enabled("hostnames", &[], &["hostnames".to_string()]));
        assert!(manager.is_enabled("hostnames", &["hostnames".to_string()], &[]));
        assert!(!manager.is_enabled(
            "hostnames",
            &["hostnames".to_string()],
            &["hostnames".to_string()]
        ));
    }

    #[test]
    fn self_info_is_opt_in() {
        let settings = Settings::defaults();
        let manager = PluginManager::from_settings(&settings);
        assert!(!manager.is_enabled("self_info", &[], &[]));
        assert!(manager.is_enabled("self_info", &["self_info".to_string()], &[]));
    }
}
