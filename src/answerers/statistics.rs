//! Statistics answerer, ported from upstream's 'searx/answerers/statistics.py'.
//!
//! Upstream reads the numbers with Babel ('babel.numbers.parse_decimal') and
//! formats the answer with Babel, too.  This port parses with Rust's
//! 'str::parse' (a comma is accepted as the decimal separator) and formats the
//! numbers itself, so grouping separators of a locale ("1.000,5") are not
//! understood - a documented simplification.
//!
//! The Lead declares 'pub mod answerers;' in 'src/lib.rs'.

use super::{AnswerContext, Answerer};

/// The keywords of the answerer (upstream 'kw2func').
pub const KEYWORDS: &[&str] = &["mean", "median", "mode", "stddev", "sum"];

/// Function names that are accepted in addition to the keywords.
const ALIASES: &[(&str, &str)] = &[("average", "mean"), ("avg", "mean"), ("std", "stddev")];

/// The statistics answerer.
pub struct Statistics;

impl Answerer for Statistics {
    fn id(&self) -> &'static str {
        "statistics"
    }
    fn name(&self) -> &'static str {
        "Statistics functions"
    }
    fn description(&self) -> &'static str {
        "Compute mean/median/mode/stddev/sum of the arguments"
    }
    fn keywords(&self) -> &'static [&'static str] {
        KEYWORDS
    }
    fn answer(&self, query: &str, context: &AnswerContext) -> Vec<String> {
        let Some((function, numbers)) = parse_query(query) else {
            return Vec::new();
        };
        let Some(result) = evaluate(&function, &numbers) else {
            return Vec::new();
        };
        vec![format_answer(&context.locale, &function, &numbers, result)]
    }
}

/// Resolve a function name (case insensitive, aliases included).
fn resolve_function(name: &str) -> Option<&'static str> {
    let lowered = name.to_lowercase();
    if let Some(known) = KEYWORDS.iter().find(|keyword| **keyword == lowered) {
        return Some(known);
    }
    ALIASES
        .iter()
        .find(|(alias, _)| *alias == lowered)
        .map(|(_, target)| *target)
}

/// Parse "<function> <number>..." into the function and its arguments.
pub fn parse_query(query: &str) -> Option<(String, Vec<f64>)> {
    let parts: Vec<&str> = query.split_whitespace().collect();
    if parts.len() < 2 {
        return None;
    }
    let function = resolve_function(parts[0])?;
    let mut numbers = Vec::with_capacity(parts.len() - 1);
    for part in &parts[1..] {
        numbers.push(parse_number(part)?);
    }
    Some((function.to_string(), numbers))
}

/// Parse one number; a comma is accepted as the decimal separator.
pub fn parse_number(text: &str) -> Option<f64> {
    let text = text.trim();
    text.parse::<f64>()
        .or_else(|_| text.replace(',', ".").parse::<f64>())
        .ok()
        .filter(|value| value.is_finite())
}

/// Evaluate one of the statistics functions.
pub fn evaluate(function: &str, numbers: &[f64]) -> Option<f64> {
    match function {
        "mean" => mean(numbers),
        "median" => median(numbers),
        "mode" => mode(numbers),
        "stddev" => stddev(numbers),
        "sum" => sum(numbers),
        _ => None,
    }
}

/// The arithmetic mean.
pub fn mean(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    Some(values.iter().sum::<f64>() / values.len() as f64)
}

/// The median (the mean of the two middle values for an even count).
pub fn median(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(|left, right| left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal));
    let middle = sorted.len() / 2;
    if sorted.len() % 2 == 0 {
        Some((sorted[middle - 1] + sorted[middle]) / 2.0)
    } else {
        Some(sorted[middle])
    }
}

/// The most frequent value (the smallest one wins a tie).
pub fn mode(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut counts: Vec<(f64, usize)> = Vec::new();
    for value in values {
        match counts.iter_mut().find(|(candidate, _)| candidate == value) {
            Some((_, count)) => *count += 1,
            None => counts.push((*value, 1)),
        }
    }
    counts
        .into_iter()
        .max_by(|left, right| {
            left.1.cmp(&right.1).then_with(|| {
                right
                    .0
                    .partial_cmp(&left.0)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
        })
        .map(|(value, _)| value)
}

/// The population standard deviation.
pub fn stddev(values: &[f64]) -> Option<f64> {
    let average = mean(values)?;
    let variance = values
        .iter()
        .map(|value| (value - average).powi(2))
        .sum::<f64>()
        / values.len() as f64;
    Some(variance.sqrt())
}

/// The sum of the values.
pub fn sum(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    Some(values.iter().sum())
}

/// Format the answer like upstream: "[locale] function(args) = result".
pub fn format_answer(locale: &str, function: &str, numbers: &[f64], result: f64) -> String {
    let arguments: Vec<String> = numbers
        .iter()
        .map(|value| format_number(*value, locale))
        .collect();
    format!(
        "[{locale}] {function}({}) = {}",
        arguments.join(", "),
        format_number(result, locale)
    )
}

/// Format a number with a decimal comma for the locales that use one.
pub fn format_number(value: f64, locale: &str) -> String {
    if !value.is_finite() {
        return String::new();
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

    fn context(locale: &str) -> AnswerContext {
        AnswerContext {
            locale: locale.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn plugin_metadata_matches_upstream() {
        let answerer = Statistics;
        assert_eq!(answerer.id(), "statistics");
        assert_eq!(answerer.name(), "Statistics functions");
        assert_eq!(answerer.keywords(), KEYWORDS);
        assert_eq!(answerer.info().keywords, KEYWORDS);
    }

    #[test]
    fn queries_are_parsed_into_function_and_numbers() {
        assert_eq!(
            parse_query("mean 1 2 3"),
            Some(("mean".to_string(), vec![1.0, 2.0, 3.0]))
        );
        assert_eq!(
            parse_query("AVG 1,5 2.5"),
            Some(("mean".to_string(), vec![1.5, 2.5]))
        );
        assert_eq!(
            parse_query("std 1 2"),
            Some(("stddev".to_string(), vec![1.0, 2.0]))
        );
        assert!(parse_query("mean").is_none());
        assert!(parse_query("mean one two").is_none());
        assert!(parse_query("unknown 1 2").is_none());
    }

    #[test]
    fn the_statistics_functions_are_correct() {
        let values = [1.0, 2.0, 2.0, 3.0, 4.0];
        assert_eq!(mean(&values), Some(2.4));
        assert_eq!(median(&values), Some(2.0));
        assert_eq!(mode(&values), Some(2.0));
        assert_eq!(sum(&values), Some(12.0));
        assert!((stddev(&values).unwrap() - 1.019803902718557).abs() < 1e-12);
        // median of an even count is the mean of the two middle values
        assert_eq!(median(&[1.0, 2.0, 3.0, 4.0]), Some(2.5));
        // the smallest value wins a tie of the mode
        assert_eq!(mode(&[2.0, 2.0, 1.0, 1.0]), Some(1.0));
        assert_eq!(mean(&[]), None);
        assert_eq!(mode(&[]), None);
    }

    #[test]
    fn answers_are_formatted_for_the_locale() {
        let answerer = Statistics;
        assert_eq!(
            answerer.answer("mean 1 2 3", &context("en")),
            vec!["[en] mean(1, 2, 3) = 2".to_string()]
        );
        assert_eq!(
            answerer.answer("sum 1,5 1,5", &context("de")),
            vec!["[de] sum(1,5, 1,5) = 3".to_string()]
        );
        assert!(answerer.answer("mean one two", &context("en")).is_empty());
        assert!(answerer.answer("hello 1 2", &context("en")).is_empty());
    }
}
