//! Tor check plugin, ported from upstream's 'searx/plugins/tor_check.py'.
//!
//! Upstream fetches 'https://check.torproject.org/exit-addresses', checks the
//! IP address of the request against the exit node list and answers whether the
//! user is using Tor.
//!
//! This port keeps the parsing and the messages, but the plugin interface only
//! runs 'post_search()' synchronously (no network handle), so the answer of the
//! plugin is the exit check link plus the URL path of the request - the live
//! check is available as 'live_answer()' for the Lead to call from an async
//! context.
//!
//! The Lead declares 'pub mod tor_check;' in 'src/plugins/mod.rs' and adds
//! 'Box::new(TorCheck)' to 'src/plugins/builtin.rs' ('all()').

use super::{Plugin, PluginContext};
use crate::aggregator::SearchResponse;
use crate::engine::EngineRequest;
use crate::network::Network;

/// The upstream keywords that trigger the plugin.
pub const KEYWORDS: &[&str] = &["tor-check", "tor_check", "torcheck", "tor", "tor check"];

/// URL of the Tor exit node list (upstream 'url_exit_list').
pub const URL_EXIT_LIST: &str = "https://check.torproject.org/exit-addresses";

/// Human readable entry point of the exit check.
pub const CHECK_URL: &str = "https://check.torproject.org/";

/// Timeout of the exit node list request.
const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// The plugin (upstream 'tor_check').
pub struct TorCheck;

/// Whether the query asks for the Tor check (upstream compares the lowercased
/// query against its keywords).
pub fn matches(query: &str) -> bool {
    let query = query.trim().to_lowercase();
    KEYWORDS.contains(&query.as_str())
}

/// The synchronous answer: the exit check link and the URL path of the request
/// ("<base_url>/search?q=<query>").
pub fn answer(context: &PluginContext) -> String {
    let base = context.base_url.trim_end_matches('/');
    let query = crate::engine::util::encode(context.search_query.trim());
    format!("{URL_EXIT_LIST} - {base}/search?q={query}")
}

/// The IP addresses of the exit node list (upstream's regex
/// '(?<=ExitAddress )\S+').
pub fn parse_exit_addresses(body: &str) -> Vec<String> {
    let mut addresses = Vec::new();
    for line in body.lines() {
        let Some(rest) = line.strip_prefix("ExitAddress ") else {
            continue;
        };
        if let Some(address) = rest.split_whitespace().next() {
            addresses.push(address.to_string());
        }
    }
    addresses
}

/// Whether 'address' is in the exit node list.
pub fn is_exit_node(addresses: &[String], address: &str) -> bool {
    addresses.iter().any(|candidate| candidate == address)
}

/// Fetch the Tor exit node list; 'None' when it cannot be downloaded.
pub async fn exit_nodes(network: &Network) -> Option<Vec<String>> {
    let response = network
        .fetch(
            EngineRequest::get(URL_EXIT_LIST),
            REQUEST_TIMEOUT,
            None,
            None,
        )
        .await
        .ok()?;
    Some(parse_exit_addresses(&response.text()))
}

/// The upstream answers of the live check, keyed by the request's IP address.
pub async fn live_answer(network: &Network, address: &str) -> String {
    match exit_nodes(network).await {
        None => format!("Could not download the list of Tor exit-nodes from {URL_EXIT_LIST}"),
        Some(nodes) => {
            if is_exit_node(&nodes, address) {
                format!(
                    "You are using Tor and it looks like you have the external IP address {address}"
                )
            } else {
                format!("You are not using Tor and you have the external IP address {address}")
            }
        }
    }
}

impl Plugin for TorCheck {
    fn id(&self) -> &'static str {
        "tor_check"
    }
    fn name(&self) -> &'static str {
        "Tor check plugin"
    }
    fn description(&self) -> &'static str {
        "This plugin checks if the address of the request is a Tor exit-node, and informs the user if it is; like check.torproject.org, but from SearXNG."
    }
    fn default_enabled(&self) -> bool {
        false
    }
    fn post_search(&self, response: &mut SearchResponse, context: &PluginContext) {
        // Only answer on the first page (upstream checks the pageno).
        if response.pageno > 1 {
            return;
        }
        if matches(&context.search_query) {
            response.answers.push(answer(context));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aggregator::SearchResponse;
    use crate::engine::EngineRegistry;
    use crate::query::Query;
    use crate::settings::Settings;

    const EXIT_LIST: &str = "ExitAddress 171.25.193.20 2024-05-01 12:00:00\n\
ExitAddress 185.220.101.1 2024-05-01 12:00:00\n\
Some other line\n\
ExitAddress 23.129.64.190 2024-05-01 12:00:00\n";

    fn context(query: &str) -> PluginContext {
        PluginContext {
            search_query: query.to_string(),
            base_url: "http://localhost:8888/".into(),
            ..Default::default()
        }
    }

    fn empty_response(query: &str, pageno: u32) -> SearchResponse {
        let registry = EngineRegistry::from_settings(&Settings::defaults()).unwrap();
        let mut response = SearchResponse::new(&Query::parse(query, &registry, &[]));
        response.pageno = pageno;
        response
    }

    #[test]
    fn plugin_metadata_matches_upstream() {
        let plugin = TorCheck;
        assert_eq!(plugin.id(), "tor_check");
        assert_eq!(plugin.name(), "Tor check plugin");
        assert!(plugin.description().starts_with("This plugin checks"));
        assert!(!plugin.default_enabled());
    }

    #[test]
    fn keywords_trigger_the_plugin() {
        for keyword in ["tor", "tor-check", "TOR CHECK", "torcheck", "tor_check"] {
            assert!(matches(keyword), "{keyword} should match");
        }
        assert!(!matches("tor relay"));
        assert!(!matches(""));
    }

    #[test]
    fn exit_addresses_are_parsed_like_the_upstream_regex() {
        let addresses = parse_exit_addresses(EXIT_LIST);
        assert_eq!(
            addresses,
            vec![
                "171.25.193.20".to_string(),
                "185.220.101.1".to_string(),
                "23.129.64.190".to_string()
            ]
        );
        assert!(is_exit_node(&addresses, "185.220.101.1"));
        assert!(!is_exit_node(&addresses, "127.0.0.1"));
        assert!(parse_exit_addresses("no addresses here").is_empty());
    }

    #[test]
    fn the_answer_contains_the_link_and_the_url_path() {
        let answer = answer(&context("tor-check"));
        assert!(answer.contains(URL_EXIT_LIST));
        assert!(answer.contains("http://localhost:8888/search?q=tor-check"));
        assert!(!answer.contains("8888//search"));
    }

    #[test]
    fn post_search_answers_on_the_first_page_only() {
        let mut response = empty_response("tor-check", 1);
        TorCheck.post_search(&mut response, &context("tor-check"));
        assert_eq!(response.answers.len(), 1);
        let mut later_page = empty_response("tor-check", 2);
        TorCheck.post_search(&mut later_page, &context("tor-check"));
        assert!(later_page.answers.is_empty());
        let mut other = empty_response("rust", 1);
        TorCheck.post_search(&mut other, &context("rust"));
        assert!(other.answers.is_empty());
    }
}
