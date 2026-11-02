//! Instant answerers, ported from upstream's 'searx/answerers/'.
//!
//! An answerer is matched by the *first* keyword of the query (upstream
//! 'AnswerStorage.ask') and answers a list of plain strings that the search
//! response shows in the answer area.
//!
//! Upstream answerers are synchronous; the weather answerer of this port has to
//! fetch the Open-Meteo API, so the trait carries both a synchronous 'answer()'
//! and an 'answer_async()' (whose default implementation wraps 'answer()').
//! 'answer_async()' returns a boxed future, which keeps 'dyn Answerer' object
//! safe (an 'async fn' in a trait is not dyn compatible).
//!
//! The Lead declares 'pub mod answerers;' in 'src/lib.rs' and calls
//! 'answerers::all()' / 'answerers::ask()' from the search path (the
//! 'PluginContext' of the plugins carries the same locale and base URL).

pub mod random;
pub mod statistics;
pub mod weather;

use std::sync::Arc;

use futures::future::BoxFuture;

use crate::network::Network;

/// The context handed to an answerer.
#[derive(Clone, Default)]
pub struct AnswerContext {
    /// The UI locale, used to format numbers.
    pub locale: String,
    /// The instance base URL.
    pub base_url: String,
    /// The HTTP client; answerers that need the network (weather) require it.
    pub network: Option<Arc<Network>>,
}

/// Metadata of an answerer (upstream 'AnswererInfo').
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnswererInfo {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub keywords: &'static [&'static str],
}

/// The answerer interface (upstream 'searx.answerers.Answerer').
pub trait Answerer: Send + Sync {
    /// The module name (upstream file / class name).
    fn id(&self) -> &'static str;
    fn name(&self) -> &'static str {
        ""
    }
    fn description(&self) -> &'static str {
        ""
    }
    /// Keywords that select the answerer (the first query word is matched).
    fn keywords(&self) -> &'static [&'static str];
    /// The synchronous answer; empty when the query is not for this answerer.
    fn answer(&self, _query: &str, _context: &AnswerContext) -> Vec<String> {
        Vec::new()
    }
    /// The asynchronous answer.  The default implementation uses 'answer()';
    /// answerers that need the network override this one.
    fn answer_async<'a>(
        &'a self,
        query: &'a str,
        context: &'a AnswerContext,
    ) -> BoxFuture<'a, Vec<String>> {
        Box::pin(std::future::ready(self.answer(query, context)))
    }
    /// The information shown on the about / preferences pages.
    fn info(&self) -> AnswererInfo {
        AnswererInfo {
            id: self.id(),
            name: self.name(),
            description: self.description(),
            keywords: self.keywords(),
        }
    }
}

/// Every bundled answerer, in the order they run.
pub fn all() -> Vec<Box<dyn Answerer>> {
    vec![
        Box::new(statistics::Statistics),
        Box::new(random::Random),
        Box::new(weather::Weather),
    ]
}

/// The first word of the query (upstream's keyword lookup).
pub fn first_keyword(query: &str) -> Option<&str> {
    query.split_whitespace().next()
}

/// The answerers that answer a keyword.
pub fn matching<'a>(answerers: &'a [Box<dyn Answerer>], keyword: &str) -> Vec<&'a dyn Answerer> {
    answerers
        .iter()
        .filter(|answerer| answerer.keywords().contains(&keyword))
        .map(|answerer| answerer.as_ref())
        .collect()
}

/// Ask every answerer that matches the first keyword of the query (asynchronous,
/// needed for the weather answerer).
pub async fn ask(
    answerers: &[Box<dyn Answerer>],
    query: &str,
    context: &AnswerContext,
) -> Vec<String> {
    let Some(keyword) = first_keyword(query) else {
        return Vec::new();
    };
    let mut answers = Vec::new();
    for answerer in matching(answerers, keyword) {
        answers.extend(answerer.answer_async(query, context).await);
    }
    answers
}

/// Ask the synchronous answerers (statistics, random).
pub fn ask_sync(
    answerers: &[Box<dyn Answerer>],
    query: &str,
    context: &AnswerContext,
) -> Vec<String> {
    let Some(keyword) = first_keyword(query) else {
        return Vec::new();
    };
    let mut answers = Vec::new();
    for answerer in matching(answerers, keyword) {
        answers.extend(answerer.answer(query, context));
    }
    answers
}

/// The information of every bundled answerer (about page).
pub fn infos() -> Vec<AnswererInfo> {
    all().iter().map(|answerer| answerer.info()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Echo;

    impl Answerer for Echo {
        fn id(&self) -> &'static str {
            "echo"
        }
        fn name(&self) -> &'static str {
            "Echo"
        }
        fn description(&self) -> &'static str {
            "Answers with the query"
        }
        fn keywords(&self) -> &'static [&'static str] {
            &["echo", "repeat"]
        }
        fn answer(&self, query: &str, _context: &AnswerContext) -> Vec<String> {
            vec![query.to_string()]
        }
    }

    #[test]
    fn the_first_word_selects_the_answerer() {
        assert_eq!(first_keyword("  mean 1 2 3"), Some("mean"));
        assert_eq!(first_keyword(""), None);
        let answerers: Vec<Box<dyn Answerer>> = vec![Box::new(Echo)];
        assert_eq!(matching(&answerers, "echo").len(), 1);
        assert_eq!(matching(&answerers, "repeat").len(), 1);
        assert!(matching(&answerers, "sum").is_empty());
    }

    #[test]
    fn ask_dispatches_after_the_keyword() {
        let answerers: Vec<Box<dyn Answerer>> = vec![Box::new(Echo)];
        let context = AnswerContext::default();
        assert_eq!(
            ask_sync(&answerers, "echo hello world", &context),
            vec!["echo hello world".to_string()]
        );
        assert!(ask_sync(&answerers, "sum 1 2", &context).is_empty());
        assert!(ask_sync(&answerers, "", &context).is_empty());
    }

    #[test]
    fn all_answerers_are_listed_with_their_keywords() {
        let answerers = all();
        let ids: Vec<&str> = answerers.iter().map(|answerer| answerer.id()).collect();
        assert_eq!(ids, vec!["statistics", "random", "weather"]);
        for info in infos() {
            assert!(!info.keywords.is_empty(), "{} has no keywords", info.id);
            assert!(!info.name.is_empty());
        }
    }

    #[test]
    fn answer_async_defaults_to_the_synchronous_answer() {
        let answerers: Vec<Box<dyn Answerer>> = vec![Box::new(Echo)];
        let context = AnswerContext::default();
        let answers = futures::executor::block_on(ask(&answerers, "echo hi", &context));
        assert_eq!(answers, vec!["echo hi".to_string()]);
    }
}
