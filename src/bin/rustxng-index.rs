//! Crawl a seed list into the local index, and query it from the terminal.
//!
//!   rustxng-index crawl --seeds seeds.txt --out ./index-data --max-pages 2000
//!   rustxng-index search --index ./index-data "tokio runtime"
//!   rustxng-index stats --index ./index-data

use std::collections::{HashMap, HashSet, VecDeque};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rustxng::localindex::{LocalIndex, Page, has_cjk};
use scraper::{Html, Selector};

/// Skip files that are not pages.
const SKIP_EXTENSIONS: &[&str] = &[
    ".png", ".jpg", ".jpeg", ".gif", ".svg", ".webp", ".ico", ".css", ".js", ".json", ".zip",
    ".gz", ".tar", ".pdf", ".mp4", ".mp3", ".woff", ".woff2", ".ttf", ".xml", ".rss", ".txt",
];

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(command) = args.first().cloned() else {
        eprintln!("usage: rustxng-index <crawl|search|stats> [options]");
        std::process::exit(2);
    };
    let options = Options::parse(&args[1..]);
    let result = match command.as_str() {
        "crawl" => crawl(&options),
        "search" => search(&options),
        "stats" => stats(&options),
        other => {
            eprintln!("unknown command: {other}");
            std::process::exit(2);
        }
    };
    if let Err(error) = result {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

/// Long form flags, kept tiny so the binary has no extra dependencies.
struct Options {
    seeds: Option<PathBuf>,
    index: PathBuf,
    max_pages: usize,
    max_depth: usize,
    delay_ms: u64,
    workers: usize,
    use_sitemaps: bool,
    limit: usize,
    query: Vec<String>,
}

impl Options {
    fn parse(args: &[String]) -> Self {
        let mut options = Self {
            seeds: None,
            index: PathBuf::from("index-data"),
            max_pages: 500,
            max_depth: 2,
            delay_ms: 500,
            workers: 4,
            use_sitemaps: false,
            limit: 10,
            query: Vec::new(),
        };
        let mut index = 0;
        while index < args.len() {
            let arg = &args[index];
            let mut next = || {
                index += 1;
                args.get(index).cloned()
            };
            match arg.as_str() {
                "--seeds" => options.seeds = next().map(PathBuf::from),
                "--index" | "--out" => {
                    if let Some(value) = next() {
                        options.index = PathBuf::from(value);
                    }
                }
                "--max-pages" => {
                    if let Some(value) = next().and_then(|v| v.parse::<usize>().ok()) {
                        options.max_pages = value;
                    }
                }
                "--max-depth" => {
                    if let Some(value) = next().and_then(|v| v.parse::<usize>().ok()) {
                        options.max_depth = value;
                    }
                }
                "--delay-ms" => {
                    if let Some(value) = next().and_then(|v| v.parse::<u64>().ok()) {
                        options.delay_ms = value;
                    }
                }
                "--sitemaps" => options.use_sitemaps = true,
                "--workers" => {
                    if let Some(value) = next().and_then(|v| v.parse::<usize>().ok()) {
                        options.workers = value.clamp(1, 32);
                    }
                }
                "--limit" => {
                    if let Some(value) = next().and_then(|v| v.parse::<usize>().ok()) {
                        options.limit = value;
                    }
                }
                other if !other.starts_with("--") => options.query.push(other.to_string()),
                _ => {}
            }
            index += 1;
        }
        options
    }
}

fn search(options: &Options) -> Result<(), String> {
    let index = LocalIndex::open(&options.index)?;
    let query = options.query.join(" ");
    if query.is_empty() {
        return Err("no query given".to_string());
    }
    let started = Instant::now();
    let hits = index.search(&query, options.limit)?;
    let elapsed = started.elapsed();
    println!(
        "{} hits in {:.1} ms (index has {} documents)",
        hits.len(),
        elapsed.as_secs_f64() * 1000.0,
        index.document_count()
    );
    for (rank, hit) in hits.iter().enumerate() {
        println!("{:>3}. {:.3}  {}", rank + 1, hit.score, hit.url);
        if !hit.title.is_empty() {
            println!("      {}", hit.title);
        }
    }
    Ok(())
}

fn stats(options: &Options) -> Result<(), String> {
    let index = LocalIndex::open(&options.index)?;
    println!("documents: {}", index.document_count());
    Ok(())
}

/// The title of a page, falling back to the first heading.
fn page_title(html: &Html) -> String {
    for selector in ["title", "h1"] {
        if let Ok(parsed) = Selector::parse(selector) {
            if let Some(element) = html.select(&parsed).next() {
                let text = element.text().collect::<Vec<_>>().join(" ");
                let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
                if !text.is_empty() {
                    return text;
                }
            }
        }
    }
    String::new()
}

/// Absolute http(s) links found on the page.
fn links(html: &Html, base: &url::Url) -> Vec<String> {
    let Ok(selector) = Selector::parse("a[href]") else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for element in html.select(&selector) {
        let Some(href) = element.value().attr("href") else {
            continue;
        };
        let Ok(resolved) = base.join(href) else {
            continue;
        };
        if !matches!(resolved.scheme(), "http" | "https") {
            continue;
        }
        let path = resolved.path().to_lowercase();
        if SKIP_EXTENSIONS
            .iter()
            .any(|extension| path.ends_with(extension))
        {
            continue;
        }
        let mut clean = resolved.clone();
        clean.set_fragment(None);
        found.push(clean.to_string());
    }
    found
}

/// The Disallow rules that apply to everyone.
fn disallowed_paths(robots: &str) -> Vec<String> {
    let mut rules = Vec::new();
    let mut applies = false;
    for line in robots.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        // Slice by chars, not bytes: robots.txt can hold Japanese comments and
        // a byte offset would panic in the middle of a character.
        let lowered = line.to_ascii_lowercase();
        if let Some(agent) = lowered.strip_prefix("user-agent:") {
            applies = agent.trim() == "*";
        } else if applies {
            if let Some(rest) = line.strip_prefix("Disallow:") {
                let path = rest.trim();
                if !path.is_empty() {
                    rules.push(path.to_string());
                }
            }
        }
    }
    rules
}

/// What a worker hands back to the writer.
struct Fetched {
    url: String,
    title: String,
    body: String,
    links: Vec<String>,
    depth: usize,
}

/// Shared crawl state. Workers pull URLs, the main thread writes documents.
struct Crawl {
    queue: Mutex<VecDeque<(String, usize)>>,
    seen: Mutex<HashSet<String>>,
    robots: Mutex<HashMap<String, Vec<String>>>,
    /// Earliest time the next request to a host may start.
    next_slot: Mutex<HashMap<String, Instant>>,
    in_flight: AtomicUsize,
}

impl Crawl {
    fn claim(&self) -> Option<(String, usize)> {
        let mut queue = self.queue.lock().ok()?;
        while let Some((url, depth)) = queue.pop_front() {
            let fresh = self
                .seen
                .lock()
                .map(|mut seen| seen.insert(url.clone()))
                .unwrap_or(false);
            if fresh {
                return Some((url, depth));
            }
        }
        None
    }

    /// Wait until this host may be asked again, so several workers still keep
    /// one request per delay per host.
    fn wait_for_host(&self, host: &str, delay: Duration) {
        loop {
            let slot = {
                let Ok(mut slots) = self.next_slot.lock() else {
                    return;
                };
                let now = Instant::now();
                let slot = slots.entry(host.to_string()).or_insert(now);
                if *slot <= now {
                    *slot = now + delay;
                    return;
                }
                *slot
            };
            std::thread::sleep(slot.saturating_duration_since(Instant::now()));
        }
    }
}

/// URLs listed in a sitemap (following a sitemap index up to two levels).
fn sitemap_urls(
    client: &reqwest::blocking::Client,
    url: &str,
    depth: usize,
    limit: usize,
    out: &mut Vec<String>,
) {
    if depth > 2 || out.len() >= limit {
        return;
    }
    let Ok(response) = client.get(url).send() else {
        return;
    };
    let Ok(body) = response.text() else {
        return;
    };
    let is_index = body.contains("<sitemapindex");
    let mut found = 0usize;
    for entry in body.match_indices("<loc>") {
        let rest = &body[entry.0 + 5..];
        let Some(end) = rest.find("</loc>") else {
            continue;
        };
        let raw = rest[..end].trim();
        let loc = raw
            .replace("&amp;", "&")
            .replace("&lt;", "<")
            .replace("&gt;", ">");
        if !loc.starts_with("http") {
            continue;
        }
        found += 1;
        if is_index {
            sitemap_urls(client, &loc, depth + 1, limit, out);
        } else {
            out.push(loc);
        }
        if out.len() >= limit || found > 5_000 {
            return;
        }
    }
}

/// Sitemaps a host advertises in its robots.txt.
fn robots_sitemaps(client: &reqwest::blocking::Client, base: &str) -> Vec<String> {
    let Ok(parsed) = url::Url::parse(base) else {
        return Vec::new();
    };
    let Some(host) = parsed.host_str() else {
        return Vec::new();
    };
    let robots = format!("{}://{host}/robots.txt", parsed.scheme());
    let Ok(response) = client.get(&robots).send() else {
        return Vec::new();
    };
    let Ok(body) = response.text() else {
        return Vec::new();
    };
    body.lines()
        .filter_map(|line| {
            let line = line.split('#').next().unwrap_or("").trim();
            let rest = line.strip_prefix("Sitemap:")?.trim();
            rest.starts_with("http").then(|| rest.to_string())
        })
        .collect()
}

fn crawl(options: &Options) -> Result<(), String> {
    let seeds_path = options.seeds.as_ref().ok_or("crawl needs --seeds <file>")?;
    let seeds = std::fs::read_to_string(seeds_path)
        .map_err(|error| format!("read {seeds_path:?}: {error}"))?;
    let mut queue: VecDeque<(String, usize)> = VecDeque::new();
    for line in seeds.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        queue.push_back((line.to_string(), 0));
    }
    if queue.is_empty() {
        return Err("no seeds".to_string());
    }

    // Site roots advertise their sitemaps in robots.txt. Following them is
    // far more thorough than walking links, and it is what lets the index
    // cover a documentation site completely.
    if options.use_sitemaps {
        let client = reqwest::blocking::Client::builder()
            .user_agent("rustxng-index/0.1 (+https://github.com/sahenjp/RustXNG)")
            .timeout(Duration::from_secs(20))
            .build()
            .map_err(|error| format!("client: {error}"))?;
        let mut expanded: Vec<String> = Vec::new();
        for (url, _) in queue.iter() {
            for sitemap in robots_sitemaps(&client, url) {
                sitemap_urls(&client, &sitemap, 0, options.max_pages, &mut expanded);
            }
            if expanded.len() >= options.max_pages {
                break;
            }
        }
        if !expanded.is_empty() {
            println!("sitemaps added {} urls", expanded.len());
            let _ = std::io::stdout().flush();
            for url in expanded {
                queue.push_back((url, 0));
            }
        }
    }

    let index = LocalIndex::open_for_write(&options.index)?;
    let crawl = Arc::new(Crawl {
        queue: Mutex::new(queue),
        seen: Mutex::new(HashSet::new()),
        robots: Mutex::new(HashMap::new()),
        next_slot: Mutex::new(HashMap::new()),
        in_flight: AtomicUsize::new(0),
    });
    let (sender, receiver) = std::sync::mpsc::channel::<Fetched>();
    let delay = Duration::from_millis(options.delay_ms);
    let max_depth = options.max_depth;

    for _ in 0..options.workers {
        let crawl = Arc::clone(&crawl);
        let sender = sender.clone();
        std::thread::spawn(move || {
            let client = match reqwest::blocking::Client::builder()
                .user_agent("rustxng-index/0.1 (+https://github.com/sahenjp/RustXNG)")
                .timeout(Duration::from_secs(20))
                .build()
            {
                Ok(client) => client,
                Err(_) => return,
            };
            loop {
                let Some((url, depth)) = crawl.claim() else {
                    if crawl.in_flight.load(Ordering::SeqCst) == 0 {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                    continue;
                };
                let Ok(parsed) = url::Url::parse(&url) else {
                    continue;
                };
                let Some(host) = parsed.host_str().map(str::to_string) else {
                    continue;
                };
                let rules = {
                    let cached = crawl
                        .robots
                        .lock()
                        .ok()
                        .and_then(|robots| robots.get(&host).cloned());
                    match cached {
                        Some(rules) => rules,
                        None => {
                            let robots_url = format!("{}://{host}/robots.txt", parsed.scheme());
                            let rules = client
                                .get(&robots_url)
                                .send()
                                .ok()
                                .and_then(|response| response.text().ok())
                                .map(|body| disallowed_paths(&body))
                                .unwrap_or_default();
                            if let Ok(mut robots) = crawl.robots.lock() {
                                robots.insert(host.clone(), rules.clone());
                            }
                            rules
                        }
                    }
                };
                if rules
                    .iter()
                    .any(|rule| parsed.path().starts_with(rule.trim_end_matches('*')))
                {
                    continue;
                }

                crawl.wait_for_host(&host, delay);
                crawl.in_flight.fetch_add(1, Ordering::SeqCst);
                let fetched = fetch(&client, &url, depth, max_depth);
                crawl.in_flight.fetch_sub(1, Ordering::SeqCst);
                if let Some(page) = fetched {
                    if sender.send(page).is_err() {
                        return;
                    }
                }
            }
        });
    }
    drop(sender);

    let mut indexed = 0usize;
    for page in receiver {
        if indexed >= options.max_pages {
            break;
        }
        if index.has_url(&page.url) {
            continue;
        }
        let lang = if has_cjk(&page.title) || has_cjk(&page.body) {
            "ja"
        } else {
            "en"
        };
        index.add(&Page {
            url: &page.url,
            title: &page.title,
            body: &page.body,
            lang,
        })?;
        indexed += 1;
        if page.depth < max_depth {
            if let Ok(mut queue) = crawl.queue.lock() {
                for link in page.links {
                    queue.push_back((link, page.depth + 1));
                }
            }
        }
        if indexed % 50 == 0 {
            index.commit()?;
            let queued = crawl.queue.lock().map(|q| q.len()).unwrap_or(0);
            println!("indexed {indexed} pages (queue {queued})");
            let _ = std::io::stdout().flush();
        }
    }

    index.commit()?;
    println!(
        "crawled {indexed} pages, index now has {} documents",
        index.document_count()
    );
    Ok(())
}

/// Fetch one page and extract what the index needs.
fn fetch(
    client: &reqwest::blocking::Client,
    url: &str,
    depth: usize,
    max_depth: usize,
) -> Option<Fetched> {
    let response = client.get(url).send().ok()?;
    if !response.status().is_success() {
        return None;
    }
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_string();
    if !content_type.contains("text/html") {
        return None;
    }
    let body = response.text().ok()?;
    let html = Html::parse_document(&body);
    let title = page_title(&html);
    let text = rustxng::content::main_text_limited(&html, 20_000);
    if title.is_empty() && text.is_empty() {
        return None;
    }
    let base = url::Url::parse(url).ok()?;
    let host = base.host_str()?.to_string();
    let links = if depth < max_depth {
        links(&html, &base)
            .into_iter()
            .filter(|link| {
                url::Url::parse(link)
                    .map(|parsed| parsed.host_str() == Some(host.as_str()))
                    .unwrap_or(false)
            })
            .collect()
    } else {
        Vec::new()
    };
    Some(Fetched {
        url: url.to_string(),
        title,
        body: text,
        links,
        depth,
    })
}
