use std::{
    collections::{BTreeMap, HashMap},
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

use chrono::{Local, TimeZone, Utc};
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};

use crate::{
    app_server::{AppServerClient, ThreadUsageGroup},
    index::{TokenUsage, WorkUsageObservation},
};

const MAX_REQUESTS: usize = 4;
const POLL_BUDGET: Duration = Duration::from_secs(8);
const MINUTE: i64 = 60_000;
const DAY: i64 = 24 * 60 * MINUTE;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct HostedUsageCoverage {
    pub(crate) candidates: Option<usize>,
    pub(crate) known: usize,
    pub(crate) queried: usize,
    pub(crate) unavailable: usize,
    pub(crate) today_candidates: usize,
    pub(crate) today_with_thread_usage: usize,
    pub(crate) today_attempted: usize,
    pub(crate) updated_for_date: String,
    pub(crate) last_checked_at_ms: Option<i64>,
    pub(crate) outcomes: BTreeMap<String, usize>,
    pub(crate) scope: Option<String>,
    pub(crate) local_control: Option<String>,
}

#[derive(Default)]
pub(crate) struct HostedUsageBatch {
    pub(crate) observations: Vec<WorkUsageObservation>,
    pub(crate) coverage: HostedUsageCoverage,
    pub(crate) warning: Option<String>,
    account_key: String,
    successes: Vec<(String, i64, i64)>,
    with_totals: Vec<(String, i64)>,
}

#[derive(Default)]
struct Check {
    attempted_at: i64,
    attempted_revision: i64,
    complete_at: Option<i64>,
    with_total_at: Option<i64>,
    needs_retry: bool,
}

#[derive(Default)]
pub(crate) struct HostedUsagePoller {
    account_key: String,
    checks: HashMap<String, Check>,
    control_checked: i64,
    local_control: Option<String>,
    last_report: Option<HostedUsageCoverage>,
}

struct Candidate {
    id: String,
    cwd: Option<String>,
    created: i64,
    updated: i64,
}

impl Candidate {
    fn active_on(&self, date: chrono::NaiveDate) -> bool {
        Local
            .timestamp_millis_opt(self.updated)
            .single()
            .is_some_and(|v| v.date_naive() == date)
    }
}

pub(crate) fn catalog_only_coverage(home: &Path) -> Option<HostedUsageCoverage> {
    let rows = discover(home, None).ok()?;
    if rows.is_empty() {
        return None;
    }
    let today = Local::now().date_naive();
    Some(HostedUsageCoverage {
        candidates: Some(rows.len()),
        scope: Some("catalogOnly".into()),
        updated_for_date: today.to_string(),
        today_candidates: rows.iter().filter(|row| row.active_on(today)).count(),
        ..Default::default()
    })
}

impl HostedUsagePoller {
    pub(crate) fn poll(
        &mut self,
        codex_home: &Path,
        account_key: &str,
        client: &mut AppServerClient,
        cancelled: &AtomicBool,
    ) -> HostedUsageBatch {
        let workspace = client.workspace_account_id().map(str::to_owned);
        self.poll_with(
            codex_home,
            account_key,
            workspace.as_deref(),
            cancelled,
            |id, timeout| client.thread_usage_read(id, timeout),
        )
    }

    fn poll_with(
        &mut self,
        codex_home: &Path,
        account_key: &str,
        workspace: Option<&str>,
        cancelled: &AtomicBool,
        mut read: impl FnMut(&str, Duration) -> Result<Vec<ThreadUsageGroup>, String>,
    ) -> HostedUsageBatch {
        if self.account_key != account_key {
            self.account_key = account_key.to_owned();
            self.checks.clear();
            self.control_checked = 0;
            self.local_control = None;
            self.last_report = None;
        }
        let mut batch = HostedUsageBatch {
            account_key: account_key.to_owned(),
            ..Default::default()
        };
        let today = Local::now().date_naive();
        batch.coverage.updated_for_date = today.to_string();
        let candidates = match discover(codex_home, workspace) {
            Ok(rows) => rows,
            Err(err) => {
                batch.warning = Some(
                    match err {
                        CatalogError::AmbiguousHost => {
                            "本机有多个 Work 宿主，当前账号的宿主暂无法唯一确定"
                        }
                        CatalogError::Unreadable => "无法读取本机的 ChatGPT Work 会话目录",
                    }
                    .into(),
                );
                return batch;
            }
        };
        let now = Utc::now().timestamp_millis();
        let scope = &mut batch.coverage;
        scope.candidates = Some(candidates.len());
        scope.scope = Some(
            if workspace.is_some() {
                "workspace"
            } else {
                "singleHost"
            }
            .into(),
        );
        scope.today_candidates = candidates.iter().filter(|row| row.active_on(today)).count();
        scope.known = candidates
            .iter()
            .filter(|candidate| {
                self.checks.get(&candidate.id).is_some_and(|check| {
                    check
                        .complete_at
                        .is_some_and(|revision| revision >= candidate.updated)
                })
            })
            .count();
        let queue = self.due_candidates(&candidates, now);
        let start = Instant::now();
        for candidate in queue.into_iter().take(MAX_REQUESTS) {
            let budget = POLL_BUDGET.saturating_sub(start.elapsed());
            if budget < Duration::from_millis(100) || cancelled.load(Ordering::Acquire) {
                break;
            }
            scope.queried += 1;
            let check = self.checks.entry(candidate.id.clone()).or_default();
            check.attempted_at = now;
            check.attempted_revision = candidate.updated;
            check.needs_retry = true;
            let groups = match read(&candidate.id, budget.min(POLL_BUDGET / 2)) {
                Ok(rows) => rows,
                Err(kind) => {
                    *scope.outcomes.entry(kind).or_default() += 1;
                    scope.unavailable += 1;
                    continue;
                }
            };
            let (observations, complete) = observations(candidate, groups);
            let outcome = if observations.is_empty() {
                "noAttributableTokens"
            } else if complete {
                "complete"
            } else {
                "partial"
            };
            *scope.outcomes.entry(outcome.into()).or_default() += 1;
            if !observations.is_empty() {
                batch
                    .with_totals
                    .push((candidate.id.clone(), candidate.updated));
            }
            batch.observations.extend(observations);
            if complete {
                if !check
                    .complete_at
                    .is_some_and(|revision| revision >= candidate.updated)
                {
                    scope.known += 1;
                }
                batch
                    .successes
                    .push((candidate.id.clone(), candidate.updated, now));
            } else {
                scope.unavailable += 1;
            }
        }
        for row in candidates.iter().filter(|row| row.active_on(today)) {
            let check = self.checks.get(&row.id);
            if check.is_some_and(|s| s.attempted_revision >= row.updated) {
                scope.today_attempted += 1;
            }
            if check.is_some_and(|s| s.with_total_at.is_some_and(|rev| rev >= row.updated))
                || batch
                    .with_totals
                    .iter()
                    .any(|(id, rev)| id == &row.id && *rev >= row.updated)
            {
                scope.today_with_thread_usage += 1;
            }
        }
        if scope.unavailable > 0
            && batch.with_totals.is_empty()
            && scope.known == 0
            && now.saturating_sub(self.control_checked) >= 6 * 60 * MINUTE
            && !cancelled.load(Ordering::Acquire)
        {
            self.control_checked = now;
            let control_result = |result: Result<Vec<ThreadUsageGroup>, String>| {
                result.map_or_else(
                    |reason| reason,
                    |rows| {
                        if rows
                            .iter()
                            .any(|r| group_usage(r).is_some_and(|(u, _)| u.total_tokens > 0))
                        {
                            "positiveTokens".into()
                        } else {
                            "zeroOrIncomplete".into()
                        }
                    },
                )
            };
            if let Some(control) = local_control_thread(codex_home, workspace) {
                self.local_control = Some(control_result(read(&control, Duration::from_secs(4))));
            }
        }
        scope.local_control.clone_from(&self.local_control);
        if scope.queried > 0 {
            scope.last_checked_at_ms = Some(now);
            self.last_report = Some(scope.clone());
        } else if let Some(previous) = self
            .last_report
            .as_ref()
            .filter(|prev| prev.updated_for_date == scope.updated_for_date)
        {
            scope.outcomes.clone_from(&previous.outcomes);
            scope.unavailable = previous.unavailable;
            scope.last_checked_at_ms = previous.last_checked_at_ms;
        }
        if scope.unavailable > 0 {
            batch.warning = Some(format!(
                "最近查询的 Work 会话中，有 {} 个暂时缺少完整的 Token 明细。",
                scope.unavailable
            ));
        }
        batch
    }

    /// A failed index commit must leave successful reads eligible for replay.
    pub(crate) fn acknowledge(&mut self, batch: &HostedUsageBatch) {
        if self.account_key != batch.account_key {
            return;
        }
        for (id, rev) in &batch.with_totals {
            if let Some(check) = self.checks.get_mut(id) {
                check.with_total_at = Some(check.with_total_at.unwrap_or(0).max(*rev));
            }
        }
        for (id, revision, attempted_at) in &batch.successes {
            if let Some(check) = self.checks.get_mut(id) {
                if check.attempted_at == *attempted_at && check.attempted_revision == *revision {
                    check.needs_retry = false;
                    check.complete_at = Some(*revision);
                }
            }
        }
    }

    fn due_candidates<'a>(&self, candidates: &'a [Candidate], now: i64) -> Vec<&'a Candidate> {
        let mut due: Vec<_> = candidates
            .iter()
            .filter(|row| {
                let Some(check) = self.checks.get(&row.id) else {
                    return true;
                };
                if row.updated > check.attempted_revision {
                    return true;
                }
                // Service-side billing may settle after catalog activity stops.
                let recent = now.saturating_sub(row.updated) <= 2 * DAY;
                let interval = match (recent, check.needs_retry) {
                    (true, true) => 5 * MINUTE,
                    (true, false) => 10 * MINUTE,
                    (false, true) => 6 * 60 * MINUTE,
                    (false, false) => 7 * DAY,
                };
                now.saturating_sub(check.attempted_at) >= interval
            })
            .collect();
        // New activity is always first. Reserve the second slot for a never- or
        // least-recently-polled thread even if the latest threads stay active.
        due.sort_by_key(|row| std::cmp::Reverse(row.updated));
        if due.len() > 2 {
            let oldest = due[1..]
                .iter()
                .enumerate()
                .min_by_key(|(_, row)| {
                    self.checks
                        .get(&row.id)
                        .map_or(0, |check| check.attempted_at)
                })
                .map(|(pos, _)| pos + 1)
                .unwrap_or(1);
            due.swap(1, oldest);
        }
        due
    }
}

fn open_readonly(path: &Path) -> rusqlite::Result<Connection> {
    let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    db.busy_timeout(Duration::from_millis(250))?;
    Ok(db)
}

// The control only compares service responses. Its usage is never written to the index.
fn local_control_thread(home: &Path, workspace: Option<&str>) -> Option<String> {
    let catalog = open_readonly(&home.join("sqlite").join("codex.db")).ok()?;
    let mut stmt = catalog
        .prepare(
            "SELECT DISTINCT host_id FROM local_thread_catalog_hosts WHERE host_kind='chatgpt'",
        )
        .ok()?;
    let hosts = stmt
        .query_map([], |r| r.get::<_, String>(0))
        .ok()?
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    let mut pairs = hosts
        .iter()
        .filter_map(|h| h.strip_prefix("chatgpt:")?.split_once(':'))
        .filter(|(account, _)| workspace.map_or(true, |id| id == *account));
    let (account, user) = pairs.next()?;
    if pairs.next().is_some() {
        return None;
    }
    let db = open_readonly(&home.join("state_5.sqlite")).ok()?;
    db.query_row(
        "SELECT id FROM threads WHERE creator_account_id=?1 AND creator_user_id=?2
         AND originator='codex_work_desktop' AND tokens_used>0 AND has_user_event=1
         AND updated_at < unixepoch('now','-1 day') ORDER BY updated_at DESC LIMIT 1",
        [account, user],
        |r| r.get(0),
    )
    .ok()
}

#[derive(Debug)]
enum CatalogError {
    Unreadable,
    AmbiguousHost,
}

impl From<rusqlite::Error> for CatalogError {
    fn from(_: rusqlite::Error) -> Self {
        Self::Unreadable
    }
}

/// The desktop catalog contains ordinary chats as well. Select Work threads
/// by origin or local project binding without reading titles or conversations.
fn discover(codex_home: &Path, workspace: Option<&str>) -> Result<Vec<Candidate>, CatalogError> {
    let db = open_readonly(&codex_home.join("sqlite").join("codex.db"))?;
    let mut hosts_query = db.prepare(
        "SELECT DISTINCT host_id FROM local_thread_catalog_hosts WHERE host_kind='chatgpt'",
    )?;
    let hosts = hosts_query
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut matching = hosts
        .iter()
        .filter(|host| workspace.map_or(true, |account| host.split(':').nth(1) == Some(account)));
    let Some(host) = matching.next() else {
        return Ok(vec![]);
    };
    if matching.next().is_some() {
        return Err(CatalogError::AmbiguousHost);
    }
    let projects = project_roots(codex_home);
    let mut stmt = db.prepare(
        "SELECT c.thread_id, c.source_created_at, c.source_updated_at, c.cwd,
                c.host_id, c.project_id, c.conversation_origin
         FROM local_thread_catalog c JOIN local_thread_catalog_hosts h ON h.host_id=c.host_id
         WHERE c.host_id=?1 AND h.host_kind='chatgpt' AND c.source_kind='chatgpt' AND c.missing_candidate=0
           AND (c.conversation_origin='tpp' OR c.project_id IS NOT NULL)
         ORDER BY c.source_updated_at DESC, c.thread_id"
    )?;
    let mut rows = stmt.query([host])?;
    let mut by_id = HashMap::new();
    while let Some(row) = rows.next()? {
        let (Some(created), Some(updated)) =
            (row.get::<_, Option<f64>>(1)?, row.get::<_, Option<f64>>(2)?)
        else {
            continue;
        };
        if !(created.is_finite()
            && updated.is_finite()
            && created > 0.0
            && updated >= created
            && updated < 253_402_300_800.0)
        {
            continue;
        }
        let id: String = row.get(0)?;
        if id.is_empty()
            || id.len() > 128
            || !id
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
        {
            continue;
        }
        let host: String = row.get(4)?;
        let project: Option<String> = row.get(5)?;
        let root = project.and_then(|id| projects.get(&(host, id)));
        if root.is_none() && row.get::<_, Option<String>>(6)?.as_deref() != Some("tpp") {
            continue;
        }
        let cwd: Option<String> = row.get(3)?;
        by_id.entry(id.clone()).or_insert_with(|| Candidate {
            id,
            cwd: cwd
                .filter(|path| !path.is_empty())
                .or_else(|| root.cloned().flatten()),
            created: (created * 1000.0).round() as i64,
            updated: (updated * 1000.0).round() as i64,
        });
    }
    let mut candidates: Vec<_> = by_id.into_values().collect();
    candidates.sort_by(|a, b| b.updated.cmp(&a.updated).then_with(|| a.id.cmp(&b.id)));
    Ok(candidates)
}

fn project_roots(codex_home: &Path) -> HashMap<(String, String), Option<String>> {
    let mut projects = HashMap::new();
    let Ok(db) = open_readonly(&codex_home.join("state_5.sqlite")) else {
        return projects;
    };
    let Ok(mut stmt) = db.prepare(
        r#"SELECT json_extract(p.metadata,'$."codex.chatGptBacking"'), r.path
         FROM projects p JOIN project_roots r ON r.project_id=p.id WHERE json_valid(p.metadata)"#,
    ) else {
        return projects;
    };
    let Ok(rows) = stmt.query_map([], |r| {
        Ok((r.get::<_, Option<String>>(0)?, r.get::<_, String>(1)?))
    }) else {
        return projects;
    };
    for (backing, path) in rows.flatten() {
        let Some(backing) = backing else { continue };
        let Ok(binding) = serde_json::from_str::<serde_json::Value>(&backing) else {
            continue;
        };
        let (Some(account), Some(user), Some(project)) = (
            binding["accountId"].as_str(),
            binding["userId"].as_str(),
            binding["projectId"].as_str(),
        ) else {
            continue;
        };
        let key = (format!("chatgpt:{account}:{user}"), project.to_owned());
        projects
            .entry(key)
            .and_modify(|root| {
                if root.as_deref() != Some(path.as_str()) {
                    *root = None; // A multi-root project has no unique working directory.
                }
            })
            .or_insert(Some(path));
    }
    projects
}

fn group_usage(group: &ThreadUsageGroup) -> Option<(TokenUsage, bool)> {
    let split_input = match (group.net_new_input_tokens, group.cached_input_tokens) {
        (Some(fresh), Some(cached)) => Some(fresh.checked_add(cached)?),
        _ => None,
    };
    if matches!((group.input_tokens, split_input), (Some(a), Some(b)) if a != b) {
        return None;
    }
    let input = group
        .input_tokens
        .or(split_input)
        .or_else(|| group.total_tokens?.checked_sub(group.output_tokens?));
    let output = group
        .output_tokens
        .or_else(|| group.total_tokens?.checked_sub(input?));
    let known_input = input
        .or(group.cached_input_tokens)
        .or(group.net_new_input_tokens);
    if known_input.is_none() && output.is_none() && group.total_tokens.is_none() {
        return None;
    }
    let lower_bound = known_input.unwrap_or(0).checked_add(output.unwrap_or(0))?;
    let total = group.total_tokens.unwrap_or(lower_bound);
    if total < lower_bound
        || matches!((input, output), (Some(_), Some(_)) if total != lower_bound)
        || input.is_some_and(|n| {
            group.cached_input_tokens.is_some_and(|c| c > n)
                || group.net_new_input_tokens.is_some_and(|f| f > n)
        })
    {
        return None;
    }
    let cached = group
        .cached_input_tokens
        .or_else(|| input?.checked_sub(group.net_new_input_tokens?))
        .or_else(|| (input == Some(0)).then_some(0));
    let mut usage = TokenUsage {
        total_tokens: total.try_into().ok()?,
        ..Default::default()
    };
    if let (Some(input), Some(cached), Some(output)) = (input, cached, output) {
        usage.input_tokens = input.try_into().ok()?;
        usage.cached_input_tokens = cached.try_into().ok()?;
        usage.output_tokens = output.try_into().ok()?;
        return Some((usage, true));
    }
    Some((usage, false))
}

fn observations(
    candidate: &Candidate,
    groups: Vec<ThreadUsageGroup>,
) -> (Vec<WorkUsageObservation>, bool) {
    let mut models: BTreeMap<String, (TokenUsage, bool, bool)> = BTreeMap::new();
    let mut complete = !groups.is_empty();
    for group in groups {
        let Some(model) = group
            .model
            .as_deref()
            .map(str::trim)
            .filter(|m| !m.is_empty())
        else {
            complete = false;
            continue;
        };
        let (sum, all_splits, observed) = models.entry(model.to_ascii_lowercase()).or_insert((
            TokenUsage::default(),
            true,
            false,
        ));
        let Some((usage, split)) = group_usage(&group) else {
            *all_splits = false;
            complete = false;
            continue;
        };
        let Some(total) = sum.total_tokens.checked_add(usage.total_tokens) else {
            return (Vec::new(), false);
        };
        sum.total_tokens = total;
        // Non-negative component totals cannot exceed the already checked sum.
        sum.input_tokens += usage.input_tokens;
        sum.cached_input_tokens += usage.cached_input_tokens;
        sum.output_tokens += usage.output_tokens;
        *observed = true;
        *all_splits &= split;
        complete &= split;
    }
    let observations = models
        .into_iter()
        .filter(|(_, (_, _, observed))| *observed)
        .map(|(model, (usage, split, _))| WorkUsageObservation {
            thread_id: candidate.id.clone(),
            model,
            cwd: candidate.cwd.clone(),
            created_at_ms: candidate.created,
            updated_at_ms: candidate.updated,
            usage: if split {
                usage
            } else {
                TokenUsage {
                    total_tokens: usage.total_tokens,
                    ..Default::default()
                }
            },
            cache_metrics_available: split,
        })
        .collect();
    (observations, complete)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn group(model: &str, total: u64) -> ThreadUsageGroup {
        ThreadUsageGroup {
            model: Some(model.into()),
            reasoning_effort: None,
            speed: None,
            net_new_input_tokens: None,
            cached_input_tokens: None,
            input_tokens: None,
            output_tokens: None,
            total_tokens: Some(total),
        }
    }

    #[test]
    fn new_activity_and_unpolled_history_get_slots_even_when_recent_threads_retry() {
        let now = 5 * DAY;
        let rows: Vec<_> = [("new", now), ("retry", now - 1), ("older", DAY)]
            .into_iter()
            .map(|(id, updated)| Candidate {
                id: id.into(),
                cwd: None,
                created: DAY,
                updated,
            })
            .collect();
        let mut poller = HostedUsagePoller::default();
        poller.checks.insert(
            "retry".into(),
            Check {
                attempted_at: now - 6 * MINUTE,
                attempted_revision: now - 1,
                needs_retry: true,
                ..Default::default()
            },
        );
        let next: Vec<_> = poller
            .due_candidates(&rows, now)
            .into_iter()
            .map(|row| row.id.as_str())
            .collect();
        assert_eq!(next, ["new", "older", "retry"]);
    }

    #[test]
    fn partial_totals_do_not_acquire_synthetic_price_components() {
        let candidate = Candidate {
            id: "test".into(),
            cwd: None,
            created: 1,
            updated: 2,
        };
        let full = ThreadUsageGroup {
            net_new_input_tokens: Some(2),
            cached_input_tokens: Some(6),
            output_tokens: Some(1),
            ..group("model-a", 9)
        };
        let (rows, full_coverage) = observations(
            &candidate,
            vec![
                full.clone(),
                group("model-a", 5),
                full.clone(),
                ThreadUsageGroup {
                    total_tokens: None,
                    output_tokens: Some(3),
                    ..group("model-b", 0)
                },
            ],
        );
        assert!(!full_coverage);
        assert_eq!(
            rows[0].usage,
            TokenUsage {
                total_tokens: 23,
                ..Default::default()
            }
        );
        assert!(!rows[0].cache_metrics_available);
        assert_eq!(rows[1].usage.total_tokens, 3);
        let (rows, full_coverage) = observations(&candidate, vec![full]);
        assert!(full_coverage);
        assert_eq!(rows[0].usage.input_tokens, 8);
        assert_eq!(rows[0].usage.cached_input_tokens, 6);
        let invalid = ThreadUsageGroup {
            input_tokens: Some(10),
            ..group("model-a", 2)
        };
        assert!(group_usage(&invalid).is_none());
    }

    #[test]
    fn catalog_selection_project_binding_and_commit_acknowledgement() {
        let temp = tempdir().unwrap();
        let dir = temp.path().join("sqlite");
        std::fs::create_dir(&dir).unwrap();
        let catalog = Connection::open(dir.join("codex.db")).unwrap();
        catalog.execute_batch(
            "CREATE TABLE local_thread_catalog_hosts(host_id TEXT,host_kind TEXT);
             INSERT INTO local_thread_catalog_hosts VALUES('chatgpt:a:u','chatgpt');
             CREATE TABLE local_thread_catalog(thread_id TEXT,source_created_at REAL,source_updated_at REAL,
               cwd TEXT,host_id TEXT,project_id TEXT,conversation_origin TEXT,source_kind TEXT,missing_candidate INTEGER);
             INSERT INTO local_thread_catalog VALUES
               ('project',100,102,NULL,'chatgpt:a:u','g1',NULL,'chatgpt',0),
               ('work',100,103,NULL,'chatgpt:a:u',NULL,'tpp','chatgpt',0),
               ('chat',100,104,NULL,'chatgpt:a:u',NULL,NULL,'chatgpt',0),
               ('removed',100,105,NULL,'chatgpt:a:u',NULL,'tpp','chatgpt',1);"
        ).unwrap();
        let state = Connection::open(temp.path().join("state_5.sqlite")).unwrap();
        state.execute_batch("CREATE TABLE projects(id TEXT,metadata TEXT); CREATE TABLE project_roots(project_id TEXT,path TEXT);
            INSERT INTO project_roots VALUES('local','test-root');").unwrap();
        let backing = serde_json::json!({"accountId":"a","userId":"u","projectId":"g1"});
        let metadata = serde_json::json!({"codex.chatGptBacking":backing.to_string()});
        state
            .execute(
                "INSERT INTO projects VALUES('local',?1)",
                [metadata.to_string()],
            )
            .unwrap();
        let candidates = discover(temp.path(), None).unwrap();
        assert_eq!(candidates.len(), 2);
        assert_eq!(candidates[0].id, "work");
        assert_eq!(candidates[1].cwd.as_deref(), Some("test-root"));
        let offline = catalog_only_coverage(temp.path()).unwrap();
        assert_eq!(offline.candidates, Some(2));
        assert_eq!(offline.today_attempted, 0);
        assert_eq!(offline.scope.as_deref(), Some("catalogOnly"));
        catalog
            .execute(
                "INSERT INTO local_thread_catalog_hosts VALUES('chatgpt:b:u','chatgpt')",
                [],
            )
            .unwrap();
        assert!(matches!(
            discover(temp.path(), None),
            Err(CatalogError::AmbiguousHost)
        ));
        assert!(catalog_only_coverage(temp.path()).is_none());
        assert!(discover(temp.path(), Some("other")).unwrap().is_empty());

        let mut poller = HostedUsagePoller::default();
        let cancel = AtomicBool::new(false);
        let batch = poller.poll_with(temp.path(), "a", Some("a"), &cancel, |id, _| {
            if id == "work" {
                return Err("threadUsageNull".into());
            }
            Ok(vec![ThreadUsageGroup {
                cached_input_tokens: Some(3),
                input_tokens: Some(4),
                output_tokens: Some(2),
                ..group("model", 6)
            }])
        });
        assert_eq!(batch.coverage.candidates, Some(2));
        assert_eq!(batch.coverage.unavailable, 1);
        assert_eq!(batch.coverage.known, 1);
        assert_eq!(batch.observations.len(), 1);
        assert!(poller.checks["project"].needs_retry);
        poller.acknowledge(&batch);
        assert!(!poller.checks["project"].needs_retry);
        assert!(poller.checks["work"].needs_retry);

        state
            .execute_batch(
                "CREATE TABLE threads(id TEXT,creator_account_id TEXT,creator_user_id TEXT,
            originator TEXT,tokens_used INTEGER,has_user_event INTEGER,updated_at INTEGER);
            INSERT INTO threads VALUES('local-control','a','u','codex_work_desktop',400,1,102);",
            )
            .unwrap();
        let mut diagnostics = HostedUsagePoller::default();
        let mut calls = Vec::new();
        let failed = diagnostics.poll_with(temp.path(), "a", Some("a"), &cancel, |id, _| {
            calls.push(id.to_owned());
            if id == "local-control" {
                return Ok(vec![group("model", 3)]);
            }
            Err("groupsEmpty".into())
        });
        assert!(failed.observations.is_empty());
        assert_eq!(calls, ["work", "project", "local-control"]);
        assert_eq!(
            failed.coverage.local_control.as_deref(),
            Some("positiveTokens")
        );
        assert_eq!(failed.coverage.outcomes["groupsEmpty"], 2);
        let idle = diagnostics.poll_with(temp.path(), "a", Some("a"), &cancel, |_, _| {
            panic!("unexpected retry")
        });
        assert_eq!(idle.coverage.queried, 0);
        assert_eq!(idle.coverage.outcomes["groupsEmpty"], 2);
        assert_eq!(
            idle.coverage.updated_for_date,
            Local::now().date_naive().to_string()
        );
        assert!(!serde_json::to_string(&idle.coverage)
            .unwrap()
            .contains("local-control"));
    }
}
