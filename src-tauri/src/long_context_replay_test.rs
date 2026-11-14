//! Opt-in reconciliation of a single-day Claude project using a local transcript
//! and an independently prepared set of expected totals. No transcript is bundled.
use super::*;
use crate::{settings, usage_export};

#[derive(serde::Deserialize)]
struct ReplayExpectations {
    cutoff: String,
    project: String,
    total_tokens: i64,
    requests: usize,
    indexed_events: i64,
    long_calls: i64,
    uncertain: i64,
    base_usd: f64,
    tiered_usd: f64,
    rule: pricing::LongContextRule,
    models: Vec<pricing::ModelPricingRule>,
}

#[test]
#[ignore = "set AI_TOKEN_CLAUDE_REPLAY_LOG and AI_TOKEN_CLAUDE_REPLAY_EXPECTATIONS (JSON)"]
fn single_day_session_reconciles_project_day_and_export() {
    let sample: ReplayExpectations = serde_json::from_str(
        &std::env::var("AI_TOKEN_CLAUDE_REPLAY_EXPECTATIONS").expect("expected totals JSON"),
    )
    .expect("valid replay expectations");
    let main =
        PathBuf::from(std::env::var_os("AI_TOKEN_CLAUDE_REPLAY_LOG").expect("main transcript"));
    let mut files = collect_files(&main.with_extension("").join("subagents"), |p| {
        extension_is(p, "jsonl")
    });
    files.push(main);
    let temporary = tempfile::tempdir().unwrap();
    let database = temporary.path().join("usage.sqlite3");
    let settings_path = temporary.path().join("settings.sqlite3");
    let mut index = MultiSourceIndex::open(&database).unwrap();
    let cutoff = chrono::DateTime::parse_from_rfc3339(&sample.cutoff)
        .unwrap()
        .timestamp_millis();
    // Replay only rows visible by the requested cutoff, before the normal
    // parser selects the latest usage snapshot for each response.
    index
        .scan_files(
            SOURCE_CLAUDE,
            &files,
            |path, content| {
                let mut prefix = String::new();
                for line in content.lines() {
                    let row: serde_json::Value =
                        serde_json::from_str(line).expect("valid sample row");
                    if timestamp(row.get("timestamp")).is_some_and(|time| time <= cutoff) {
                        prefix.push_str(line);
                        prefix.push('\n');
                    }
                }
                parse_claude_file(path, &prefix)
            },
            |_, _| {},
        )
        .unwrap();

    let long_calls: i64 = index
        .connection
        .query_row(
            "SELECT COUNT(*) FROM source_events WHERE request_input_tokens > ?1",
            [sample.rule.threshold],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(long_calls, sample.long_calls);
    let mut prices = pricing::PricingSettings {
        models: sample.models,
        ..Default::default()
    };
    let local_day = local_date_from_ms(cutoff).unwrap().to_string();
    for (apply, expected_usd, expected_uncertain) in [
        (false, sample.base_usd, 0),
        (true, sample.tiered_usd, sample.uncertain),
    ] {
        for model in &mut prices.models {
            model.long_context = apply.then(|| sample.rule.clone());
        }
        let check = |amount: f64| {
            assert!(
                (amount - expected_usd).abs() < 0.00000001,
                "{amount} vs {expected_usd}"
            )
        };
        let summary = index.aggregate_source_with_pricing(
            SourceState::ready(SOURCE_CLAUDE, "Claude Code"),
            &prices,
        );
        assert_eq!(summary.indexed_events, sample.indexed_events);
        assert_eq!(summary.total.total_tokens, sample.total_tokens);
        assert_eq!(
            summary.cost.long_context_unconfirmed_events,
            expected_uncertain
        );
        assert_eq!(summary.cost.unpriced_tokens, 0);
        check(summary.cost.usd);
        assert_eq!(summary.by_project.len(), 1);
        assert_eq!(summary.by_project[0].label, sample.project);
        check(summary.by_project[0].cost.usd);

        let day = build_day_detail(
            local_day.clone(),
            index.day_tasks_with_pricing(&local_day, &prices).unwrap(),
        );
        assert_eq!(
            day.tasks
                .iter()
                .filter(|task| task.usage.total_tokens > 0)
                .count(),
            sample.requests
        );
        check(day.tasks.iter().map(|task| task.cost.usd).sum());

        settings::write_pricing_settings(&settings_path, &prices).unwrap();
        let report = usage_export::read_report(
            usage_export::UsageExportQuery {
                range: usage_export::ExportRange::Dates {
                    start_date: local_day.clone(),
                    end_date: local_day.clone(),
                },
                source_ids: vec![SOURCE_CLAUDE.to_owned()],
            },
            &temporary.path().join("unused-codex.sqlite3"),
            &database,
            &settings_path,
            None,
        )
        .unwrap();
        let report = serde_json::to_value(report).unwrap();
        for path in ["/summary", "/projects/0", "/daily/0", "/sources/0"] {
            let row = report.pointer(path).unwrap();
            assert_eq!(row["usage"]["totalTokens"], sample.total_tokens);
            assert_eq!(
                row["cost"]["longContextUnconfirmedEvents"],
                expected_uncertain
            );
            check(row["cost"]["usd"].as_f64().unwrap());
        }
        assert_eq!(
            report["models"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|r| r["usage"]["totalTokens"].as_i64().unwrap() > 0)
                .count(),
            prices.models.len()
        );
        check(
            report["models"]
                .as_array()
                .unwrap()
                .iter()
                .map(|r| r["cost"]["usd"].as_f64().unwrap())
                .sum(),
        );
    }
}
