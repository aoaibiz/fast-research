//! Ahmia blacklist plugin, ported from upstream's 'searx/plugins/ahmia_filter.py'.
//!
//! Upstream keeps a list of MD5 hashes of onion host names
//! ('searx/data/ahmia_blacklist.txt', built by
//! 'searxng_extra/update/update_ahmia_blacklist.py') and drops every onion
//! result whose host name is in that list.
//!
//! Simplifications of this port:
//!
//! * the plugin interface of this crate has no way to drop a result from
//!   'on_result()' (upstream returns 'False' there), so the results are removed
//!   in 'post_search()' instead - the user sees the same result page;
//! * a tiny built-in sample is shipped so that the plugin works without the
//!   ~59.000 entry upstream data file.  A real instance loads the full list at
//!   startup with 'with_blacklist()' or by calling 'set_blacklist_path()' before
//!   the plugin manager is built;
//! * upstream disables the plugin when 'outgoing.using_tor_proxy' is false
//!   ('init()' returns False); the same check is exposed as 'enabled_for()'.
//!
//! The Lead declares 'pub mod ahmia_filter;' in 'src/plugins/mod.rs' and either
//! registers 'AhmiaFilter::new()' in 'src/plugins/builtin.rs' or builds the
//! filter with the entries of 'searx/data/ahmia_blacklist.txt'.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use super::{Plugin, PluginContext};
use crate::aggregator::SearchResponse;
use crate::results::SearchResult;
use crate::settings::Settings;

/// A tiny sample of Ahmia's blacklist: the first entries of the upstream data
/// file plus the hashes of a few well known onion host names (so that the tests
/// and small deployments have something to match against).
pub static BUILTIN_BLACKLIST: &[&str] = &[
    // First entries of upstream's searx/data/ahmia_blacklist.txt.
    "0000412c901989287c281fb4416d39dd",
    "0001541d64dde572ce7e71881c923c81",
    "0001ea1eb79a6974764beaa650f6688e",
    "0003109cbe5c38f2cfae235830f84a7b",
    // md5("facebookcorewwwi.onion"), md5("example.onion"),
    // md5("searxng.onion") and the DuckDuckGo onion service.
    "54ea3a06ba67018765ca5341c37fe34e",
    "b786647bf90127c0b783411bab9c33f7",
    "62f9c2054e2427b473df986ad34421d2",
    "c7bfbaed0b59d804993973304bdf6d7b",
];

/// Optional path of the full Ahmia blacklist file.  It has to be set before the
/// plugin manager is built (a plain static, no configuration file involved).
static BLACKLIST_PATH: OnceLock<PathBuf> = OnceLock::new();

/// Set the path of 'searx/data/ahmia_blacklist.txt' for 'AhmiaFilter::new()'.
pub fn set_blacklist_path(path: impl Into<PathBuf>) {
    let _ = BLACKLIST_PATH.set(path.into());
}

/// The configured blacklist path, when one was set.
pub fn blacklist_path() -> Option<&'static Path> {
    BLACKLIST_PATH.get().map(PathBuf::as_path)
}

/// The Ahmia blacklist filter.
pub struct AhmiaFilter {
    blacklist: BTreeSet<String>,
}

impl Default for AhmiaFilter {
    fn default() -> Self {
        Self::new()
    }
}

impl AhmiaFilter {
    /// The filter with the full blacklist from 'blacklist_path()', or with the
    /// built-in sample when no path was configured / the file cannot be read.
    pub fn new() -> Self {
        match blacklist_path() {
            Some(path) => Self::from_path(path).unwrap_or_else(|_| Self::builtin()),
            None => Self::builtin(),
        }
    }

    /// A filter with the given (MD5 hex) entries; the Lead loads
    /// 'searx/data/ahmia_blacklist.txt' and passes its entries here.
    pub fn with_blacklist(entries: Vec<String>) -> Self {
        Self::from_entries(entries)
    }

    /// The built-in sample list.
    pub fn builtin() -> Self {
        Self::from_entries(BUILTIN_BLACKLIST.iter().map(|entry| entry.to_string()))
    }

    fn from_entries(entries: impl IntoIterator<Item = String>) -> Self {
        Self {
            blacklist: entries
                .into_iter()
                .map(|entry| entry.trim().to_lowercase())
                .filter(|entry| !entry.is_empty())
                .collect(),
        }
    }

    /// Read a whitespace separated blacklist file (upstream
    /// 'ahmia_blacklist_loader()' uses 'f.read().split()').
    pub fn from_path(path: &Path) -> std::io::Result<Self> {
        let content = std::fs::read_to_string(path)?;
        Ok(Self::from_entries(
            content.split_whitespace().map(str::to_string),
        ))
    }

    /// The number of blacklist entries.
    pub fn len(&self) -> usize {
        self.blacklist.len()
    }

    pub fn is_empty(&self) -> bool {
        self.blacklist.is_empty()
    }

    /// Whether the MD5 hash of the host name is blacklisted.
    pub fn is_blacklisted(&self, host: &str) -> bool {
        self.blacklist.contains(&md5_hex(host.as_bytes()))
    }

    /// Whether a result URL points at a blacklisted onion service.
    pub fn blocks(&self, url: &str) -> bool {
        match host_of(url) {
            Some(host) => is_onion_host(&host) && self.is_blacklisted(&host),
            None => false,
        }
    }
}

/// Whether the plugin is usable on an instance (upstream only loads the filter
/// when the instance uses a Tor proxy).
pub fn enabled_for(settings: &Settings) -> bool {
    settings.outgoing.using_tor_proxy
}

/// The host name of a result URL (without a trailing dot).
pub fn host_of(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url).ok()?;
    let host = parsed.host_str()?;
    Some(host.trim_end_matches('.').to_string())
}

/// Whether a host name is an onion service (upstream's 'result.is_onion').
pub fn is_onion_host(host: &str) -> bool {
    host.to_lowercase().ends_with(".onion")
}

impl Plugin for AhmiaFilter {
    fn id(&self) -> &'static str {
        "ahmia_filter"
    }
    fn name(&self) -> &'static str {
        "Ahmia blacklist"
    }
    fn description(&self) -> &'static str {
        "Filter out onion results that appear in Ahmia's blacklist."
    }
    fn default_enabled(&self) -> bool {
        true
    }
    fn on_result(&self, _result: &mut SearchResult, _context: &PluginContext) {
        // Upstream drops blacklisted onion results here; this crate's plugin
        // interface cannot remove a result, see 'post_search()'.
    }
    fn post_search(&self, response: &mut SearchResponse, _context: &PluginContext) {
        response.results.retain(|result| !self.blocks(&result.url));
    }
}

// ---------------------------------------------------------------------------
// MD5 (RFC 1321); upstream uses hashlib.md5() to hash the onion host names.
// ---------------------------------------------------------------------------

const MD5_SHIFTS: [u32; 64] = [
    7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9,
    14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10, 15,
    21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
];

const MD5_CONSTANTS: [u32; 64] = [
    0xD76AA478, 0xE8C7B756, 0x242070DB, 0xC1BDCEEE, 0xF57C0FAF, 0x4787C62A, 0xA8304613, 0xFD469501,
    0x698098D8, 0x8B44F7AF, 0xFFFF5BB1, 0x895CD7BE, 0x6B901122, 0xFD987193, 0xA679438E, 0x49B40821,
    0xF61E2562, 0xC040B340, 0x265E5A51, 0xE9B6C7AA, 0xD62F105D, 0x02441453, 0xD8A1E681, 0xE7D3FBC8,
    0x21E1CDE6, 0xC33707D6, 0xF4D50D87, 0x455A14ED, 0xA9E3E905, 0xFCEFA3F8, 0x676F02D9, 0x8D2A4C8A,
    0xFFFA3942, 0x8771F681, 0x6D9D6122, 0xFDE5380C, 0xA4BEEA44, 0x4BDECFA9, 0xF6BB4B60, 0xBEBFBC70,
    0x289B7EC6, 0xEAA127FA, 0xD4EF3085, 0x04881D05, 0xD9D4D039, 0xE6DB99E5, 0x1FA27CF8, 0xC4AC5665,
    0xF4292244, 0x432AFF97, 0xAB9423A7, 0xFC93A039, 0x655B59C3, 0x8F0CCC92, 0xFFEFF47D, 0x85845DD1,
    0x6FA87E4F, 0xFE2CE6E0, 0xA3014314, 0x4E0811A1, 0xF7537E82, 0xBD3AF235, 0x2AD7D2BB, 0xEB86D391,
];

/// The MD5 digest of 'input'.
pub fn md5(input: &[u8]) -> [u8; 16] {
    let mut message = input.to_vec();
    let bit_length = (input.len() as u64).wrapping_mul(8);
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bit_length.to_le_bytes());

    let mut state = [0x67452301u32, 0xEFCDAB89, 0x98BADCFE, 0x10325476];
    for chunk in message.chunks(64) {
        let mut words = [0u32; 16];
        for (index, word) in chunk.chunks(4).enumerate() {
            words[index] = u32::from_le_bytes([word[0], word[1], word[2], word[3]]);
        }
        let [mut a, mut b, mut c, mut d] = state;
        for index in 0..64 {
            let (mixed, word) = match index {
                0..=15 => ((b & c) | (!b & d), index),
                16..=31 => ((d & b) | (!d & c), (5 * index + 1) % 16),
                32..=47 => (b ^ c ^ d, (3 * index + 5) % 16),
                _ => (c ^ (b | !d), (7 * index) % 16),
            };
            let sum = a
                .wrapping_add(mixed)
                .wrapping_add(MD5_CONSTANTS[index])
                .wrapping_add(words[word]);
            a = d;
            d = c;
            c = b;
            b = b.wrapping_add(sum.rotate_left(MD5_SHIFTS[index]));
        }
        state[0] = state[0].wrapping_add(a);
        state[1] = state[1].wrapping_add(b);
        state[2] = state[2].wrapping_add(c);
        state[3] = state[3].wrapping_add(d);
    }

    let mut digest = [0u8; 16];
    for (index, word) in state.iter().enumerate() {
        digest[index * 4..index * 4 + 4].copy_from_slice(&word.to_le_bytes());
    }
    digest
}

/// The lowercase hex digest of 'input' (upstream 'md5(...).hexdigest()').
pub fn md5_hex(input: &[u8]) -> String {
    md5(input)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aggregator::SearchResponse;
    use crate::engine::EngineRegistry;
    use crate::query::Query;
    use crate::results::SearchResult;

    fn context() -> PluginContext {
        PluginContext {
            base_url: "http://localhost:8888".into(),
            ..Default::default()
        }
    }

    fn empty_response() -> SearchResponse {
        let registry = EngineRegistry::from_settings(&Settings::defaults()).unwrap();
        SearchResponse::new(&Query::parse("onion", &registry, &[]))
    }

    #[test]
    fn md5_matches_the_rfc_1321_vectors() {
        assert_eq!(md5_hex(b""), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(md5_hex(b"abc"), "900150983cd24fb0d6963f7d28e17f72");
        assert_eq!(
            md5_hex(b"The quick brown fox jumps over the lazy dog"),
            "9e107d9d372bb6826bd81d3542a419d6"
        );
        assert_eq!(
            md5_hex(
                b"12345678901234567890123456789012345678901234567890123456789012345678901234567890"
            ),
            "57edf4a22be3c955ac49da2e2107b67a"
        );
    }

    #[test]
    fn onion_hosts_are_recognised() {
        assert!(is_onion_host("facebookcorewwwi.onion"));
        assert!(is_onion_host("example.ONION"));
        assert!(!is_onion_host("onion.com"));
        assert_eq!(
            host_of("http://example.onion/x").as_deref(),
            Some("example.onion")
        );
        assert_eq!(host_of("not a url"), None);
    }

    #[test]
    fn builtin_sample_blocks_blacklisted_onion_results() {
        let filter = AhmiaFilter::builtin();
        assert!(filter.len() >= BUILTIN_BLACKLIST.len());
        assert!(filter.blocks("http://facebookcorewwwi.onion/"));
        assert!(filter.blocks("http://example.onion/search?q=x"));
        assert!(filter.is_blacklisted("example.onion"));
        assert!(!filter.is_blacklisted("example.com"));
        assert!(!filter.blocks("https://example.com/"));
    }

    #[test]
    fn with_blacklist_uses_the_given_entries() {
        let filter = AhmiaFilter::with_blacklist(vec![
            md5_hex(b"evil.onion"),
            "  EXTRA-HASH  ".to_string(),
            String::new(),
        ]);
        assert_eq!(filter.len(), 2);
        assert!(filter.blocks("http://evil.onion/"));
        assert!(!filter.blocks("http://good.onion/"));
        assert!(!filter.is_empty());
    }

    #[test]
    fn from_path_reads_a_whitespace_separated_file() {
        let path = std::env::temp_dir().join(format!("ahmia-test-{}.txt", std::process::id()));
        let content = format!(
            "{}\n{} extra\n",
            md5_hex(b"evil.onion"),
            md5_hex(b"bad.onion")
        );
        std::fs::write(&path, content).unwrap();
        let filter = AhmiaFilter::from_path(&path).unwrap();
        assert_eq!(filter.len(), 3);
        assert!(filter.is_blacklisted("evil.onion"));
        assert!(filter.is_blacklisted("bad.onion"));
        assert!(!filter.is_blacklisted("good.onion"));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn post_search_removes_the_blacklisted_results() {
        let mut response = empty_response();
        response.results = vec![
            SearchResult::new("http://facebookcorewwwi.onion/", "blocked"),
            SearchResult::new("https://example.com/", "kept"),
            SearchResult::new("http://good.onion/", "kept onion"),
        ];
        AhmiaFilter::builtin().post_search(&mut response, &context());
        let urls: Vec<&str> = response.results.iter().map(|r| r.url.as_str()).collect();
        assert_eq!(urls, vec!["https://example.com/", "http://good.onion/"]);
    }

    #[test]
    fn plugin_metadata_matches_upstream() {
        let plugin = AhmiaFilter::builtin();
        assert_eq!(plugin.id(), "ahmia_filter");
        assert_eq!(plugin.name(), "Ahmia blacklist");
        assert_eq!(
            plugin.description(),
            "Filter out onion results that appear in Ahmia's blacklist."
        );
        assert!(plugin.default_enabled());
        assert!(!enabled_for(&Settings::defaults()));
    }
}
