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
//!   PLANNER_EVAL_MODEL     (default qwen2.5:7b)
//!   PLANNER_EVAL_RUNS      repetitions per case (default 1) — LLMs are
//!                          stochastic; >1 exposes variance and firms up rates.
//!   PLANNER_EVAL_API_KEY   bearer token for hosted endpoints
//!   PLANNER_EVAL_REPORT    path to write a JSON summary (for tracking runs)
//!
//! Grading per compiled plan:
//!   - valid : empty plan (legit "answer directly") OR passes `validate_plan`.
//!   - exact : plan's capability set == expected set.
//!   - precision/recall : over tool cases only (expected non-empty).
//!
//! Grading runs on the plan the **runtime** would execute, not on the raw
//! first compile: a rejected plan goes through `resolve_plan`, which
//! grants one corrective recompile with the validator's error fed back.
//! The first-pass rate is reported separately so a retry that rescues a
//! plan is visible as a recovery rather than silently inflating the
//! headline number — and so a retry that makes things worse cannot hide.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Instant;

use n3ur0n_adapters::Backend;
use n3ur0n_adapters::openai::{OpenAIBackend, OpenAIConfig};
use n3ur0n_adapters::utility::UtilityBackend;
use n3ur0n_node::planner::catalog::{Catalog, ToolDef};
use n3ur0n_node::planner::compiler::{LocalLLMCompiler, PlanCompiler};
use n3ur0n_node::planner::plan::{Plan, validate_plan};
use n3ur0n_node::planner::plan_exec::{
    PlanOutcome, REMOTE_TOP_K, default_compile_system_prompt, resolve_plan,
};
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
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default(),
        })
        .collect()
}

/// Build the catalog the planner sees: the four real `UtilityBackend`
/// caps, plus the harder fixture caps in `fixtures/planner_caps.json`.
///
/// The utility caps alone are trivially distinct, single-argument and all
/// on one peer, which is why a competent 7B scores 100% against them and
/// the suite stops telling you anything. The fixture adds overlapping
/// caps, several peers, a duplicate `chat` name, multi-argument schemas
/// and topical traps — see the `_comment` block in that file.
///
/// Set `PLANNER_EVAL_CATALOG=basic` to load only the utility caps, e.g.
/// to reproduce a historical number.
async fn build_catalog() -> Catalog {
    const EVAL_PEER: &str = "n3:evalpeer000000000000000000000000";
    let decls = UtilityBackend
        .describe()
        .await
        .expect("describe utility caps");
    let mut tools: Vec<ToolDef> = decls
        .into_iter()
        .map(|cap| ToolDef {
            peer_id: EVAL_PEER.into(),
            peer_endpoint: Some("http://eval.local:4242".into()),
            cap,
        })
        .collect();

    if std::env::var("PLANNER_EVAL_CATALOG").as_deref() == Ok("basic") {
        return Catalog { tools };
    }

    let path = format!(
        "{}/tests/fixtures/planner_caps.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let raw = std::fs::read_to_string(&path).expect("read planner_caps.json");
    let doc: Value = serde_json::from_str(&raw).expect("parse planner_caps.json");
    let peers = doc["peers"].as_object().expect("`peers` map");
    for entry in doc["caps"].as_array().expect("`caps` array") {
        let alias = entry["peer"].as_str().expect("cap.peer");
        let peer_id = peers
            .get(alias)
            .and_then(|v| v.as_str())
            .unwrap_or_else(|| panic!("unknown peer alias `{alias}` in planner_caps.json"));
        let cap = serde_json::from_value(entry["decl"].clone())
            .unwrap_or_else(|e| panic!("cap `{alias}` does not deserialize: {e}"));
        tools.push(ToolDef {
            peer_id: peer_id.to_string(),
            peer_endpoint: Some(format!("http://{alias}.eval.local:4242")),
            cap,
        });
    }
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
    let base =
        std::env::var("PLANNER_EVAL_BASE_URL").unwrap_or_else(|_| "http://localhost:11434".into());
    let model = std::env::var("PLANNER_EVAL_MODEL").unwrap_or_else(|_| "qwen2.5:7b".into());
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
    let mut tool_valid = 0usize; // valid plans among tool cases (the hard gate)
    let mut latencies: Vec<u128> = Vec::new();
    let mut by_cat: BTreeMap<String, Agg> = BTreeMap::new();
    // Retry accounting: how often the first compile was rejected, how
    // often the corrective recompile rescued it, and what it cost.
    let mut first_valid_ok = 0usize;
    let mut retried_n = 0usize;
    let mut recovered = 0usize;
    let mut retry_ms: Vec<u128> = Vec::new();

    for case in &cases {
        for _ in 0..runs {
            // Filter exactly as the runtime does, so the suite measures
            // the catalog the model actually sees. Skipping this would
            // grade a code path no user ever hits — and would hide both
            // the benefit of the relevance floor and any capability it
            // wrongly prunes.
            let catalog = catalog.clone().filter_for_query(&case.query, REMOTE_TOP_K);
            let t0 = Instant::now();
            let plan = compiler
                .compile(&case.query, &catalog)
                .await
                .expect("compile");
            let ms = t0.elapsed().as_millis();
            latencies.push(ms);
            total += 1;

            // First-compile verdict: what the model produced unaided.
            let first_validity = if plan.plan.is_empty() {
                Ok(())
            } else {
                validate_plan(&plan, &catalog).map_err(|e| e.to_string())
            };
            let first_valid = first_validity.is_ok();
            first_valid_ok += usize::from(first_valid);

            // Then the runtime path: `resolve_plan` gives a rejected plan
            // one corrective recompile with the validator's error fed
            // back. Grading on its result is what the user actually gets;
            // grading on the first compile alone would credit the planner
            // with failures it now recovers from — and would hide retries
            // that make things worse.
            let (plan, validity, retried) = if first_valid {
                (plan, first_validity, false)
            } else {
                let rejected = plan.clone();
                let t1 = Instant::now();
                let out = resolve_plan(&compiler, &case.query, &catalog, plan).await;
                retry_ms.push(t1.elapsed().as_millis());
                retried_n += 1;
                match out {
                    PlanOutcome::Valid(p) => (p, Ok(()), true),
                    // The model reconsidered and answered directly. Same
                    // outcome as a first-pass empty plan.
                    PlanOutcome::Empty => (Plan { plan: vec![] }, Ok(()), true),
                    // Still rejected: the runtime executes nothing. Keep
                    // the rejected plan so `got` still reports what the
                    // model asked for, and keep the verdict invalid —
                    // mapping this to an empty plan would silently score
                    // a failure as a success.
                    PlanOutcome::Invalid(e) => (rejected, Err(e), true),
                }
            };
            let valid = validity.is_ok();
            if retried && valid {
                recovered += 1;
            }
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
                tool_valid += usize::from(valid);
                let inter = got.intersection(&case.expect).count() as f64;
                prec_sum += if got.is_empty() {
                    0.0
                } else {
                    inter / got.len() as f64
                };
                rec_sum += inter / case.expect.len() as f64;
            }

            let mark = if valid && exact { "OK  " } else { "FAIL" };
            let retry_mark = if retried { " ↻" } else { "" };
            println!(
                "[{mark}] {:<22} {:<6} · {:>6}ms{retry_mark} · expect {:?} · got {:?}",
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
    let tool_prec = if tool_cases > 0 {
        prec_sum / tool_cases as f64
    } else {
        1.0
    };
    let tool_rec = if tool_cases > 0 {
        rec_sum / tool_cases as f64
    } else {
        1.0
    };

    let retry_mean = if retry_ms.is_empty() {
        0
    } else {
        retry_ms.iter().sum::<u128>() / retry_ms.len() as u128
    };

    println!("\n--- aggregate ({total} runs) ---");
    println!(
        "plan-valid : {valid_ok}/{total}  ({:.0}%)   [after retry]",
        100.0 * valid_ok as f64 / n
    );
    println!(
        "  first pass : {first_valid_ok}/{total}  ({:.0}%)   \
         retried {retried_n} · recovered {recovered} · retry mean {retry_mean}ms",
        100.0 * first_valid_ok as f64 / n
    );
    println!(
        "  tool-valid : {tool_valid}/{tool_cases}  ({:.0}%)  [gated]",
        if tool_cases > 0 {
            100.0 * tool_valid as f64 / tool_cases as f64
        } else {
            100.0
        }
    );
    println!(
        "tool-exact : {exact_ok}/{total}  ({:.0}%)",
        100.0 * exact_ok as f64 / n
    );
    println!(
        "  (note: `none`/`trap` \"invalid\" = model over-planned; rejected at compile -> safe direct-reply fallback)"
    );
    println!(
        "tool prec  : {:.0}%   recall {:.0}%   (tool cases only)",
        100.0 * tool_prec,
        100.0 * tool_rec
    );
    println!("compile ms : mean {mean} · p50 {p50} · p95 {p95} · max {max}");
    println!("\n  by category:");
    for (cat, a) in &by_cat {
        println!(
            "    {:<7} valid {:>3.0}%  exact {:>3.0}%  (n={})",
            cat,
            100.0 * a.valid as f64 / a.n as f64,
            100.0 * a.exact as f64 / a.n as f64,
            a.n
        );
    }
    println!();

    let tool_valid_pct = if tool_cases > 0 {
        100.0 * tool_valid as f64 / tool_cases as f64
    } else {
        100.0
    };

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
            "retry": {
                "first_pass_valid_pct": (100.0 * first_valid_ok as f64 / n).round(),
                "retried": retried_n,
                "recovered": recovered,
                "mean_ms": retry_mean,
            },
            "tool_valid_pct": tool_valid_pct.round(),
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

    // Regression gate on TOOL cases only: when the user clearly wants tools, the
    // plan must be structurally valid (peer/cap/refs resolve). This catches
    // real prompt/compile regressions (the peer::cap conflation bug scored 0%
    // here) without failing on the model over-planning no-tool questions — those
    // `none`/`trap` "invalids" are the model's restraint problem and are safely
    // rejected at compile (→ direct-reply fallback), not a code bug.
    assert!(
        tool_valid_pct >= 95.0,
        "tool-case plan-valid dropped to {tool_valid_pct:.0}% (<95%) — prompt/compile regression"
    );
}
