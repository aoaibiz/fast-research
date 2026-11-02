//! User preferences, ported from `searx/preferences.py`.
//!
//! Preferences are stored in cookies, one cookie per setting, exactly like
//! upstream (the `searxng_prefs` cookie does not exist).

use crate::settings::Settings;

/// Cookie name of a preference (upstream `searx/preferences.py`).
pub const COOKIE_CATEGORIES: &str = "category";
pub const COOKIE_DISABLED_ENGINES: &str = "disabled_engines";
pub const COOKIE_ENABLED_PLUGINS: &str = "enabled_plugins";
pub const COOKIE_DISABLED_PLUGINS: &str = "disabled_plugins";
pub const COOKIE_IMAGE_PROXY: &str = "image_proxy";
pub const COOKIE_LANGUAGE: &str = "language";
pub const COOKIE_LOCALE: &str = "locale";
pub const COOKIE_METHOD: &str = "method";
pub const COOKIE_SAFESEARCH: &str = "safesearch";
pub const COOKIE_THEME: &str = "theme";
pub const COOKIE_AUTOCOMPLETE: &str = "autocomplete";
pub const COOKIE_MAX_PAGE: &str = "max_page";
pub const COOKIE_RESULTS_ON_NEW_TAB: &str = "results_on_new_tab";
pub const COOKIE_INFINITE_SCROLL: &str = "infinite_scroll";
pub const COOKIE_CENTER_ALIGNMENT: &str = "center_alignment";

/// The preferences of one visitor.
#[derive(Debug, Clone)]
pub struct Preferences {
    pub categories: Vec<String>,
    pub disabled_engines: Vec<String>,
    pub enabled_plugins: Vec<String>,
    pub disabled_plugins: Vec<String>,
    pub language: String,
    pub locale: String,
    pub safesearch: u8,
    pub image_proxy: bool,
    pub autocomplete: String,
    pub theme: String,
    pub method: String,
    pub max_page: u32,
    pub results_on_new_tab: bool,
    pub infinite_scroll: bool,
    pub center_alignment: bool,
}

impl Preferences {
    /// The defaults from the settings file.
    pub fn from_settings(settings: &Settings) -> Self {
        Self {
            categories: vec![crate::categories::GENERAL.to_string()],
            disabled_engines: Vec::new(),
            enabled_plugins: settings.plugins.enabled.clone(),
            disabled_plugins: settings.plugins.disabled.clone(),
            language: settings.search.default_lang.clone(),
            locale: settings.ui.default_locale.clone(),
            safesearch: settings.search.safe_search,
            image_proxy: settings.server.image_proxy,
            autocomplete: settings.search.autocomplete.clone(),
            theme: settings.ui.default_theme.clone(),
            method: settings.server.method.clone(),
            max_page: settings.search.max_page,
            results_on_new_tab: settings.ui.results_on_new_tab,
            infinite_scroll: settings.ui.infinite_scroll,
            center_alignment: settings.ui.center_alignment,
        }
    }

    /// Read the preferences of a request from its `Cookie` header.
    pub fn from_cookies(cookie_header: Option<&str>, settings: &Settings) -> Self {
        let mut preferences = Self::from_settings(settings);
        let Some(cookie_header) = cookie_header else {
            return preferences;
        };
        for cookie in cookie_header.split(';') {
            let mut parts = cookie.splitn(2, '=');
            let name = parts.next().unwrap_or_default().trim();
            let value = decode(parts.next().unwrap_or_default().trim());
            match name {
                // upstream calls the cookie "category"; accept the old name too
                COOKIE_CATEGORIES | "categories" => preferences.categories = split_list(&value),
                COOKIE_DISABLED_ENGINES => preferences.disabled_engines = split_list(&value),
                COOKIE_ENABLED_PLUGINS => preferences.enabled_plugins = split_list(&value),
                COOKIE_DISABLED_PLUGINS => preferences.disabled_plugins = split_list(&value),
                COOKIE_LANGUAGE => preferences.language = value,
                COOKIE_LOCALE => preferences.locale = value,
                COOKIE_SAFESEARCH => {
                    preferences.safesearch = value.parse().unwrap_or(preferences.safesearch).min(2)
                }
                COOKIE_IMAGE_PROXY => preferences.image_proxy = is_true(&value),
                COOKIE_AUTOCOMPLETE => preferences.autocomplete = value,
                COOKIE_THEME => preferences.theme = value,
                COOKIE_METHOD => preferences.method = value,
                COOKIE_MAX_PAGE => {
                    preferences.max_page = value.parse().unwrap_or(preferences.max_page)
                }
                COOKIE_RESULTS_ON_NEW_TAB => preferences.results_on_new_tab = is_true(&value),
                COOKIE_INFINITE_SCROLL => preferences.infinite_scroll = is_true(&value),
                COOKIE_CENTER_ALIGNMENT => preferences.center_alignment = is_true(&value),
                _ => {}
            }
        }
        preferences
    }

    /// The `Set-Cookie` headers answering a preferences form submission.
    pub fn to_cookies(&self) -> Vec<(String, String)> {
        let mut cookies = vec![
            (COOKIE_CATEGORIES.to_string(), join_list(&self.categories)),
            (
                COOKIE_DISABLED_ENGINES.to_string(),
                join_list(&self.disabled_engines),
            ),
            (
                COOKIE_ENABLED_PLUGINS.to_string(),
                join_list(&self.enabled_plugins),
            ),
            (
                COOKIE_DISABLED_PLUGINS.to_string(),
                join_list(&self.disabled_plugins),
            ),
            (COOKIE_LANGUAGE.to_string(), self.language.clone()),
            (COOKIE_LOCALE.to_string(), self.locale.clone()),
            (COOKIE_SAFESEARCH.to_string(), self.safesearch.to_string()),
            (COOKIE_IMAGE_PROXY.to_string(), self.image_proxy.to_string()),
            (COOKIE_AUTOCOMPLETE.to_string(), self.autocomplete.clone()),
            (COOKIE_THEME.to_string(), self.theme.clone()),
            (COOKIE_METHOD.to_string(), self.method.clone()),
            (COOKIE_MAX_PAGE.to_string(), self.max_page.to_string()),
            (
                COOKIE_RESULTS_ON_NEW_TAB.to_string(),
                self.results_on_new_tab.to_string(),
            ),
            (
                COOKIE_INFINITE_SCROLL.to_string(),
                self.infinite_scroll.to_string(),
            ),
            (
                COOKIE_CENTER_ALIGNMENT.to_string(),
                self.center_alignment.to_string(),
            ),
        ];
        for (_, value) in &mut cookies {
            *value = encode(value);
        }
        cookies
    }
}

fn is_true(value: &str) -> bool {
    matches!(value.to_lowercase().as_str(), "1" | "true" | "on" | "yes")
}

/// Split a `|` separated list.
pub fn split_list(value: &str) -> Vec<String> {
    value
        .split('|')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_string)
        .collect()
}

/// Join a list with `|`.
pub fn join_list(values: &[String]) -> String {
    values.join("|")
}

fn encode(value: &str) -> String {
    percent_encoding::utf8_percent_encode(value, percent_encoding::NON_ALPHANUMERIC).to_string()
}

fn decode(value: &str) -> String {
    percent_encoding::percent_decode_str(value)
        .decode_utf8_lossy()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cookies_round_trip() {
        let settings = Settings::defaults();
        let mut preferences = Preferences::from_settings(&settings);
        preferences.language = "ja".into();
        preferences.disabled_engines = vec!["google".into(), "bing".into()];
        preferences.safesearch = 2;
        let cookies = preferences.to_cookies();
        let header = cookies
            .iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect::<Vec<_>>()
            .join("; ");
        let restored = Preferences::from_cookies(Some(&header), &settings);
        assert_eq!(restored.language, "ja");
        assert_eq!(restored.safesearch, 2);
        assert_eq!(restored.disabled_engines, vec!["google", "bing"]);
    }

    #[test]
    fn missing_cookie_header_falls_back_to_defaults() {
        let settings = Settings::defaults();
        let preferences = Preferences::from_cookies(None, &settings);
        assert_eq!(preferences.safesearch, settings.search.safe_search);
    }
}
