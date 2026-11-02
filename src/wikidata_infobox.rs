//! Wikidata infobox, ported from the infobox part of upstream's
//! 'searx/engines/wikidata.py', 'searx/wikidata_properties.py' and the bundled
//! data files 'searx/data/wikidata_properties.json' and
//! 'searx/data/wikidata_units.json' (both copied into 'src/data/').
//!
//! Upstream builds the infobox from a SPARQL query that resolves the labels of
//! every referenced entity while the query runs.  This port fetches the entity
//! document from 'Special:EntityData' instead, so there is no label service:
//! references to other entities (country, author, ...) are rendered with their
//! Wikidata id, and everything else (dates, quantities with their unit, media,
//! coordinates, URLs) is formatted like upstream.
//!
//! The module is standalone: the Lead adds 'pub mod wikidata_infobox;' to
//! 'src/lib.rs' and calls 'infobox_for()' with the searcher's network.

use std::collections::BTreeMap;
use std::sync::OnceLock;
use std::time::Duration;

use serde_json::Value;

use crate::engine::EngineRequest;
use crate::network::Network;
use crate::results::{Infobox, InfoboxUrl};

/// Base URL of the entity data API.
pub const ENTITY_DATA_URL: &str = "https://www.wikidata.org/wiki/Special:EntityData";

/// The language of the labels / descriptions that are read from the document.
pub const DEFAULT_LANGUAGE: &str = "en";

/// Prefix of entity ids in quantity values.
const ENTITY_URI_PREFIX: &str = "http://www.wikidata.org/entity/";

/// Width of the thumbnail requested from Wikimedia Commons.
const IMAGE_WIDTH: u32 = 300;

/// Timeout of the entity data request.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

/// The properties whose values are shown as infobox attributes, in the order of
/// upstream's 'get_attributes()' ('searx/wikidata_properties.py').  Image,
/// coordinate, Wikipedia and URL attributes are handled separately.
pub static DEFAULT_ATTRIBUTES: &[&str] = &[
    // Dates.
    "P571", "P576", "P580", "P582", "P569", "P570", "P619", "P620",
    // Labels (states and places).
    "P27", "P495", "P17", "P159", // Labels (politics).
    "P36", "P35", "P6", "P122", "P37", // Values and amounts.
    "P1082", "P2046", "P281", "P38", "P2048", // Media.
    "P400", "P50", "P170", "P57", "P175", "P178", "P162", "P176", "P58", "P272", "P264", "P123",
    "P449", "P750", "P86", "P577", "P136", "P364", "P212", "P957", "P275", "P277", "P348", "P840",
    // Languages.
    "P1098", "P282", "P1018", "P218", // Other.
    "P169", "P112", "P1454", "P137", "P1029", "P225", "P274", "P1346", "P1120", "P498",
];

/// Attributes whose value is a URL and which are collected in 'Infobox::urls'.
pub static URL_ATTRIBUTES: &[&str] = &["P856", "P1324", "P1581"];

/// Fediverse accounts, written as 'account@domain' and rewritten into a URL
/// with the given path prefix (upstream 'url_path_prefix').
pub static FEDIVERSE_ATTRIBUTES: &[(&str, &str)] =
    &[("P4033", "/@"), ("P11947", "/c/"), ("P12622", "/c/")];

/// Image properties with the priority upstream assigns to them; the image with
/// the highest priority that is present wins.
pub static IMAGE_ATTRIBUTES: &[(&str, u8)] = &[
    ("P15", 1),
    ("P242", 2),
    ("P154", 3),
    ("P18", 4),
    ("P41", 5),
    ("P2716", 6),
    ("P2910", 7),
];

/// The geographic coordinates property; its value becomes an OSM link.
pub const GEO_ATTRIBUTE: &str = "P625";

/// Labels of the properties that upstream resolved with the label service but
/// that are missing from 'wikidata_properties.json' (images and coordinates).
static EXTRA_LABELS: &[(&str, &str)] = &[
    ("P15", "Route map"),
    ("P18", "Image"),
    ("P41", "Flag"),
    ("P154", "Logo"),
    ("P242", "Locator map"),
    ("P2716", "Collage"),
    ("P2910", "Icon"),
    ("P625", "Coordinates"),
    ("P856", "Official website"),
];

static PROPERTIES: OnceLock<Vec<(&'static str, &'static str)>> = OnceLock::new();
static UNITS: OnceLock<BTreeMap<&'static str, &'static str>> = OnceLock::new();

/// Property labels from 'wikidata_properties.json', sorted by property id.
///
/// The table is built on first use from the JSON file that upstream uses to
/// label wikidata properties; entries without an English label are dropped.
pub fn properties() -> &'static [(&'static str, &'static str)] {
    PROPERTIES.get_or_init(build_properties).as_slice()
}

fn build_properties() -> Vec<(&'static str, &'static str)> {
    let document: Value =
        serde_json::from_str(include_str!("data/wikidata_properties.json")).unwrap_or(Value::Null);
    let mut table: Vec<(&'static str, &'static str)> = Vec::new();
    if let Value::Object(entries) = document {
        for (id, value) in entries {
            let label = match value {
                Value::String(label) => label,
                Value::Object(labels) => match labels.get(DEFAULT_LANGUAGE) {
                    Some(Value::String(label)) => label.clone(),
                    _ => continue,
                },
                _ => continue,
            };
            if label.is_empty() {
                continue;
            }
            table.push((
                Box::leak(id.into_boxed_str()),
                Box::leak(label.into_boxed_str()),
            ));
        }
    }
    table.sort_by(|left, right| left.0.cmp(right.0));
    table
}

/// The unit symbols from 'wikidata_units.json', keyed by unit entity id.
fn units() -> &'static BTreeMap<&'static str, &'static str> {
    UNITS.get_or_init(|| {
        let document: Value =
            serde_json::from_str(include_str!("data/wikidata_units.json")).unwrap_or(Value::Null);
        let mut table = BTreeMap::new();
        if let Value::Object(entries) = document {
            for (id, value) in entries {
                if let Some(Value::String(symbol)) = value.get("symbol") {
                    table.insert(
                        Box::leak(id.into_boxed_str()) as &'static str,
                        Box::leak(symbol.clone().into_boxed_str()) as &'static str,
                    );
                }
            }
        }
        table
    })
}

/// The label of a wikidata property, or the property id when it is unknown
/// (upstream 'get_label_for_entity()').
pub fn property_label(property: &str) -> String {
    let table = properties();
    if let Ok(index) = table.binary_search_by(|(id, _)| id.cmp(&property)) {
        return table[index].1.to_string();
    }
    if let Some((_, label)) = EXTRA_LABELS.iter().find(|(id, _)| *id == property) {
        return (*label).to_string();
    }
    property.to_string()
}

/// The symbol of a unit entity ('Q712226' -> 'km²'), or 'None' when the unit is
/// unknown.
pub fn unit_symbol(unit: &str) -> Option<&'static str> {
    units().get(unit).copied()
}

/// Normalize and validate an entity id ('q42' -> 'Q42').
pub fn normalize_entity_id(entity_id: &str) -> Option<String> {
    let trimmed = entity_id.trim();
    let mut chars = trimmed.chars();
    let prefix = chars.next()?;
    if prefix != 'Q' && prefix != 'q' && prefix != 'P' && prefix != 'p' {
        return None;
    }
    let digits: String = chars.collect();
    if digits.is_empty() || !digits.chars().all(|character| character.is_ascii_digit()) {
        return None;
    }
    Some(format!("{}{}", prefix.to_ascii_uppercase(), digits))
}

/// The 'Special:EntityData' URL of an entity, or 'None' for an invalid id.
pub fn entity_data_url(entity_id: &str) -> Option<String> {
    let id = normalize_entity_id(entity_id)?;
    Some(format!("{ENTITY_DATA_URL}/{id}.json"))
}

/// Fetch an entity and build its infobox; 'None' when the entity does not
/// exist, the request fails or the document cannot be parsed.
pub async fn infobox_for(network: &Network, entity_id: &str) -> Option<Infobox> {
    let id = normalize_entity_id(entity_id)?;
    let url = entity_data_url(&id)?;
    let request = EngineRequest::get(url).header("Accept", "application/json");
    let response = network
        .fetch(request, REQUEST_TIMEOUT, None, None)
        .await
        .ok()?;
    let document = response.json().ok()?;
    infobox_from_entity_json(&id, &document)
}

/// Build the infobox of one entity from an entity data document.  Split out of
/// 'infobox_for()' so the parsing can be tested with an inline fixture.
pub fn infobox_from_entity_json(entity_id: &str, document: &Value) -> Option<Infobox> {
    let id = normalize_entity_id(entity_id)?;
    let entity = document.get("entities")?.get(&id)?;

    let title = language_value(entity, "labels", DEFAULT_LANGUAGE).unwrap_or_else(|| id.clone());
    let content = language_value(entity, "descriptions", DEFAULT_LANGUAGE).unwrap_or_default();

    // Attributes, in the order of the configured property list (the map keeps
    // them sorted by their label, like the infobox template does).
    let mut attributes = BTreeMap::new();
    for property in DEFAULT_ATTRIBUTES {
        let values = formatted_claims(entity, property);
        if !values.is_empty() {
            attributes.insert(property_label(property), values.join(", "));
        }
    }

    // URLs: plain URL attributes, fediverse accounts, the OSM link of the
    // coordinates, the Wikipedia article and finally the Wikidata page.
    let mut urls: Vec<InfoboxUrl> = Vec::new();
    for property in URL_ATTRIBUTES {
        for value in formatted_claims(entity, property) {
            urls.push(InfoboxUrl {
                title: property_label(property),
                url: value,
            });
        }
    }
    for (property, prefix) in FEDIVERSE_ATTRIBUTES {
        for (_, value) in claim_values(entity, property) {
            if let Some(value) = value.as_str() {
                if let Some(url) = fediverse_url(value, prefix) {
                    urls.push(InfoboxUrl {
                        title: property_label(property),
                        url,
                    });
                }
            }
        }
    }
    if let Some((latitude, longitude)) = coordinates(entity) {
        urls.push(InfoboxUrl {
            title: property_label(GEO_ATTRIBUTE),
            url: osm_url(latitude, longitude),
        });
    }

    // The image with the highest upstream priority that is present wins.
    let mut img_src = None;
    let mut best_priority = 0u8;
    for (property, priority) in IMAGE_ATTRIBUTES {
        if *priority <= best_priority {
            continue;
        }
        if let Some(file) = first_string_claim(entity, property) {
            img_src = Some(commons_url(file));
            best_priority = *priority;
        }
    }

    // The Wikipedia article is the canonical infobox id when it exists
    // (upstream replaces the entity URL with the article URL).
    let mut infobox_id = entity_url(&id);
    if let Some(sitelink) = sitelink_title(entity, "enwiki") {
        let url = wikipedia_url(&sitelink);
        urls.push(InfoboxUrl {
            title: "Wikipedia (en)".to_string(),
            url: url.clone(),
        });
        infobox_id = url;
    }
    urls.push(InfoboxUrl {
        title: "Wikidata".to_string(),
        url: entity_url(&id),
    });

    Some(Infobox {
        id: infobox_id,
        title,
        content,
        img_src,
        url: Some(entity_url(&id)),
        urls,
        attributes,
        engine: "wikidata".to_string(),
    })
}

/// The value of 'labels[language].value' / 'descriptions[language].value'.
fn language_value(entity: &Value, field: &str, language: &str) -> Option<String> {
    entity
        .get(field)?
        .get(language)?
        .get("value")?
        .as_str()
        .map(str::to_string)
}

/// The non-deprecated main snaks of one property as (datatype, value) pairs.
fn claim_values(entity: &Value, property: &str) -> Vec<(String, Value)> {
    let mut values = Vec::new();
    let Some(statements) = entity
        .get("claims")
        .and_then(|claims| claims.get(property))
        .and_then(Value::as_array)
    else {
        return values;
    };
    for statement in statements {
        if statement.get("rank").and_then(Value::as_str) == Some("deprecated") {
            continue;
        }
        let Some(snak) = statement.get("mainsnak") else {
            continue;
        };
        if snak.get("snaktype").and_then(Value::as_str) != Some("value") {
            continue;
        }
        let Some(datatype) = snak.get("datatype").and_then(Value::as_str) else {
            continue;
        };
        let Some(value) = snak
            .get("datavalue")
            .and_then(|datavalue| datavalue.get("value"))
        else {
            continue;
        };
        values.push((datatype.to_string(), value.clone()));
    }
    values
}

/// The formatted, de-duplicated values of one property.
fn formatted_claims(entity: &Value, property: &str) -> Vec<String> {
    let mut values: Vec<String> = Vec::new();
    for (datatype, value) in claim_values(entity, property) {
        if let Some(formatted) = format_value(&datatype, &value) {
            if !formatted.is_empty() && !values.contains(&formatted) {
                values.push(formatted);
            }
        }
    }
    values
}

/// The first plain string value of one property (images, media files, ...).
fn first_string_claim(entity: &Value, property: &str) -> Option<String> {
    formatted_claims(entity, property).into_iter().next()
}

/// The coordinates of 'P625' as (latitude, longitude).
fn coordinates(entity: &Value) -> Option<(f64, f64)> {
    for (datatype, value) in claim_values(entity, GEO_ATTRIBUTE) {
        if datatype != "globecoordinate" {
            continue;
        }
        if let (Some(latitude), Some(longitude)) = (
            value.get("latitude").and_then(Value::as_f64),
            value.get("longitude").and_then(Value::as_f64),
        ) {
            return Some((latitude, longitude));
        }
    }
    None
}

/// The title of a Wikipedia sitelink ('enwiki').
fn sitelink_title(entity: &Value, site: &str) -> Option<String> {
    entity
        .get("sitelinks")?
        .get(site)?
        .get("title")?
        .as_str()
        .map(str::to_string)
}

/// Format one wikidata value by its datatype.
fn format_value(datatype: &str, value: &Value) -> Option<String> {
    match datatype {
        "wikibase-entityid" | "wikibase-item" | "wikibase-property" | "wikibase-lexeme"
        | "wikibase-form" | "wikibase-sense" => {
            value.get("id").and_then(Value::as_str).map(str::to_string)
        }
        "quantity" => {
            let amount = value.get("amount").and_then(Value::as_str)?;
            let amount = amount.strip_prefix('+').unwrap_or(amount);
            let unit = value.get("unit").and_then(Value::as_str).unwrap_or("1");
            if unit == "1" {
                return Some(amount.to_string());
            }
            let unit_id = unit.strip_prefix(ENTITY_URI_PREFIX).unwrap_or(unit);
            Some(format!(
                "{amount} {}",
                unit_symbol(unit_id).unwrap_or(unit_id)
            ))
        }
        "time" => value.get("time").and_then(Value::as_str).map(format_time),
        "url" => value.as_str().map(str::to_string),
        "monolingualtext" => value
            .get("text")
            .and_then(Value::as_str)
            .map(str::to_string),
        "globecoordinate" => {
            let latitude = value.get("latitude").and_then(Value::as_f64)?;
            let longitude = value.get("longitude").and_then(Value::as_f64)?;
            Some(format!("{latitude}, {longitude}"))
        }
        "commonsMedia" => value.as_str().map(str::to_string),
        _ => value
            .as_str()
            .map(str::to_string)
            .or_else(|| value.get("id").and_then(Value::as_str).map(str::to_string)),
    }
}

/// '+1952-03-11T00:00:00Z' -> '1952-03-11' (the precision is not shown).
fn format_time(time: &str) -> String {
    let date = time.split('T').next().unwrap_or(time);
    match date.strip_prefix('-') {
        Some(rest) => format!("-{rest}"),
        None => date.trim_start_matches('+').to_string(),
    }
}

/// The English Wikipedia article URL of a sitelink title.
fn wikipedia_url(title: &str) -> String {
    format!(
        "https://en.wikipedia.org/wiki/{}",
        crate::engine::util::encode(&title.replace(' ', "_"))
    )
}

/// The Wikidata page of an entity.
fn entity_url(id: &str) -> String {
    format!("https://www.wikidata.org/wiki/{id}")
}

/// The Wikimedia Commons thumbnail of a media file.
fn commons_url(file: String) -> String {
    format!(
        "https://commons.wikimedia.org/wiki/Special:FilePath/{}?width={IMAGE_WIDTH}",
        crate::engine::util::encode(&file.replace(' ', "_"))
    )
}

/// The upstream OpenStreetMap link of a coordinate ('external_urls.json').
fn osm_url(latitude: f64, longitude: f64) -> String {
    format!("https://www.openstreetmap.org/?lat={latitude}&lon={longitude}&zoom=19&layers=M")
}

/// 'account@domain' -> 'https://domain<prefix>account' (upstream
/// 'WDURLAttribute.url_path_prefix').
fn fediverse_url(value: &str, prefix: &str) -> Option<String> {
    let (account, domain) = value.rsplit_once('@')?;
    let account = account.trim().trim_start_matches('@');
    let domain = domain.trim();
    if account.is_empty() || domain.is_empty() {
        return None;
    }
    Some(format!("https://{domain}{prefix}{account}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn statement(datatype: &str, value: Value) -> Value {
        json!({
            "mainsnak": {
                "snaktype": "value",
                "datatype": datatype,
                "datavalue": {"value": value, "type": datatype}
            },
            "type": "statement",
            "rank": "normal"
        })
    }

    fn entity_document() -> Value {
        json!({
            "entities": {
                "Q42": {
                    "labels": {"en": {"language": "en", "value": "Douglas Adams"}},
                    "descriptions": {"en": {"language": "en", "value": "English writer"}},
                    "sitelinks": {"enwiki": {"site": "enwiki", "title": "Douglas Adams"}},
                    "claims": {
                        "P31": [statement("wikibase-item", json!({"id": "Q5"}))],
                        "P569": [statement("time", json!({"time": "+1952-03-11T00:00:00Z", "precision": 11}))],
                        "P570": [{
                            "mainsnak": {
                                "snaktype": "value",
                                "datatype": "time",
                                "datavalue": {"value": {"time": "+2001-05-11T00:00:00Z"}, "type": "time"}
                            },
                            "type": "statement",
                            "rank": "deprecated"
                        }],
                        "P2046": [statement("quantity", json!({
                            "amount": "+1.5",
                            "unit": "http://www.wikidata.org/entity/Q712226"
                        }))],
                        "P1082": [statement("quantity", json!({"amount": "+123456", "unit": "1"}))],
                        "P15": [statement("commonsMedia", json!("Map.jpg"))],
                        "P18": [statement("commonsMedia", json!("Douglas Adams.jpg"))],
                        "P856": [statement("url", json!("https://example.com/"))],
                        "P4033": [statement("string", json!("searxng@fosstodon.org"))],
                        "P625": [statement("globecoordinate", json!({"latitude": 52.5, "longitude": 13.4}))]
                    }
                }
            }
        })
    }

    #[test]
    fn infobox_is_built_from_an_entity_document() {
        let infobox = infobox_from_entity_json("Q42", &entity_document()).expect("infobox");
        assert_eq!(infobox.title, "Douglas Adams");
        assert_eq!(infobox.content, "English writer");
        assert_eq!(infobox.id, "https://en.wikipedia.org/wiki/Douglas_Adams");
        assert_eq!(infobox.engine, "wikidata");
        assert_eq!(
            infobox.attributes.get("Date of birth").map(String::as_str),
            Some("1952-03-11")
        );
        assert_eq!(
            infobox.attributes.get("Area").map(String::as_str),
            Some("1.5 km²")
        );
        assert_eq!(
            infobox.attributes.get("Population").map(String::as_str),
            Some("123456")
        );
        let titles: Vec<&str> = infobox.urls.iter().map(|url| url.title.as_str()).collect();
        for expected in [
            "Official website",
            "Mastodon",
            "Coordinates",
            "Wikipedia (en)",
            "Wikidata",
        ] {
            assert!(titles.contains(&expected), "missing url {expected}");
        }
        assert!(
            infobox
                .urls
                .iter()
                .any(|url| url.url == "https://fosstodon.org/@searxng")
        );
        assert!(
            infobox
                .urls
                .iter()
                .any(|url| url.url.contains("openstreetmap.org/?lat=52.5"))
        );
    }

    #[test]
    fn image_priority_prefers_the_higher_priority_property() {
        let infobox = infobox_from_entity_json("Q42", &entity_document()).expect("infobox");
        assert_eq!(
            infobox.img_src.as_deref(),
            Some("https://commons.wikimedia.org/wiki/Special:FilePath/Douglas_Adams.jpg?width=300")
        );
    }

    #[test]
    fn deprecated_claims_are_not_shown() {
        let infobox = infobox_from_entity_json("Q42", &entity_document()).expect("infobox");
        assert!(!infobox.attributes.contains_key("Date of death"));
    }

    #[test]
    fn unknown_entities_and_invalid_ids_return_none() {
        assert!(infobox_from_entity_json("Q404", &entity_document()).is_none());
        assert!(infobox_from_entity_json("not-an-id", &entity_document()).is_none());
        assert!(entity_data_url("42").is_none());
        assert!(entity_data_url("../secrets").is_none());
        assert_eq!(
            entity_data_url("q42").as_deref(),
            Some("https://www.wikidata.org/wiki/Special:EntityData/Q42.json")
        );
        assert_eq!(normalize_entity_id(" p31 ").as_deref(), Some("P31"));
    }

    #[test]
    fn entities_without_a_wikipedia_sitelink_use_the_wikidata_url() {
        let document = json!({
            "entities": {"Q1": {"labels": {"en": {"value": "Universe"}}, "claims": {}}}
        });
        let infobox = infobox_from_entity_json("Q1", &document).expect("infobox");
        assert_eq!(infobox.id, "https://www.wikidata.org/wiki/Q1");
        assert_eq!(infobox.title, "Universe");
        assert!(infobox.img_src.is_none());
    }

    #[test]
    fn properties_come_from_the_bundled_json() {
        let table = properties();
        assert!(table.len() > 100, "only {} properties", table.len());
        assert_eq!(property_label("P569"), "Date of birth");
        assert_eq!(property_label("P856"), "Official website");
        assert_eq!(property_label("P18"), "Image");
        // Unknown properties fall back to their id (upstream does the same).
        assert_eq!(property_label("P31"), "P31");
        assert_eq!(property_label("P999999"), "P999999");
        assert!(
            table
                .iter()
                .any(|(id, label)| *id == "P856" && *label == "Official website")
        );
    }

    #[test]
    fn unit_symbols_come_from_the_bundled_json() {
        assert_eq!(unit_symbol("Q712226"), Some("km²"));
        assert_eq!(unit_symbol("Q11573"), Some("m"));
        assert_eq!(unit_symbol("Q999999"), None);
    }

    #[test]
    fn values_are_formatted_by_datatype() {
        assert_eq!(
            format_value("wikibase-item", &json!({"id": "Q5"})).as_deref(),
            Some("Q5")
        );
        assert_eq!(
            format_value("time", &json!({"time": "+1952-03-11T00:00:00Z"})).as_deref(),
            Some("1952-03-11")
        );
        assert_eq!(
            format_value("quantity", &json!({"amount": "+1.5", "unit": "1"})).as_deref(),
            Some("1.5")
        );
        assert_eq!(
            format_value(
                "quantity",
                &json!({"amount": "-3", "unit": "http://www.wikidata.org/entity/Q11573"})
            )
            .as_deref(),
            Some("-3 m")
        );
        assert_eq!(
            format_value("monolingualtext", &json!({"text": "hello"})).as_deref(),
            Some("hello")
        );
        assert!(format_value("unknown", &json!({"nested": 1})).is_none());
    }

    #[test]
    fn fediverse_accounts_become_urls() {
        assert_eq!(
            fediverse_url("searxng@fosstodon.org", "/@").as_deref(),
            Some("https://fosstodon.org/@searxng")
        );
        assert_eq!(
            fediverse_url("searxng@lemmy.world", "/c/").as_deref(),
            Some("https://lemmy.world/c/searxng")
        );
        assert!(fediverse_url("no-domain", "/@").is_none());
    }
}
