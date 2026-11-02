//! Weather answerer, ported from the Open-Meteo engine of upstream
//! ('searx/engines/open_meteo.py') and the weather helpers of 'searx/weather.py'.
//!
//! A query "weather <place>" resolves the place with the Open-Meteo geocoding
//! API and answers with the current temperature, the condition (the WMO weather
//! code mapped onto the condition names of upstream) and the wind speed from
//! '\''api.open-meteo.com'\''.
//!
//! Simplifications of this port: only the *current* values are read (upstream
//! also renders an hourly forecast), the timezone/locale specific formatting of
//! Babel is replaced by a decimal comma for the locales that use one, and the
//! answer is a single line instead of upstream's 'WeatherAnswer' structure.
//!
//! The Lead declares 'pub mod answerers;' in 'src/lib.rs' and gives the
//! answerer an 'AnswerContext' with 'network: Some(searcher.network.clone())'.

use std::time::Duration;

use futures::future::BoxFuture;
use serde_json::Value;

use super::{AnswerContext, Answerer};
use crate::engine::EngineRequest;
use crate::engine::util::query_string;
use crate::network::Network;

/// The keyword that triggers the answerer.
pub const KEYWORDS: &[&str] = &["weather"];

/// The geocoding endpoint (upstream 'geo_url').
pub const GEOCODING_URL: &str = "https://geocoding-api.open-meteo.com/v1/search";

/// The forecast endpoint (upstream 'api_url').
pub const FORECAST_URL: &str = "https://api.open-meteo.com/v1/forecast";

/// The current weather fields searched (upstream 'data_of_interest').
pub static CURRENT_FIELDS: &[&str] = &[
    "temperature_2m",
    "apparent_temperature",
    "relative_humidity_2m",
    "cloud_cover",
    "pressure_msl",
    "wind_speed_10m",
    "wind_direction_10m",
    "weather_code",
];

/// Timeout of both requests.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

/// A place resolved by the geocoding API (upstream 'weather.GeoLocation').
#[derive(Debug, Clone, PartialEq)]
pub struct Place {
    pub name: String,
    pub country: String,
    pub latitude: f64,
    pub longitude: f64,
    pub timezone: String,
}

/// The current weather of a place (a subset of upstream 'WeatherAnswer.Item').
#[derive(Debug, Clone, PartialEq)]
pub struct CurrentWeather {
    pub temperature: f64,
    pub apparent_temperature: Option<f64>,
    pub humidity: Option<f64>,
    pub wind_speed: Option<f64>,
    pub wind_direction: Option<f64>,
    pub pressure: Option<f64>,
    pub weather_code: u32,
}

/// The weather answerer.
pub struct Weather;

impl Answerer for Weather {
    fn id(&self) -> &'static str {
        "weather"
    }
    fn name(&self) -> &'static str {
        "Weather"
    }
    fn description(&self) -> &'static str {
        "Current weather of a place, from the Open-Meteo API"
    }
    fn keywords(&self) -> &'static [&'static str] {
        KEYWORDS
    }
    fn answer_async<'a>(
        &'a self,
        query: &'a str,
        context: &'a AnswerContext,
    ) -> BoxFuture<'a, Vec<String>> {
        Box::pin(async move {
            // The network is required; without it the answerer stays silent.
            let Some(network) = context.network.clone() else {
                return Vec::new();
            };
            let place = query
                .split_whitespace()
                .skip(1)
                .collect::<Vec<&str>>()
                .join(" ");
            if place.is_empty() {
                return Vec::new();
            }
            let locale = if context.locale.is_empty() {
                "en"
            } else {
                context.locale.as_str()
            };
            match weather_for(&network, &place, locale).await {
                Some(answer) => vec![answer],
                None => Vec::new(),
            }
        })
    }
}

/// The answer for a place: "Berlin: 21.4 °C, partly cloudy, wind 12.3 km/h".
pub async fn weather_for(network: &Network, place: &str, locale: &str) -> Option<String> {
    let place = place.trim();
    if place.is_empty() {
        return None;
    }
    let language = crate::engine::util::locale_language(locale);
    let language = if language.is_empty() || language == "all" {
        "en".to_string()
    } else {
        language
    };
    let geocoding = fetch_json(network, &geocoding_url(place, &language)).await?;
    let location = parse_place(&geocoding)?;
    let forecast = fetch_json(network, &forecast_url(&location)).await?;
    let current = parse_current(&forecast)?;
    Some(format_answer(&location, &current, locale))
}

async fn fetch_json(network: &Network, url: &str) -> Option<Value> {
    let request = EngineRequest::get(url).header("Accept", "application/json");
    let response = network
        .fetch(request, REQUEST_TIMEOUT, None, None)
        .await
        .ok()?;
    response.json().ok()
}

/// The geocoding URL of a place (upstream 'GeoLocation._query_open_meteo').
pub fn geocoding_url(place: &str, language: &str) -> String {
    let pairs: Vec<(&str, String)> = vec![
        ("name", place.to_string()),
        ("count", "1".to_string()),
        ("language", language.to_string()),
        ("format", "json".to_string()),
    ];
    format!("{GEOCODING_URL}?{}", query_string(&pairs))
}

/// The forecast URL of a place (upstream 'request()').
pub fn forecast_url(place: &Place) -> String {
    let pairs: Vec<(&str, String)> = vec![
        ("latitude", place.latitude.to_string()),
        ("longitude", place.longitude.to_string()),
        ("current", CURRENT_FIELDS.join(",")),
        ("timezone", "auto".to_string()),
        ("format", "json".to_string()),
    ];
    format!("{FORECAST_URL}?{}", query_string(&pairs))
}

/// The first result of the geocoding answer (upstream uses 'results[0]').
pub fn parse_place(document: &Value) -> Option<Place> {
    let result = document.get("results")?.as_array()?.first()?;
    let name = result.get("name")?.as_str()?.to_string();
    let latitude = result.get("latitude")?.as_f64()?;
    let longitude = result.get("longitude")?.as_f64()?;
    let country = result
        .get("country_code")
        .and_then(Value::as_str)
        .or_else(|| result.get("country").and_then(Value::as_str))
        .unwrap_or_default()
        .to_string();
    let timezone = result
        .get("timezone")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    Some(Place {
        name,
        country,
        latitude,
        longitude,
        timezone,
    })
}

/// The 'current' block of the forecast answer.
pub fn parse_current(document: &Value) -> Option<CurrentWeather> {
    let current = document.get("current")?;
    let temperature = current.get("temperature_2m")?.as_f64()?;
    let weather_code = current
        .get("weather_code")
        .and_then(|value| value.as_u64().or_else(|| value.as_f64().map(|f| f as u64)))?
        as u32;
    Some(CurrentWeather {
        temperature,
        apparent_temperature: number(current, "apparent_temperature"),
        humidity: number(current, "relative_humidity_2m"),
        wind_speed: number(current, "wind_speed_10m"),
        wind_direction: number(current, "wind_direction_10m"),
        pressure: number(current, "pressure_msl"),
        weather_code,
    })
}

fn number(value: &Value, field: &str) -> Option<f64> {
    value.get(field).and_then(Value::as_f64)
}

/// The WMO weather code as a condition name (upstream 'WMO_TO_CONDITION').
pub fn condition(code: u32) -> &'static str {
    match code {
        0 => "clear sky",
        1 => "fair",
        2 => "partly cloudy",
        3 => "cloudy",
        45 | 48 => "fog",
        51 | 53 | 55 => "light rain",
        56 => "light sleet showers",
        57 => "light sleet",
        61 => "light rain",
        63 => "rain",
        65 => "heavy rain",
        66 => "light sleet showers",
        67 => "light sleet",
        71 => "light sleet",
        73 => "sleet",
        75 => "heavy sleet",
        77 => "snow",
        80 => "light rain showers",
        81 => "rain showers",
        82 => "heavy rain showers",
        85 => "snow showers",
        86 => "heavy snow showers",
        95 => "rain and thunder",
        96 => "light snow and thunder",
        99 => "heavy snow and thunder",
        _ => "unknown",
    }
}

/// The one line answer.
pub fn format_answer(place: &Place, current: &CurrentWeather, locale: &str) -> String {
    let mut parts = vec![
        format!("{} \u{00b0}C", format_number(current.temperature, locale)),
        condition(current.weather_code).to_string(),
    ];
    if let Some(speed) = current.wind_speed {
        parts.push(format!("wind {} km/h", format_number(speed, locale)));
    }
    format!("{}: {}", place.name, parts.join(", "))
}

/// Format a number with at most one decimal and a decimal comma where the
/// locale uses one.
pub fn format_number(value: f64, locale: &str) -> String {
    let mut text = format!("{value:.1}");
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

fn uses_decimal_comma(locale: &str) -> bool {
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const GEOCODING_FIXTURE: &str = r#"{
        "results": [
            {
                "id": 2950159,
                "name": "Berlin",
                "latitude": 52.52437,
                "longitude": 13.41053,
                "elevation": 74.0,
                "country_code": "DE",
                "timezone": "Europe/Berlin",
                "country": "Deutschland",
                "admin1": "Berlin"
            }
        ],
        "generationtime_ms": 0.5
    }"#;

    const FORECAST_FIXTURE: &str = r#"{
        "latitude": 52.52,
        "longitude": 13.419998,
        "timezone": "Europe/Berlin",
        "current": {
            "time": 1714550400,
            "interval": 900,
            "temperature_2m": 21.4,
            "apparent_temperature": 20.1,
            "relative_humidity_2m": 53,
            "cloud_cover": 40,
            "pressure_msl": 1013.2,
            "wind_speed_10m": 12.3,
            "wind_direction_10m": 240,
            "weather_code": 2
        }
    }"#;

    fn context() -> AnswerContext {
        AnswerContext {
            locale: "en".to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn plugin_metadata_matches_upstream() {
        let answerer = Weather;
        assert_eq!(answerer.id(), "weather");
        assert_eq!(answerer.name(), "Weather");
        assert_eq!(answerer.keywords(), KEYWORDS);
    }

    #[test]
    fn the_geocoding_answer_is_parsed() {
        let document: Value = serde_json::from_str(GEOCODING_FIXTURE).unwrap();
        let place = parse_place(&document).expect("place");
        assert_eq!(place.name, "Berlin");
        assert_eq!(place.country, "DE");
        assert_eq!(place.latitude, 52.52437);
        assert_eq!(place.longitude, 13.41053);
        assert_eq!(place.timezone, "Europe/Berlin");

        assert!(parse_place(&json!({"results": []})).is_none());
        assert!(parse_place(&json!({})).is_none());
        assert!(parse_place(&json!({"results": [{"name": "X"}]})).is_none());
    }

    #[test]
    fn the_current_weather_is_parsed() {
        let document: Value = serde_json::from_str(FORECAST_FIXTURE).unwrap();
        let current = parse_current(&document).expect("current");
        assert_eq!(current.temperature, 21.4);
        assert_eq!(current.apparent_temperature, Some(20.1));
        assert_eq!(current.humidity, Some(53.0));
        assert_eq!(current.wind_speed, Some(12.3));
        assert_eq!(current.wind_direction, Some(240.0));
        assert_eq!(current.pressure, Some(1013.2));
        assert_eq!(current.weather_code, 2);
        assert_eq!(condition(current.weather_code), "partly cloudy");

        assert!(parse_current(&json!({"current": {"weather_code": 0}})).is_none());
        assert!(parse_current(&json!({})).is_none());
    }

    #[test]
    fn wmo_codes_map_onto_the_upstream_conditions() {
        assert_eq!(condition(0), "clear sky");
        assert_eq!(condition(1), "fair");
        assert_eq!(condition(2), "partly cloudy");
        assert_eq!(condition(3), "cloudy");
        assert_eq!(condition(45), "fog");
        assert_eq!(condition(61), "light rain");
        assert_eq!(condition(75), "heavy sleet");
        assert_eq!(condition(95), "rain and thunder");
        assert_eq!(condition(1234), "unknown");
    }

    #[test]
    fn the_answer_contains_temperature_condition_and_wind() {
        let document: Value = serde_json::from_str(FORECAST_FIXTURE).unwrap();
        let place = parse_place(&serde_json::from_str(GEOCODING_FIXTURE).unwrap()).unwrap();
        let current = parse_current(&document).unwrap();
        let answer = format_answer(&place, &current, "en");
        assert_eq!(
            answer,
            "Berlin: 21.4 \u{00b0}C, partly cloudy, wind 12.3 km/h"
        );
        let german = format_answer(&place, &current, "de-DE");
        assert!(german.contains("21,4 \u{00b0}C"), "{german}");
        assert!(german.contains("wind 12,3 km/h"), "{german}");
    }

    #[test]
    fn missing_wind_is_left_out_of_the_answer() {
        let place = Place {
            name: "Berlin".to_string(),
            country: "DE".to_string(),
            latitude: 52.5,
            longitude: 13.4,
            timezone: "Europe/Berlin".to_string(),
        };
        let current = CurrentWeather {
            temperature: 0.0,
            apparent_temperature: None,
            humidity: None,
            wind_speed: None,
            wind_direction: None,
            pressure: None,
            weather_code: 0,
        };
        assert_eq!(
            format_answer(&place, &current, "en"),
            "Berlin: 0 \u{00b0}C, clear sky"
        );
    }

    #[test]
    fn the_request_urls_are_built_like_upstream() {
        let url = geocoding_url("New York", "en");
        assert!(url.starts_with(GEOCODING_URL));
        assert!(url.contains("name=New%20York"), "{url}");
        assert!(url.contains("count=1"));
        assert!(url.contains("format=json"));

        let place = parse_place(&serde_json::from_str(GEOCODING_FIXTURE).unwrap()).unwrap();
        let url = forecast_url(&place);
        assert!(url.starts_with(FORECAST_URL));
        assert!(url.contains("latitude=52.52437"));
        assert!(url.contains("longitude=13.41053"));
        assert!(url.contains("timezone=auto"));
        assert!(url.contains("weather_code"), "{url}");
    }

    #[test]
    fn the_answerer_needs_a_place_and_a_network() {
        let answerer = Weather;
        // Without a network the answerer answers nothing (no network in tests).
        let answers =
            futures::executor::block_on(answerer.answer_async("weather Berlin", &context()));
        assert!(answers.is_empty());
        let answers = futures::executor::block_on(answerer.answer_async("weather", &context()));
        assert!(answers.is_empty());
        // The synchronous entry point has no answer either.
        assert!(answerer.answer("weather Berlin", &context()).is_empty());
    }
}
