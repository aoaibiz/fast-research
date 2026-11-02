//! Unit converter plugin, ported from upstream's 'searx/plugins/unit_converter.py'.
//!
//! Upstream resolves the unit symbols against the whole Wikidata unit table
//! ('searx/wikidata_units.py') and uses Babel to parse and format the number.
//! This port ships a small hand written table for the dimensions SearXNG's users
//! ask for most (length, mass, temperature, area, volume, speed) and formats the
//! result itself: no thousands grouping, a decimal comma for the locales that
//! use one.
//!
//! The Lead registers the plugin in 'src/plugins/builtin.rs' ('all()') and
//! declares 'pub mod unit_converter;' in 'src/plugins/mod.rs'.

use super::{Plugin, PluginContext};
use crate::aggregator::SearchResponse;

/// Keywords that separate the source from the target ("10 km in miles").
pub const CONVERT_KEYWORDS: &[&str] = &["in", "to", "as"];

/// The dimension of a unit; units of different dimensions are never converted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dimension {
    Length,
    Mass,
    Temperature,
    Area,
    Volume,
    Speed,
}

/// How a unit is turned into (and back from) the SI unit of its dimension.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Scale {
    /// 'si = value * factor'.
    Factor(f64),
    /// Degrees Celsius.
    Celsius,
    /// Degrees Fahrenheit.
    Fahrenheit,
    /// Kelvin.
    Kelvin,
}

impl Scale {
    /// The value in the SI unit of the dimension.
    pub fn to_si(self, value: f64) -> f64 {
        match self {
            Self::Factor(factor) => value * factor,
            Self::Celsius => value + 273.15,
            Self::Fahrenheit => (value + 459.67) * 5.0 / 9.0,
            Self::Kelvin => value,
        }
    }

    /// The value in this unit, given a value in the SI unit.
    pub fn from_si(self, value: f64) -> f64 {
        match self {
            Self::Factor(factor) => value / factor,
            Self::Celsius => value - 273.15,
            Self::Fahrenheit => value * 9.0 / 5.0 - 459.67,
            Self::Kelvin => value,
        }
    }
}

/// One unit of the conversion table.
#[derive(Debug, Clone, Copy)]
pub struct Unit {
    /// The canonical symbol (the first one), used in the answer.
    pub symbol: &'static str,
    /// Every symbol the unit is known by (upstream's 'symbol_to_si').
    pub symbols: &'static [&'static str],
    /// The dimension (upstream compares the 'si_name' of source and target).
    pub dimension: Dimension,
    pub scale: Scale,
}

/// The conversion table.  Symbols that are ambiguous appear more than once and
/// are resolved by the dimension of the other side of the conversion
/// ("1 oz in g" is a mass, "1 oz in ml" a volume).
pub static UNITS: &[Unit] = &[
    // Length (SI: metre).
    unit(
        "m",
        &["m", "meter", "meters", "metre", "metres"],
        Dimension::Length,
        1.0,
    ),
    unit(
        "km",
        &["km", "kilometer", "kilometers", "kilometre", "kilometres"],
        Dimension::Length,
        1000.0,
    ),
    unit(
        "cm",
        &[
            "cm",
            "centimeter",
            "centimeters",
            "centimetre",
            "centimetres",
        ],
        Dimension::Length,
        0.01,
    ),
    unit(
        "mm",
        &[
            "mm",
            "millimeter",
            "millimeters",
            "millimetre",
            "millimetres",
        ],
        Dimension::Length,
        0.001,
    ),
    unit("mi", &["mi", "mile", "miles"], Dimension::Length, 1609.344),
    unit("yd", &["yd", "yard", "yards"], Dimension::Length, 0.9144),
    unit("ft", &["ft", "foot", "feet"], Dimension::Length, 0.3048),
    unit("in", &["in", "inch", "inches"], Dimension::Length, 0.0254),
    unit(
        "nmi",
        &["nmi", "nautical mile", "nautical miles"],
        Dimension::Length,
        1852.0,
    ),
    // Mass (SI: kilogram).
    unit("kg", &["kg", "kilogram", "kilograms"], Dimension::Mass, 1.0),
    unit("g", &["g", "gram", "grams"], Dimension::Mass, 0.001),
    unit(
        "mg",
        &["mg", "milligram", "milligrams"],
        Dimension::Mass,
        1e-6,
    ),
    unit(
        "t",
        &["t", "tonne", "tonnes", "ton", "tons"],
        Dimension::Mass,
        1000.0,
    ),
    unit(
        "lb",
        &["lb", "lbs", "pound", "pounds"],
        Dimension::Mass,
        0.45359237,
    ),
    unit(
        "oz",
        &["oz", "ounce", "ounces"],
        Dimension::Mass,
        0.028349523125,
    ),
    unit(
        "st",
        &["st", "stone", "stones"],
        Dimension::Mass,
        6.35029318,
    ),
    // Temperature (SI: kelvin).
    unit_with(
        "c",
        &["c", "\u{00b0}c", "celsius", "centigrade"],
        Dimension::Temperature,
        Scale::Celsius,
    ),
    unit_with(
        "f",
        &["f", "\u{00b0}f", "fahrenheit"],
        Dimension::Temperature,
        Scale::Fahrenheit,
    ),
    unit_with("k", &["k", "kelvin"], Dimension::Temperature, Scale::Kelvin),
    // Area (SI: square metre).
    unit(
        "m2",
        &["m2", "m\u{00b2}", "sqm", "square meter", "square meters"],
        Dimension::Area,
        1.0,
    ),
    unit(
        "km2",
        &["km2", "km\u{00b2}", "square kilometer", "square kilometers"],
        Dimension::Area,
        1e6,
    ),
    unit(
        "cm2",
        &[
            "cm2",
            "cm\u{00b2}",
            "square centimeter",
            "square centimeters",
        ],
        Dimension::Area,
        1e-4,
    ),
    unit("ha", &["ha", "hectare", "hectares"], Dimension::Area, 1e4),
    unit("acre", &["acre", "acres"], Dimension::Area, 4046.8564224),
    unit(
        "ft2",
        &["ft2", "ft\u{00b2}", "sqft", "square foot", "square feet"],
        Dimension::Area,
        0.09290304,
    ),
    unit(
        "in2",
        &["in2", "in\u{00b2}", "sqin", "square inch", "square inches"],
        Dimension::Area,
        0.00064516,
    ),
    unit(
        "mi2",
        &["mi2", "mi\u{00b2}", "square mile", "square miles"],
        Dimension::Area,
        2589988.110336,
    ),
    // Volume (SI: cubic metre).
    unit(
        "m3",
        &["m3", "m\u{00b3}", "cbm", "cubic meter", "cubic meters"],
        Dimension::Volume,
        1.0,
    ),
    unit(
        "l",
        &["l", "liter", "liters", "litre", "litres"],
        Dimension::Volume,
        0.001,
    ),
    unit(
        "ml",
        &[
            "ml",
            "milliliter",
            "milliliters",
            "millilitre",
            "millilitres",
        ],
        Dimension::Volume,
        1e-6,
    ),
    unit(
        "cl",
        &["cl", "centiliter", "centiliters"],
        Dimension::Volume,
        1e-5,
    ),
    unit(
        "gal",
        &["gal", "gallon", "gallons"],
        Dimension::Volume,
        0.003785411784,
    ),
    unit(
        "qt",
        &["qt", "quart", "quarts"],
        Dimension::Volume,
        0.000946352946,
    ),
    unit(
        "pt",
        &["pt", "pint", "pints"],
        Dimension::Volume,
        0.000473176473,
    ),
    unit("cup", &["cup", "cups"], Dimension::Volume, 0.0002365882365),
    unit(
        "oz",
        &["oz", "fl oz", "fluid ounce", "fluid ounces"],
        Dimension::Volume,
        2.95735295625e-5,
    ),
    // Speed (SI: metre per second).
    unit(
        "km/h",
        &["km/h", "kph", "kmh", "kilometers per hour"],
        Dimension::Speed,
        1.0 / 3.6,
    ),
    unit(
        "m/s",
        &["m/s", "mps", "meters per second"],
        Dimension::Speed,
        1.0,
    ),
    unit("mph", &["mph", "miles per hour"], Dimension::Speed, 0.44704),
    unit(
        "kn",
        &["kn", "knot", "knots"],
        Dimension::Speed,
        0.5144444444444445,
    ),
];

const fn unit(
    symbol: &'static str,
    symbols: &'static [&'static str],
    dimension: Dimension,
    factor: f64,
) -> Unit {
    unit_with(symbol, symbols, dimension, Scale::Factor(factor))
}

const fn unit_with(
    symbol: &'static str,
    symbols: &'static [&'static str],
    dimension: Dimension,
    scale: Scale,
) -> Unit {
    Unit {
        symbol,
        symbols,
        dimension,
        scale,
    }
}

/// The conversion table.
pub fn units() -> &'static [Unit] {
    UNITS
}

/// Every unit whose symbol equals 'symbol' (exact match first, then a unique
/// case-insensitive match - unit symbols are case sensitive, but "C" / "F" are
/// usually written carelessly).
pub fn resolve(symbol: &str) -> Vec<&'static Unit> {
    let exact: Vec<&'static Unit> = UNITS
        .iter()
        .filter(|unit| unit.symbols.contains(&symbol))
        .collect();
    if !exact.is_empty() {
        return exact;
    }
    let lowered = symbol.to_lowercase();
    UNITS
        .iter()
        .filter(|unit| {
            unit.symbols
                .iter()
                .any(|candidate| candidate.to_lowercase() == lowered)
        })
        .collect()
}

/// The result of a conversion: the value in 'unit'.
#[derive(Debug, Clone, Copy)]
pub struct Conversion {
    pub value: f64,
    pub unit: &'static str,
}

/// Convert 'value' from one unit into another; 'None' when one of the units is
/// unknown or both belong to different dimensions.
pub fn convert(value: f64, from: &str, to: &str) -> Option<Conversion> {
    let sources = resolve(from);
    let targets = resolve(to);
    for source in &sources {
        for target in &targets {
            if source.dimension != target.dimension {
                continue;
            }
            let si = source.scale.to_si(value);
            return Some(Conversion {
                value: target.scale.from_si(si),
                unit: target.symbol,
            });
        }
    }
    None
}

/// Parse "<number> <unit> (in|to|as) <unit>" (upstream's 'RE_MEASURE').
///
/// The keyword is searched from the right, so that the unit symbol "in" (inch)
/// still works ("10 in in cm"); earlier occurrences are used as a fallback.
pub fn parse_query(query: &str) -> Option<(f64, String, String)> {
    let parts: Vec<&str> = query.split_whitespace().collect();
    if parts.len() < 3 {
        return None;
    }
    for index in (1..parts.len() - 1).rev() {
        if !CONVERT_KEYWORDS.contains(&parts[index].to_lowercase().as_str()) {
            continue;
        }
        let mut from = parts[..index].join(" ");
        let mut to = parts[index + 1..].join(" ");
        // "10 in in cm" leaves the unit "in" on the source side.
        if let Some((value, unit)) = parse_measure(&from) {
            to = to.trim().to_string();
            from = unit;
            if !to.is_empty() {
                return Some((value, from, to));
            }
            continue;
        }
    }
    None
}

/// Parse "<number> <unit>" into the value and the unit symbol.
pub fn parse_measure(measure: &str) -> Option<(f64, String)> {
    let measure = measure.trim();
    let split = measure.find(char::is_whitespace)?;
    let (number, unit) = measure.split_at(split);
    let value = parse_number(number)?;
    let unit = unit.trim();
    if unit.is_empty() {
        return None;
    }
    Some((value, unit.to_string()))
}

/// Parse a number that may use "." or "," as the decimal separator and may be
/// written in scientific notation ("1,5e3").
pub fn parse_number(number: &str) -> Option<f64> {
    let number = number.trim();
    if number.is_empty() {
        return None;
    }
    let sign = if number.starts_with('-') { "-" } else { "" };
    let magnitude = number.trim_start_matches(['+', '-']);
    let (mantissa, exponent) = match magnitude.find(['e', 'E']) {
        Some(index) => (&magnitude[..index], &magnitude[index + 1..]),
        None => (magnitude, ""),
    };
    let digits = match (mantissa.rfind('.'), mantissa.rfind(',')) {
        // "1.000,5": the comma is the decimal separator.
        (Some(dot), Some(comma)) if dot > comma => mantissa.replace(',', ""),
        // "1,000.5": the dot is the decimal separator.
        (Some(_), Some(_)) => mantissa.replace('.', "").replace(',', "."),
        (Some(_), None) => mantissa.to_string(),
        (None, Some(comma)) => {
            let fraction = &mantissa[comma + 1..];
            if fraction.len() == 3 && mantissa.len() > 4 {
                // "1,000" is a thousands separator.
                mantissa.replace(',', "")
            } else {
                mantissa.replace(',', ".")
            }
        }
        (None, None) => mantissa.to_string(),
    };
    let digits = if digits.is_empty() {
        "0"
    } else {
        digits.as_str()
    };
    let text = if exponent.is_empty() {
        format!("{sign}{digits}")
    } else {
        format!("{sign}{digits}e{exponent}")
    };
    text.parse::<f64>().ok().filter(|value| value.is_finite())
}

/// Whether a locale writes the decimal separator as a comma.
pub fn uses_decimal_comma(locale: &str) -> bool {
    let language = locale.split(['-', '_']).next().unwrap_or("").to_lowercase();
    matches!(
        language.as_str(),
        "de" | "es"
            | "fr"
            | "it"
            | "pt"
            | "ru"
            | "uk"
            | "nl"
            | "pl"
            | "tr"
            | "sv"
            | "da"
            | "fi"
            | "nb"
            | "cs"
            | "sk"
            | "ro"
            | "hu"
            | "el"
            | "bg"
            | "hr"
            | "lt"
            | "lv"
            | "et"
            | "is"
            | "id"
            | "vi"
    )
}

/// Format a converted value ('babel.numbers.format_decimal' with at most ten
/// decimals, without grouping).
pub fn format_number(value: f64, locale: &str) -> String {
    if !value.is_finite() {
        return String::new();
    }
    let magnitude = value.abs();
    if magnitude != 0.0 && !(1e-6..1e12).contains(&magnitude) {
        // Scientific notation, like upstream's format_scientific.
        return format!("{value:e}");
    }
    let mut text = format!("{value:.10}");
    if text.contains('.') {
        text = text.trim_end_matches('0').trim_end_matches('.').to_string();
    }
    if text.is_empty() || text == "-" {
        text = "0".to_string();
    }
    if uses_decimal_comma(locale) {
        text = text.replace('.', ",");
    }
    text
}

/// Answer a conversion query; 'None' when the query is not a conversion.
pub fn answer(query: &str, locale: &str) -> Option<String> {
    let (value, from, to) = parse_query(query)?;
    let conversion = convert(value, &from, &to)?;
    Some(format!(
        "{} {}",
        format_number(conversion.value, locale),
        conversion.unit
    ))
}

/// The plugin (upstream 'unit_converter').
pub struct UnitConverter;

impl Plugin for UnitConverter {
    fn id(&self) -> &'static str {
        "unit_converter"
    }
    fn name(&self) -> &'static str {
        "Unit converter plugin"
    }
    fn description(&self) -> &'static str {
        "Convert between units"
    }
    fn post_search(&self, response: &mut SearchResponse, context: &PluginContext) {
        // Only convert on the first page (upstream checks the pageno).
        if response.pageno > 1 {
            return;
        }
        if let Some(answer) = answer(&context.search_query, &context.locale) {
            response.answers.push(answer);
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

    fn context(query: &str, locale: &str) -> PluginContext {
        PluginContext {
            search_query: query.to_string(),
            locale: locale.to_string(),
            base_url: "http://localhost:8888".into(),
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
        let plugin = UnitConverter;
        assert_eq!(plugin.id(), "unit_converter");
        assert_eq!(plugin.name(), "Unit converter plugin");
        assert_eq!(plugin.description(), "Convert between units");
        assert!(plugin.default_enabled());
    }

    #[test]
    fn query_is_parsed_like_upstream() {
        assert_eq!(
            parse_query("10 km in miles"),
            Some((10.0, "km".to_string(), "miles".to_string()))
        );
        assert_eq!(
            parse_query("1 l AS ml"),
            Some((1.0, "l".to_string(), "ml".to_string()))
        );
        // The inch symbol is also the "in" keyword.
        assert_eq!(
            parse_query("10 in in cm"),
            Some((10.0, "in".to_string(), "cm".to_string()))
        );
        assert!(parse_query("how to cook rice").is_none());
        assert!(parse_query("10 km").is_none());
    }

    #[test]
    fn numbers_with_a_decimal_comma_are_understood() {
        assert_eq!(parse_number("1.5"), Some(1.5));
        assert_eq!(parse_number("1,5"), Some(1.5));
        assert_eq!(parse_number("1.000,5"), Some(1000.5));
        assert_eq!(parse_number("1,000.5"), Some(1000.5));
        assert_eq!(parse_number("1,000"), Some(1000.0));
        assert_eq!(parse_number("-2"), Some(-2.0));
        assert_eq!(parse_number("1e3"), Some(1000.0));
        assert!(parse_number("km").is_none());
    }

    #[test]
    fn length_weight_area_volume_and_speed_are_converted() {
        let length = convert(10.0, "km", "miles").unwrap();
        assert_eq!(length.unit, "mi");
        assert!((length.value - 6.21371192237334).abs() < 1e-9);

        let weight = convert(1.0, "kg", "lb").unwrap();
        assert_eq!(weight.unit, "lb");
        assert!((weight.value - 2.2046226218487757).abs() < 1e-9);

        let area = convert(1.0, "m2", "ft2").unwrap();
        assert!((area.value - 10.763910416709722).abs() < 1e-6);

        let volume = convert(1.0, "l", "ml").unwrap();
        assert!((volume.value - 1000.0).abs() < 1e-9);

        let speed = convert(100.0, "km/h", "mph").unwrap();
        assert_eq!(speed.unit, "mph");
        assert!((speed.value - 62.13711922373339).abs() < 1e-9);
    }

    #[test]
    fn temperature_is_converted_with_an_offset() {
        assert!((convert(100.0, "c", "f").unwrap().value - 212.0).abs() < 1e-9);
        assert!((convert(32.0, "f", "c").unwrap().value - 0.0).abs() < 1e-9);
        assert!((convert(0.0, "c", "k").unwrap().value - 273.15).abs() < 1e-9);
        // Case insensitive fallback: "100 C in F".
        assert!((convert(100.0, "C", "F").unwrap().value - 212.0).abs() < 1e-9);
    }

    #[test]
    fn ambiguous_symbols_are_resolved_by_the_dimension() {
        let mass = convert(1.0, "oz", "g").unwrap();
        assert!((mass.value - 28.349523125).abs() < 1e-9);
        let volume = convert(1.0, "oz", "ml").unwrap();
        assert!((volume.value - 29.5735295625).abs() < 1e-6);
    }

    #[test]
    fn unknown_or_incompatible_units_are_rejected() {
        assert!(convert(1.0, "xyz", "m").is_none());
        assert!(convert(1.0, "kg", "m").is_none());
        assert!(answer("10 xyz in abc", "en").is_none());
    }

    #[test]
    fn answers_are_formatted_for_the_locale() {
        assert_eq!(
            answer("10 km in miles", "en"),
            Some("6.2137119224 mi".to_string())
        );
        assert_eq!(answer("1 l in ml", "en"), Some("1000 ml".to_string()));
        assert_eq!(answer("1.5 m in cm", "de"), Some("150 cm".to_string()));
        assert_eq!(format_number(1.5, "de"), "1,5");
        assert_eq!(format_number(1.5, "en"), "1.5");
        assert_eq!(format_number(1000.0, "en"), "1000");
    }

    #[test]
    fn plugin_pushes_the_answer_only_on_the_first_page() {
        let query = "10 km in miles";
        let mut response = empty_response(query, 1);
        UnitConverter.post_search(&mut response, &context(query, "en"));
        assert_eq!(response.answers, vec!["6.2137119224 mi".to_string()]);

        let mut later_page = empty_response(query, 2);
        UnitConverter.post_search(&mut later_page, &context(query, "en"));
        assert!(later_page.answers.is_empty());
    }
}
