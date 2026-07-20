//! Reusable planner-accuracy suite.
//!
//! Measures the compile step's accuracy + latency against a real LLM — the
//! metric that matters most for the project. Golden cases live in
//! `tests/fixtures/planner_cases.json` (data, not code): add cases there, no
//! recompile needed. Catalog = the four `UtilityBackend` caps (time,
//! random_int, reverse, string_length).
//!
//! Ignored by default (needs a running Ollama / OpenAI-compatible endpoint):
//!
//! ```sh
//! cargo test -p n3ur0n-node --test planner_eval -- --ignored --nocapture
//! # or the convenience runner:
//! scripts/planner-eval.sh [model]
//! ```
//!
//! Env:
//!   PLANNER_EVAL_BASE_URL  (default http://localhost:11434)
//!   PLANNER_EVAL_MODEL     (default llama3.1:8b)
//!   PLANNER_EVAL_RUNS      repetitions per case (default 1) — LLMs are
//!                          stochastic; >1 exposes variance and firms up rates.
//!   PLANNER_EVAL_API_KEY   bearer token for hosted endpoints
//!   PLANNER_EVAL_REPORT    path to write a JSON summary (for tracking runs)
//!
//! Grading per compiled plan:
//!   - valid : empty plan (legit "answer directly") OR passes `validate_plan`.
//!   - exact : plan's capability set == expected set.
//!   - precision/recall : over tool cases only (expected non-empty).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Instant;

use n3ur0n_adapters::Backend;
use n3ur0n_adapters::openai::{OpenAIBackend, OpenAIConfig};
use n3ur0n_adapters::utility::UtilityBackend;
use n3ur0n_node::planner::catalog::{Catalog, ToolDef};
use n3ur0n_node::planner::compiler::{LocalLLMCompiler, PlanCompiler};
use n3ur0n_node::planner::plan::{Plan, validate_plan};
use n3ur0n_node::planner::plan_exec::default_compile_system_prompt;
use serde_json::{Value, json};

struct Case {
    name: String,
    category: String,
    query: String,
    expect: BTreeSet<String>,
}

fn load_cases() -> Vec<Case> {
    let path = format!(
        "{}/tests/fixtures/planner_cases.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let raw = std::fs::read_to_string(&path).expect("read planner_cases.json");
    let doc: Value = serde_json::from_str(&raw).expect("parse planner_cases.json");
    doc["cases"]
        .as_array()
        .expect("`cases` array")
        .iter()
        .map(|c| Case {
            name: c["name"].as_str().unwrap_or("?").to_string(),
            category: c["category"].as_str().unwrap_or("?").to_string(),
            query: c["query"].as_str().expect("case.query").to_string(),
            expect: c["expect_tools"]
                .as_array()
                .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                .unwrap_or_default(),
        })
        .collect()
}

async fn build_catalog() -> Catalog {
    let decls = UtilityBackend.describe().await.expect("describe utility caps");
    let tools = decls
        .into_iter()
        .map(|cap| ToolDef {
            peer_id: "n3:evalpeer000000000000000000000000".into(),
            peer_endpoint: Some("http://eval.local:4242".into()),
            cap,
        })
        .collect();
    Catalog { tools }
}

fn plan_caps(plan: &Plan) -> BTreeSet<String> {
    plan.plan.iter().map(|s| s.capability.clone()).collect()
}

fn percentile(sorted: &[u128], p: f64) -> u128 {
    if sorted.is_empty() {
        return 0;
    }
    sorted[(((sorted.len() - 1) as f64) * p).round() as usize]
}

#[derive(Default)]
struct Agg {
    n: usize,
    valid: usize,
    exact: usize,
}

#[tokio::test]
#[ignore = "needs a running LLM endpoint; run with --ignored"]
async fn planner_eval() {
    let base = std::env::var("PLANNER_EVAL_BASE_URL")
        .unwrap_or_else(|_| "http://localhost:11434".into());
    let model = std::env::var("PLANNER_EVAL_MODEL").unwrap_or_else(|_| "llama3.1:8b".into());
    let runs: usize = std::env::var("PLANNER_EVAL_RUNS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1);

    let backend = Arc::new(
        OpenAIBackend::new(OpenAIConfig {
            base_url: base.clone(),
            default_model: model.clone(),
            api_key: std::env::var("PLANNER_EVAL_API_KEY").ok(),
            description: None,
            allow_model_override: false,
        })
        .expect("build backend"),
    );
    let compiler = LocalLLMCompiler {
        llm_backend: backend,
        model_hint: Some(model.clone()),
        system_prompt: Arc::new(default_compile_system_prompt),
    };
    let catalog = build_catalog().await;
    let cases = load_cases();

    println!(
        "\n=== planner eval · model={model} · endpoint={base} · {} cases × {runs} run(s) ===\n",
        cases.len()
    );

    let (mut valid_ok, mut exact_ok, mut total) = (0usize, 0usize, 0usize);
    let (mut prec_sum, mut rec_sum, mut tool_cases) = (0.0f64, 0.0f64, 0usize);
    let mut latencies: Vec<u128> = Vec::new();
    let mut by_cat: BTreeMap<String, Agg> = BTreeMap::new();

    for case in &cases {
        for _ in 0..runs {
            let t0 = Instant::now();
            let plan = compiler.compile(&case.query, &catalog).await.expect("compile");
            let ms = t0.elapsed().as_millis();
            latencies.push(ms);
            total += 1;

            let validity = if plan.plan.is_empty() {
                Ok(())
            } else {
                validate_plan(&plan, &catalog).map_err(|e| e.to_string())
            };
            let valid = validity.is_ok();
            let got = plan_caps(&plan);
            let exact = got == case.expect;

            valid_ok += usize::from(valid);
            exact_ok += usize::from(exact);
            let cat = by_cat.entry(case.category.clone()).or_default();
            cat.n += 1;
            cat.valid += usize::from(valid);
            cat.exact += usize::from(exact);

            if !case.expect.is_empty() {
                tool_cases += 1;
                let inter = got.intersection(&case.expect).count() as f64;
                prec_sum += if got.is_empty() { 0.0 } else { inter / got.len() as f64 };
                rec_sum += inter / case.expect.len() as f64;
            }

            let mark = if valid && exact { "OK  " } else { "FAIL" };
            println!(
                "[{mark}] {:<22} {:<6} · {:>6}ms · expect {:?} · got {:?}",
                case.name, case.category, ms, case.expect, got
            );
            if let Err(e) = &validity {
                println!("        invalid: {e}");
            }
        }
    }

    let n = total as f64;
    let mut sorted = latencies.clone();
    sorted.sort_unstable();
    let mean = latencies.iter().sum::<u128>() / total.max(1) as u128;
    let (p50, p95, max) = (
        percentile(&sorted, 0.5),
        percentile(&sorted, 0.95),
        sorted.last().copied().unwrap_or(0),
    );
    let tool_prec = if tool_cases > 0 { prec_sum / tool_cases as f64 } else { 1.0 };
    let tool_rec = if tool_cases > 0 { rec_sum / tool_cases as f64 } else { 1.0 };

    println!("\n--- aggregate ({total} runs) ---");
    println!("plan-valid : {valid_ok}/{total}  ({:.0}%)", 100.0 * valid_ok as f64 / n);
    println!("tool-exact : {exact_ok}/{total}  ({:.0}%)", 100.0 * exact_ok as f64 / n);
    println!("tool prec  : {:.0}%   recall {:.0}%   (tool cases only)", 100.0 * tool_prec, 100.0 * tool_rec);
    println!("compile ms : mean {mean} · p50 {p50} · p95 {p95} · max {max}");
    println!("\n  by category:");
    for (cat, a) in &by_cat {
        println!(
            "    {:<7} valid {:>3.0}%  exact {:>3.0}%  (n={})",
            cat, 100.0 * a.valid as f64 / a.n as f64, 100.0 * a.exact as f64 / a.n as f64, a.n
        );
    }
    println!();

    // Machine-readable report for tracking runs over time.
    if let Ok(path) = std::env::var("PLANNER_EVAL_REPORT") {
        let cats: BTreeMap<String, Value> = by_cat
            .iter()
            .map(|(k, a)| {
                (
                    k.clone(),
                    json!({
                        "n": a.n,
                        "valid_pct": (100.0 * a.valid as f64 / a.n as f64).round(),
                        "exact_pct": (100.0 * a.exact as f64 / a.n as f64).round(),
                    }),
                )
            })
            .collect();
        let report = json!({
            "model": model, "endpoint": base, "runs_per_case": runs, "total_runs": total,
            "plan_valid_pct": (100.0 * valid_ok as f64 / n).round(),
            "tool_exact_pct": (100.0 * exact_ok as f64 / n).round(),
            "tool_precision_pct": (100.0 * tool_prec).round(),
            "tool_recall_pct": (100.0 * tool_rec).round(),
            "compile_ms": {"mean": mean, "p50": p50, "p95": p95, "max": max},
            "by_category": cats,
        });
        if let Some(parent) = std::path::Path::new(&path).parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        std::fs::write(&path, serde_json::to_string_pretty(&report).unwrap())
            .expect("write report");
        println!("report written: {path}\n");
    }

    // Generous regression gate. This is a behaviour-measurement harness, not a
    // strict pass/fail — the metrics + JSON report are the point. But a large
    // validity drop is a real prompt/code regression (the peer::cap conflation
    // bug scored 12% here), so fail below 90%. Tool-exact is model-dependent and
    // reported for tracking, not gated.
    let valid_pct = 100.0 * valid_ok as f64 / n;
    assert!(
        valid_pct >= 90.0,
        "plan-valid dropped to {valid_pct:.0}% (<90%) — likely a prompt/compile regression"
    );
}
