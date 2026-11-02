//! Latency benchmark for rustxng.
//!
//! The binary boots the real application in process, drives it over HTTP like a
//! client would and reports the latency distribution per query and per engine.
//!
//! \`\`\`text
//! cargo run --release --example benchmark -- --repeats 2 --engines wikipedia,hackernews
//! \`\`\`
//!
//! The same queries can be replayed against a reference SearXNG instance with
//! \`bench/compare.py\`, which also compares the result sets.

use std::net::SocketAddr;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use rustxng::aggregator::Searcher;
use rustxng::settings::Settings;
use rustxng::web::{AppState, router};

const DEFAULT_QUERIES: &[&str] = &[
    "rust programming language",
    "large language models",
    "openstreetmap berlin",
    "tokio async runtime",
    "climate change arctic",
];

struct Args {
    repeats: usize,
    queries: Vec<String>,
    engines: Vec<String>,
    port: u16,
    json: bool,
    timeout: f64,
    time_range: String,
    engines_joined: String,
}

fn parse_args() -> Result<Option<Args>, String> {
    let mut args = Args {
        repeats: 2,
        queries: DEFAULT_QUERIES
            .iter()
            .map(|query| (*query).to_string())
            .collect(),
        engines: Vec::new(),
        port: 0,
        json: false,
        timeout: 0.0,
        time_range: String::new(),
        engines_joined: String::new(),
    };
    let mut argv = std::env::args().skip(1);
    while let Some(argument) = argv.next() {
        match argument.as_str() {
            "-h" | "--help" => {
                println!(
                    "usage: benchmark [--repeats N] [--queries \"a,b\"] [--engines \"e1,e2\"] [--time-range day|week|month|year] [--port N] [--timeout SECONDS] [--json]"
                );
                return Ok(None);
            }
            "--repeats" => {
                args.repeats = argv
                    .next()
                    .ok_or("--repeats needs a number")?
                    .parse()
                    .map_err(|_| "--repeats needs a number")?
            }
            "--queries" => {
                args.queries = argv
                    .next()
                    .ok_or("--queries needs a list")?
                    .split(',')
                    .map(|query| query.trim().to_string())
                    .filter(|query| !query.is_empty())
                    .collect()
            }
            "--engines" => {
                args.engines = argv
                    .next()
                    .ok_or("--engines needs a list")?
                    .split(',')
                    .map(|engine| engine.trim().to_string())
                    .filter(|engine| !engine.is_empty())
                    .collect();
                args.engines_joined = args.engines.join(",");
            }
            "--port" => {
                args.port = argv
                    .next()
                    .ok_or("--port needs a number")?
                    .parse()
                    .map_err(|_| "--port needs a number")?
            }
            "--timeout" => {
                args.timeout = argv
                    .next()
                    .ok_or("--timeout needs a number")?
                    .parse()
                    .map_err(|_| "--timeout needs a number")?
            }
            "--time-range" => {
                args.time_range = argv.next().ok_or("--time-range needs a value")?;
            }
            "--json" => args.json = true,
            other => return Err(format!("unknown argument {other:?} (try --help)")),
        }
    }
    Ok(Some(args))
}

struct Sample {
    query: String,
    repeat: usize,
    millis: f64,
    results: usize,
    engines: Vec<String>,
    unresponsive: Vec<String>,
    timings: Vec<(String, f64)>,
}

fn stats(values: &[f64]) -> (f64, f64, f64, f64) {
    if values.is_empty() {
        return (0.0, 0.0, 0.0, 0.0);
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mean = sorted.iter().sum::<f64>() / sorted.len() as f64;
    let median = sorted[sorted.len() / 2];
    let p95_index = ((sorted.len() as f64 * 0.95).ceil() as usize)
        .saturating_sub(1)
        .min(sorted.len() - 1);
    (
        mean,
        median,
        sorted[p95_index],
        *sorted.last().unwrap_or(&0.0),
    )
}

#[tokio::main]
async fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(Some(args)) => args,
        Ok(None) => return ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            return ExitCode::from(2);
        }
    };

    let mut settings = Settings::defaults();
    if !args.engines.is_empty() {
        settings.engines.retain(|engine| {
            args.engines
                .iter()
                .any(|wanted| wanted == &engine.name || wanted == &engine.engine)
        });
    }
    settings.search.autocomplete = "none".to_string();
    if args.timeout > 0.0 {
        settings.search.timeout_limit = args.timeout;
        settings.outgoing.max_request_timeout =
            args.timeout.max(settings.outgoing.max_request_timeout);
    }

    let searcher = match Searcher::new(settings) {
        Ok(searcher) => searcher,
        Err(error) => {
            eprintln!("error: {error}");
            return ExitCode::FAILURE;
        }
    };
    let engine_count = searcher.registry.len();
    let app = router(AppState::new(searcher));
    let listener = match tokio::net::TcpListener::bind(("127.0.0.1", args.port)).await {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("error: could not bind: {error}");
            return ExitCode::FAILURE;
        }
    };
    let address: SocketAddr = listener.local_addr().expect("local address");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .build()
        .expect("http client");

    // Baseline: the cost of the HTTP stack without any engine.
    let mut baseline = Vec::new();
    for _ in 0..5 {
        let started = Instant::now();
        let _ = client
            .get(format!("http://{address}/healthz"))
            .send()
            .await
            .expect("healthz");
        baseline.push(started.elapsed().as_secs_f64() * 1000.0);
    }
    let (base_mean, _, _, _) = stats(&baseline);

    let mut samples = Vec::new();
    for query in &args.queries {
        for repeat in 0..args.repeats {
            // full=1 keeps the engine timings in the JSON document
            let mut params = vec![("q", query.as_str()), ("format", "json"), ("full", "1")];
            if !args.time_range.is_empty() {
                params.push(("time_range", args.time_range.as_str()));
            }
            if !args.engines.is_empty() {
                // Without this the request would only search the "general"
                // category, which excludes e.g. the news engines.
                params.push(("engines", args.engines_joined.as_str()));
            }
            let url = reqwest::Url::parse_with_params(&format!("http://{address}/search"), &params)
                .expect("url");
            let started = Instant::now();
            let response = match client.get(url).send().await {
                Ok(response) => response,
                Err(error) => {
                    eprintln!("error: {query:?}: {error}");
                    continue;
                }
            };
            let millis = started.elapsed().as_secs_f64() * 1000.0;
            let body: serde_json::Value = match response.json().await {
                Ok(body) => body,
                Err(error) => {
                    eprintln!("error: {query:?}: {error}");
                    continue;
                }
            };
            let timings = body["timings"]
                .as_array()
                .map(|timings| {
                    timings
                        .iter()
                        .map(|timing| {
                            (
                                timing["engine"].as_str().unwrap_or_default().to_string(),
                                timing["total"].as_f64().unwrap_or(0.0) * 1000.0,
                            )
                        })
                        .collect()
                })
                .unwrap_or_default();
            samples.push(Sample {
                query: query.clone(),
                repeat,
                millis,
                results: body["results"].as_array().map(Vec::len).unwrap_or(0),
                engines: body["engines"]
                    .as_array()
                    .map(|engines| {
                        engines
                            .iter()
                            .filter_map(|engine| engine.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default(),
                unresponsive: body["unresponsive_engines"]
                    .as_array()
                    .map(|engines| {
                        // upstream shape: [[engine, "Translated error"], ...]
                        engines
                            .iter()
                            .filter_map(|entry| {
                                let pair = entry.as_array()?;
                                Some(format!(
                                    "{}:{}",
                                    pair.first()?.as_str().unwrap_or_default(),
                                    pair.get(1)?.as_str().unwrap_or_default()
                                ))
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
                timings,
            });
        }
    }

    if args.json {
        let payload = serde_json::json!({
            "engines": engine_count,
            "baseline_ms": base_mean,
            "queries": args.queries,
            "repeats": args.repeats,
            "samples": samples.iter().map(|sample| serde_json::json!({
                "query": sample.query,
                "repeat": sample.repeat,
                "millis": sample.millis,
                "results": sample.results,
                "engines": sample.engines,
                "unresponsive": sample.unresponsive,
                "timings": sample.timings.iter().map(|(engine, ms)| serde_json::json!({"engine": engine, "millis": ms})).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&payload).unwrap_or_default()
        );
        return ExitCode::SUCCESS;
    }

    println!(
        "rustxng benchmark — {engine_count} engines, {} queries × {} repeats{}",
        args.queries.len(),
        args.repeats,
        if args.time_range.is_empty() {
            String::new()
        } else {
            format!(", time_range={}", args.time_range)
        }
    );
    println!("baseline /healthz (no engine): {base_mean:.1} ms\n");
    println!(
        "{:<34} {:>6} {:>8} {:>8} {:>8} {:>8}",
        "query", "rep", "total", "results", "engine_ms", "note"
    );
    for sample in &samples {
        let slowest = sample
            .timings
            .iter()
            .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(engine, ms)| format!("{engine} {ms:.0}"))
            .unwrap_or_default();
        let note = if sample.unresponsive.is_empty() {
            slowest
        } else {
            format!("{} ({})", slowest, sample.unresponsive.join(", "))
        };
        println!(
            "{:<34} {:>6} {:>7.0}ms {:>8} {:>8} {}",
            truncate(&sample.query, 34),
            sample.repeat + 1,
            sample.millis,
            sample.results,
            sample.timings.len(),
            truncate(&note, 40)
        );
    }

    let all: Vec<f64> = samples.iter().map(|sample| sample.millis).collect();
    let (mean, median, p95, max) = stats(&all);
    println!(
        "\ntotal latency: mean {mean:.0} ms, median {median:.0} ms, p95 {p95:.0} ms, max {max:.0} ms (n={})",
        all.len()
    );

    let first: Vec<f64> = samples
        .iter()
        .filter(|sample| sample.repeat == 0)
        .map(|sample| sample.millis)
        .collect();
    let later: Vec<f64> = samples
        .iter()
        .filter(|sample| sample.repeat > 0)
        .map(|sample| sample.millis)
        .collect();
    if !first.is_empty() && !later.is_empty() {
        let (first_mean, _, _, _) = stats(&first);
        let (later_mean, _, _, _) = stats(&later);
        println!(
            "cache effect: first pass {first_mean:.0} ms, later passes {later_mean:.0} ms ({:+.0}%)",
            (later_mean - first_mean) / first_mean * 100.0
        );
    }

    let per_engine = |samples: &[Sample]| {
        let mut values: std::collections::BTreeMap<String, Vec<f64>> =
            std::collections::BTreeMap::new();
        for sample in samples {
            for (engine, ms) in &sample.timings {
                values.entry(engine.clone()).or_default().push(*ms);
            }
        }
        values
    };
    println!("\nper engine (from the engine timings):");
    println!("{:<26} {:>6} {:>9} {:>9}", "engine", "n", "median", "max");
    for (engine, values) in per_engine(&samples) {
        let (_, median, _, max) = stats(&values);
        println!(
            "{:<26} {:>6} {:>8.0}ms {:>8.0}ms",
            engine,
            values.len(),
            median,
            max
        );
    }

    ExitCode::SUCCESS
}

fn truncate(value: &str, width: usize) -> String {
    if value.chars().count() <= width {
        return value.to_string();
    }
    let mut truncated: String = value.chars().take(width.saturating_sub(1)).collect();
    truncated.push('…');
    truncated
}
