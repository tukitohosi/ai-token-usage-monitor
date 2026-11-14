use chrono::{Local, NaiveDate, TimeZone};
use rusqlite::{params, OptionalExtension, Transaction, TransactionBehavior};

use super::parser::{hash_bytes, normalize_project, unknown_attribution};
use super::store::{ensure_model, ensure_project};
use super::{IncrementalIndex, IndexError, IndexResult, TokenUsage};

/// All accepted groups for one model in one Work thread, summed by the reader.
pub(crate) struct WorkUsageObservation {
    pub thread_id: String,
    pub model: String,
    pub cwd: Option<String>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub usage: TokenUsage,
    pub cache_metrics_available: bool,
}

struct Baseline {
    observed_at_ms: i64,
    activity_at_ms: i64,
    usage: TokenUsage,
}

impl IncrementalIndex {
    pub(crate) fn active_work_account_matches(&self, account_key: &str) -> IndexResult<bool> {
        Ok(self.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM app_meta WHERE key = 'active_work_account' AND value = ?1)",
            [account_hash(account_key)?], |row| row.get(0),
        )?)
    }

    pub(crate) fn clear_active_work_account(&mut self) -> IndexResult<bool> {
        Ok(self
            .connection
            .execute("DELETE FROM app_meta WHERE key = 'active_work_account'", [])?
            != 0)
    }

    pub(crate) fn merge_work_usage(
        &mut self,
        account_key: &str,
        observed_at_ms: i64,
        observations: &[WorkUsageObservation],
    ) -> IndexResult<i64> {
        let account_hash = account_hash(account_key)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "INSERT INTO app_meta(key, value) VALUES ('active_work_account', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value WHERE app_meta.value != excluded.value",
            [&account_hash],
        )?;
        let mut added = 0;
        for observation in observations {
            added += merge_observation(&tx, &account_hash, observed_at_ms, observation)?;
        }
        tx.commit()?;
        Ok(added)
    }
}

fn account_hash(key: &str) -> IndexResult<String> {
    if key.trim().is_empty() {
        return Err(IndexError::InvalidState("missing Work account scope"));
    }
    Ok(blake3::hash(key.as_bytes()).to_hex().to_string())
}

fn local_day(ms: i64) -> Option<NaiveDate> {
    Local
        .timestamp_millis_opt(ms)
        .single()
        .map(|date| date.date_naive())
}

fn valid_usage(usage: TokenUsage) -> bool {
    !usage.has_negative_delta_from(TokenUsage::default())
        && usage.cached_input_tokens <= usage.input_tokens
        && usage.reasoning_output_tokens <= usage.output_tokens
        && usage.input_tokens.checked_add(usage.output_tokens) == Some(usage.total_tokens)
}

fn merge_observation(
    tx: &Transaction<'_>,
    account: &str,
    observed: i64,
    item: &WorkUsageObservation,
) -> IndexResult<i64> {
    let model = item.model.trim();
    let id = item.thread_id.trim();
    let usage = item.usage;
    if id.is_empty()
        || model.is_empty()
        || usage.total_tokens < 0
        || (item.cache_metrics_available && !valid_usage(usage))
        || item.created_at_ms <= 0
        || item.updated_at_ms < item.created_at_ms
        || observed <= 0
    {
        return Ok(0);
    }
    let thread_hash = hash_bytes(id.to_ascii_lowercase().as_bytes());
    let mut attribution = unknown_attribution();
    attribution.model_key = model.to_lowercase();
    attribution.model_label = model.to_owned();
    if let Some(cwd) = item.cwd.as_deref().filter(|s| !s.trim().is_empty()) {
        (attribution.project_key, attribution.project_label) = normalize_project(cwd);
    }
    let model_id = ensure_model(tx, &attribution)?;
    let project_id = ensure_project(tx, &attribution)?;
    if !item.cache_metrics_available {
        let counter_id: i64 = tx.query_row(
            "INSERT INTO work_counters(account_hash, thread_hash, model_id, observed_at_ms, activity_at_ms,
                input_tokens,cached_input_tokens,output_tokens,reasoning_output_tokens,total_tokens)
             VALUES (?1,?2,?3,0,0,0,0,0,0,0)
             ON CONFLICT(account_hash,thread_hash,model_id) DO UPDATE SET model_id=excluded.model_id
             RETURNING id", params![account,&thread_hash[..],model_id], |row| row.get(0),
        )?;
        let classified: i64 = tx.query_row(
            "SELECT total_tokens FROM work_counters WHERE id=?1",
            [counter_id],
            |row| row.get(0),
        )?;
        let residual = usage.total_tokens - classified;
        if residual <= 0 {
            return Ok(0);
        }
        let day = local_day(item.created_at_ms);
        let occurred_at =
            (day.is_some() && day == local_day(item.updated_at_ms)).then_some(item.updated_at_ms);
        let mut key = blake3::Hasher::new();
        key.update(b"work-unclassified-v1\0");
        key.update(account.as_bytes());
        key.update(&thread_hash);
        key.update(attribution.model_key.as_bytes());
        // Published totals may temporarily regress. Only increase the residual
        // here; the detailed branch consumes it when classifications arrive.
        let changed = tx.execute(
            "INSERT INTO work_deltas(counter_id,logical_event_key,project_id,occurred_at_ms,input_tokens,
                cached_input_tokens,output_tokens,reasoning_output_tokens,total_tokens,cache_metrics_available)
             VALUES (?1,?2,?3,?4,0,0,0,0,?5,0)
             ON CONFLICT(logical_event_key) DO UPDATE SET total_tokens=excluded.total_tokens,
                occurred_at_ms=excluded.occurred_at_ms,project_id=excluded.project_id
             WHERE excluded.total_tokens > work_deltas.total_tokens",
            params![counter_id,&key.finalize().as_bytes()[..],project_id,occurred_at,residual],
        )?;
        return Ok(changed as i64);
    }
    let baseline = tx
        .query_row(
            "SELECT observed_at_ms, activity_at_ms, input_tokens,cached_input_tokens,output_tokens,
                reasoning_output_tokens,total_tokens FROM work_counters
         WHERE account_hash=?1 AND thread_hash=?2 AND model_id=?3 AND observed_at_ms>0",
            params![account, &thread_hash[..], model_id],
            |row| {
                Ok(Baseline {
                    observed_at_ms: row.get(0)?,
                    activity_at_ms: row.get(1)?,
                    usage: TokenUsage {
                        input_tokens: row.get(2)?,
                        cached_input_tokens: row.get(3)?,
                        output_tokens: row.get(4)?,
                        reasoning_output_tokens: row.get(5)?,
                        total_tokens: row.get(6)?,
                    },
                })
            },
        )
        .optional()?;
    if let Some(previous) = &baseline {
        // An old response or a temporarily regressed server counter cannot undo a
        // committed high-water mark and then cause the same tokens to be billed twice.
        if observed < previous.observed_at_ms
            || item.updated_at_ms < previous.activity_at_ms
            || usage.has_negative_delta_from(previous.usage)
        {
            return Ok(0);
        }
    }
    let previous_usage = baseline.as_ref().map(|row| row.usage).unwrap_or_default();
    let delta = usage.saturating_delta_from(previous_usage);
    if !valid_usage(delta) {
        return Ok(0);
    }
    let counter_id: i64 = tx.query_row(
        "INSERT INTO work_counters(account_hash, thread_hash, model_id, observed_at_ms, activity_at_ms,
            input_tokens,cached_input_tokens,output_tokens,reasoning_output_tokens,total_tokens)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)
         ON CONFLICT(account_hash,thread_hash,model_id) DO UPDATE SET
            observed_at_ms=excluded.observed_at_ms, activity_at_ms=excluded.activity_at_ms,
            input_tokens=excluded.input_tokens, cached_input_tokens=excluded.cached_input_tokens,
            output_tokens=excluded.output_tokens, reasoning_output_tokens=excluded.reasoning_output_tokens,
            total_tokens=excluded.total_tokens RETURNING id",
        params![account,&thread_hash[..],model_id,observed,item.updated_at_ms,usage.input_tokens,
            usage.cached_input_tokens,usage.output_tokens,usage.reasoning_output_tokens,usage.total_tokens],
        |row| row.get(0),
    )?;
    if delta.total_tokens == 0 {
        return Ok(0);
    }
    tx.execute(
        "DELETE FROM work_deltas WHERE counter_id=?1 AND cache_metrics_available=0 AND total_tokens<=?2",
        params![counter_id,delta.total_tokens],
    )?;
    tx.execute(
        "UPDATE work_deltas SET total_tokens=total_tokens-?2 WHERE counter_id=?1 AND cache_metrics_available=0",
        params![counter_id,delta.total_tokens],
    )?;
    // A first, multiday cumulative value is exact for all time. It carries no
    // day. Later deltas are dateable when their possible interval fits one day.
    let end_day = local_day(item.updated_at_ms);
    let start = baseline.as_ref().map_or(item.created_at_ms, |previous| {
        // The previous read bounded the interval only if the catalog now
        // confirms activity at or beyond that observation.
        if previous.observed_at_ms <= item.updated_at_ms {
            previous.observed_at_ms.max(previous.activity_at_ms)
        } else {
            previous.activity_at_ms
        }
    });
    let occurred_at =
        (end_day.is_some() && local_day(start) == end_day).then_some(item.updated_at_ms);
    let mut key = blake3::Hasher::new();
    key.update(b"work-usage-v1\0");
    key.update(account.as_bytes());
    key.update(&thread_hash);
    key.update(attribution.model_key.as_bytes());
    key.update(&usage.total_tokens.to_le_bytes());
    tx.execute(
        "INSERT INTO work_deltas(counter_id,logical_event_key,project_id,occurred_at_ms,input_tokens,
          cached_input_tokens,output_tokens,reasoning_output_tokens,total_tokens,cache_metrics_available)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
        params![counter_id,&key.finalize().as_bytes()[..],project_id,occurred_at,delta.input_tokens,
            delta.cached_input_tokens,delta.output_tokens,delta.reasoning_output_tokens,delta.total_tokens,
            item.cache_metrics_available],
    )?;
    Ok(1)
}
