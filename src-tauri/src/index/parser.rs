use chrono::DateTime;
use serde_json::{Map, Value};

use super::model::{
    Attribution, OwnershipMode, ParseEffects, ParserState, PendingUsageEvent, TokenUsage,
    UsageSource,
};

pub(crate) const MAX_LINE_BYTES: usize = 32 * 1024 * 1024;

pub(crate) fn initial_state() -> ParserState {
    ParserState {
        first_record_seen: false,
        next_history_ordinal: 0,
        is_subagent: false,
        ownership_mode: OwnershipMode::All,
        boundary_ordinal: None,
        owned: true,
        current: unknown_attribution(),
        legacy_candidate: None,
        previous_total: None,
        last_fallback_pending: false,
        skipped_events: 0,
        filtered_parent_events: 0,
    }
}

pub(crate) fn unknown_attribution() -> Attribution {
    Attribution {
        model_key: "__unknown_model__".to_owned(),
        model_label: "Unknown model".to_owned(),
        project_key: hash_bytes(b"__unknown_project__").to_vec(),
        project_label: "Unknown project".to_owned(),
    }
}

pub(crate) fn process_complete_line(
    state: &mut ParserState,
    bytes: &[u8],
    oversized: bool,
    source_line: i64,
    end_offset: i64,
    rollout_key_hash: &[u8],
) -> ParseEffects {
    let is_first_record = !state.first_record_seen;
    let ordinal = (!is_first_record).then_some(state.next_history_ordinal);

    let effects = if oversized {
        invalid_record(state, is_first_record, "invalid-json")
    } else {
        let text = String::from_utf8_lossy(bytes);
        match serde_json::from_str::<Value>(&text) {
            Ok(Value::Object(record)) => process_record(
                state,
                &record,
                is_first_record,
                ordinal,
                source_line,
                end_offset,
                rollout_key_hash,
            ),
            _ => invalid_record(state, is_first_record, "invalid-json"),
        }
    };

    // Ordinals are positional. Every complete record, including a damaged or
    // oversized record, consumes exactly one position. The first session_meta
    // record itself is outside history and therefore has no ordinal.
    if is_first_record {
        state.first_record_seen = true;
    } else {
        state.next_history_ordinal += 1;
    }
    effects
}

fn process_record(
    state: &mut ParserState,
    record: &Map<String, Value>,
    is_first_record: bool,
    ordinal: Option<i64>,
    source_line: i64,
    end_offset: i64,
    rollout_key_hash: &[u8],
) -> ParseEffects {
    let record_type = non_empty_string(record.get("type"));
    let payload = record.get("payload").and_then(Value::as_object);
    let occurred_at_ms = record
        .get("timestamp")
        .and_then(Value::as_str)
        .and_then(parse_timestamp_ms);
    let mut effects = ParseEffects::empty();

    if is_first_record && (record_type != Some("session_meta") || payload.is_none()) {
        make_first_record_conservative(state);
        effects.warning_codes.push("invalid-session-meta");
    }

    match (record_type, payload) {
        (Some("session_meta"), Some(payload)) if is_first_record => {
            handle_session_meta(state, payload, &mut effects)
        }
        (Some("turn_context"), Some(payload)) => {
            let attribution = parse_attribution(payload, &state.current);
            if is_owned_record(state, ordinal) {
                state.current = attribution;
            } else {
                state.legacy_candidate = Some(attribution);
            }
        }
        (Some("inter_agent_communication_metadata"), Some(payload)) => {
            if state.is_subagent
                && state.ownership_mode == OwnershipMode::Unresolved
                && payload.get("trigger_turn").and_then(Value::as_bool) == Some(true)
            {
                state.ownership_mode = OwnershipMode::LegacyMarker;
                state.owned = true;
                if let Some(candidate) = state.legacy_candidate.clone() {
                    state.current = candidate;
                }
                // Keep previous_total: the copied parent snapshot is the
                // baseline for the first child-owned cumulative snapshot.
                state.last_fallback_pending = false;
            }
        }
        (Some("event_msg"), Some(payload))
            if non_empty_string(payload.get("type")) == Some("token_count") =>
        {
            effects = process_token_event(
                state,
                payload,
                ordinal,
                source_line,
                end_offset,
                occurred_at_ms,
                rollout_key_hash,
            );
        }
        _ => {}
    }
    effects
}

fn invalid_record(
    state: &mut ParserState,
    is_first_record: bool,
    warning: &'static str,
) -> ParseEffects {
    state.skipped_events += 1;
    let mut effects = ParseEffects::empty();
    effects.warning_codes.push(warning);
    if is_first_record {
        make_first_record_conservative(state);
        effects.warning_codes.push("invalid-session-meta");
    }
    effects
}

fn make_first_record_conservative(state: &mut ParserState) {
    state.is_subagent = true;
    state.ownership_mode = OwnershipMode::Unresolved;
    state.boundary_ordinal = None;
    state.owned = false;
}

fn handle_session_meta(
    state: &mut ParserState,
    payload: &Map<String, Value>,
    effects: &mut ParseEffects,
) {
    let source_has_subagent = payload
        .get("source")
        .and_then(Value::as_object)
        .is_some_and(|source| source.contains_key("subagent"));
    let is_subagent = non_empty_string(payload.get("parent_thread_id")).is_some()
        || non_empty_string(payload.get("forked_from_id")).is_some()
        || source_has_subagent;
    let boundary = non_negative_i64(payload.get("subagent_history_start_ordinal"));

    state.is_subagent = is_subagent;
    state.ownership_mode = if !is_subagent {
        OwnershipMode::All
    } else if boundary.is_some() {
        OwnershipMode::Ordinal
    } else {
        OwnershipMode::Unresolved
    };
    state.boundary_ordinal = boundary;
    state.owned = !is_subagent;

    if let Some(cwd) = non_empty_string(payload.get("cwd")) {
        let (project_key, project_label) = normalize_project(cwd);
        state.current.project_key = project_key;
        state.current.project_label = project_label;
    }
    if !is_subagent && boundary.is_some() {
        effects.warning_codes.push("invalid-session-meta");
    }
}

fn process_token_event(
    state: &mut ParserState,
    payload: &Map<String, Value>,
    ordinal: Option<i64>,
    source_line: i64,
    end_offset: i64,
    occurred_at_ms: Option<i64>,
    rollout_key_hash: &[u8],
) -> ParseEffects {
    let info = payload.get("info").and_then(Value::as_object);
    let total = info.and_then(|value| {
        read_usage(
            value
                .get("total_token_usage")
                .or_else(|| value.get("totalTokenUsage")),
        )
    });
    let last = info.and_then(|value| {
        read_usage(
            value
                .get("last_token_usage")
                .or_else(|| value.get("lastTokenUsage")),
        )
    });

    if !is_owned_record(state, ordinal) {
        if let Some(total) = total {
            state.previous_total = Some(total.usage);
            state.last_fallback_pending = false;
        }
        state.filtered_parent_events += 1;
        return ParseEffects::empty();
    }

    let mut effects = ParseEffects::empty();
    let usage = if let Some(total) = total {
        let result = if let Some(previous) = state.previous_total {
            if total.usage.has_negative_delta_from(previous) {
                effects.warning_codes.push("cumulative-counter-reset");
                (
                    total.usage,
                    total.cache_metrics_available,
                    UsageSource::TotalSegmentStart,
                )
            } else {
                (
                    total.usage.saturating_delta_from(previous),
                    total.cache_metrics_available,
                    UsageSource::TotalDelta,
                )
            }
        } else if state.last_fallback_pending {
            (TokenUsage::default(), false, UsageSource::TotalDelta)
        } else {
            (
                total.usage,
                total.cache_metrics_available,
                UsageSource::TotalSegmentStart,
            )
        };
        state.previous_total = Some(total.usage);
        state.last_fallback_pending = false;
        Some(result)
    } else if let Some(last) = last {
        if state.previous_total.is_none() && !state.last_fallback_pending {
            state.last_fallback_pending = true;
            effects.warning_codes.push("last-usage-fallback");
            Some((
                last.usage,
                last.cache_metrics_available,
                UsageSource::LastFallback,
            ))
        } else {
            state.skipped_events += 1;
            effects.warning_codes.push("last-usage-skipped");
            None
        }
    } else {
        state.skipped_events += 1;
        effects.warning_codes.push("invalid-token-count");
        None
    };

    if let Some((usage, cache_metrics_available, source)) = usage {
        let logical_key = logical_event_key(rollout_key_hash, ordinal, source_line);
        let payload_hash = event_payload_hash(&logical_key, &state.current, usage, source);
        effects.event = Some(PendingUsageEvent {
            // A cumulative delta / segment start may span multiple requests.
            // Verify it against the separately reported latest call before tiering.
            request_input_tokens: last
                .filter(|call| {
                    call.complete_request
                        && cache_metrics_available
                        && call.usage.input_tokens == usage.input_tokens
                        && call.usage.cached_input_tokens == usage.cached_input_tokens
                        && call.usage.output_tokens == usage.output_tokens
                        && call.usage.total_tokens == usage.total_tokens
                })
                .map(|call| call.usage.input_tokens),
            logical_key,
            source_ordinal: ordinal,
            source_line,
            end_offset,
            occurred_at_ms,
            source,
            attribution: state.current.clone(),
            usage,
            cache_metrics_available,
            payload_hash,
        });
    }
    effects
}

fn is_owned_record(state: &ParserState, ordinal: Option<i64>) -> bool {
    match state.ownership_mode {
        OwnershipMode::All => true,
        OwnershipMode::Ordinal => ordinal
            .zip(state.boundary_ordinal)
            .is_some_and(|(ordinal, boundary)| ordinal > boundary),
        OwnershipMode::LegacyMarker | OwnershipMode::Unresolved => state.owned,
    }
}

fn parse_attribution(payload: &Map<String, Value>, fallback: &Attribution) -> Attribution {
    let model_label = non_empty_string(payload.get("model"))
        .map(str::to_owned)
        .unwrap_or_else(|| fallback.model_label.clone());
    let model_key = if model_label == "Unknown model" {
        fallback.model_key.clone()
    } else {
        model_label.to_lowercase()
    };
    let (project_key, project_label) = non_empty_string(payload.get("cwd"))
        .map(normalize_project)
        .unwrap_or_else(|| (fallback.project_key.clone(), fallback.project_label.clone()));
    Attribution {
        model_key,
        model_label,
        project_key,
        project_label,
    }
}

#[derive(Clone, Copy)]
struct IndexedUsage {
    usage: TokenUsage,
    complete_request: bool,
    cache_metrics_available: bool,
}

fn read_usage(value: Option<&Value>) -> Option<IndexedUsage> {
    let value = value?.as_object()?;
    let input = pick_number(value, "input_tokens", "inputTokens");
    let cached = pick_number(value, "cached_input_tokens", "cachedInputTokens");
    let output = pick_number(value, "output_tokens", "outputTokens");
    let reasoning = pick_number(value, "reasoning_output_tokens", "reasoningOutputTokens");
    let supplied_total = pick_number(value, "total_tokens", "totalTokens");
    if input.is_none()
        && cached.is_none()
        && output.is_none()
        && reasoning.is_none()
        && supplied_total.is_none()
    {
        return None;
    }
    Some(IndexedUsage {
        complete_request: input
            .zip(cached)
            .zip(output)
            .is_some_and(|((input, cached), output)| {
                cached <= input
                    && input
                        .checked_add(output)
                        .is_some_and(|sum| supplied_total.map_or(true, |total| sum == total))
            }),
        usage: TokenUsage {
            input_tokens: input.unwrap_or(0),
            cached_input_tokens: cached.unwrap_or(0),
            output_tokens: output.unwrap_or(0),
            reasoning_output_tokens: reasoning.unwrap_or(0),
            total_tokens: supplied_total.unwrap_or(input.unwrap_or(0) + output.unwrap_or(0)),
        },
        // The Codex counter format has no cache-write category; when both
        // input categories are present, the indexed cache-write amount is a
        // known zero rather than an inferred value.
        cache_metrics_available: input.is_some() && cached.is_some(),
    })
}

fn pick_number(value: &Map<String, Value>, snake: &str, camel: &str) -> Option<i64> {
    non_negative_i64(value.get(snake).or_else(|| value.get(camel)))
}

fn non_negative_i64(value: Option<&Value>) -> Option<i64> {
    let number = value?.as_f64()?;
    if number.is_finite() && number >= 0.0 && number <= i64::MAX as f64 {
        Some(number.trunc() as i64)
    } else {
        None
    }
}

fn non_empty_string(value: Option<&Value>) -> Option<&str> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn parse_timestamp_ms(value: &str) -> Option<i64> {
    let has_zone = value.get(10..).is_some_and(|tail| {
        tail.contains('Z')
            || tail.contains('z')
            || tail.contains('+')
            || tail.get(1..).is_some_and(|rest| rest.contains('-'))
    });
    let candidate = if has_zone {
        value.to_owned()
    } else {
        format!("{value}Z")
    };
    DateTime::parse_from_rfc3339(&candidate)
        .ok()
        .map(|timestamp| timestamp.timestamp_millis())
}

pub(crate) fn normalize_project(raw: &str) -> (Vec<u8>, String) {
    let mut value = raw.trim().to_owned();
    if let Some(stripped) = value.strip_prefix(r"\\?\UNC\") {
        value = format!(r"\\{stripped}");
    } else if let Some(stripped) = value.strip_prefix(r"\\?\") {
        value = stripped.to_owned();
    }

    let windows_like = value.starts_with(r"\\")
        || value
            .as_bytes()
            .get(1)
            .is_some_and(|character| *character == b':');
    let separator = if windows_like { '\\' } else { '/' };
    if windows_like {
        value = value.replace('/', r"\");
    } else {
        value = value.replace('\\', "/");
    }

    let prefix = if windows_like && value.starts_with(r"\\") {
        r"\\"
    } else if windows_like && value.as_bytes().get(1) == Some(&b':') {
        &value[..2]
    } else if !windows_like && value.starts_with('/') {
        "/"
    } else {
        ""
    };
    let body_source = if prefix.ends_with(':') {
        value.get(2..).unwrap_or_default()
    } else {
        &value
    };
    let body = body_source.trim_start_matches(['/', '\\']);
    let mut components: Vec<&str> = Vec::new();
    for component in body.split(['/', '\\']) {
        match component {
            "" | "." => {}
            ".." => {
                components.pop();
            }
            _ => components.push(component),
        }
    }
    let joined = components.join(&separator.to_string());
    let normalized = if prefix.ends_with(':') && !joined.is_empty() {
        format!("{prefix}{separator}{joined}")
    } else {
        format!("{prefix}{joined}")
    };
    let label = components
        .last()
        .copied()
        .filter(|value| !value.is_empty())
        .unwrap_or(&normalized)
        .to_owned();
    let key_material = if windows_like {
        normalized.to_lowercase()
    } else {
        normalized
    };
    (hash_bytes(key_material.as_bytes()).to_vec(), label)
}

pub(crate) fn hash_bytes(value: &[u8]) -> [u8; 32] {
    *blake3::hash(value).as_bytes()
}

fn logical_event_key(rollout_key_hash: &[u8], ordinal: Option<i64>, source_line: i64) -> Vec<u8> {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"usage-v1\0");
    hasher.update(rollout_key_hash);
    match ordinal {
        Some(value) => {
            hasher.update(b"\0o");
            hasher.update(&value.to_le_bytes());
        }
        None => {
            hasher.update(b"\0l");
            hasher.update(&source_line.to_le_bytes());
        }
    }
    hasher.finalize().as_bytes().to_vec()
}

fn event_payload_hash(
    logical_key: &[u8],
    attribution: &Attribution,
    usage: TokenUsage,
    source: UsageSource,
) -> Vec<u8> {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"event-payload-v1\0");
    hasher.update(logical_key);
    hasher.update(source.as_str().as_bytes());
    hasher.update(attribution.model_key.as_bytes());
    hasher.update(&attribution.project_key);
    for number in [
        usage.input_tokens,
        usage.cached_input_tokens,
        usage.output_tokens,
        usage.reasoning_output_tokens,
        usage.total_tokens,
    ] {
        hasher.update(&number.to_le_bytes());
    }
    hasher.finalize().as_bytes().to_vec()
}
