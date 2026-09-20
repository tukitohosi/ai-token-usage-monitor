use std::{
    env, fs,
    io::{self, BufRead, BufReader, Write},
    path::PathBuf,
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc::{self, Receiver, RecvTimeoutError},
    thread,
    time::{Duration, Instant, SystemTime},
};

use serde::Serialize;
use serde_json::{json, Map, Value};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const SAFE_TEXT_LIMIT: usize = 256;
const ALLOWED_REQUEST_METHODS: &[&str] = &[
    "initialize",
    "account/read",
    "account/rateLimits/read",
    "account/usage/read",
];
const ALLOWED_NOTIFICATION_METHODS: &[&str] = &["initialized"];

#[derive(Debug)]
pub(crate) enum AppServerError {
    NotFound,
    Io,
    Timeout,
    Protocol,
    Rpc { code: Option<i64> },
}

impl AppServerError {
    pub(crate) fn is_unsupported_method(&self) -> bool {
        matches!(self, Self::Rpc { code: Some(-32601) })
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CreditBalance {
    pub(crate) has_credits: bool,
    pub(crate) unlimited: bool,
    pub(crate) balance: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NormalizedQuotaWindow {
    pub(crate) used_percent: f64,
    pub(crate) window_duration_mins: f64,
    pub(crate) resets_at: f64,
    pub(crate) key: String,
    pub(crate) limit_id: String,
    pub(crate) limit_name: Option<String>,
    pub(crate) normal_model_slug: Option<String>,
    pub(crate) lane: &'static str,
    pub(crate) label: String,
    pub(crate) remaining_percent: f64,
    pub(crate) credits: Option<CreditBalance>,
    pub(crate) plan_type: Option<String>,
    pub(crate) reached_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ResetCredit {
    pub(crate) id: String,
    pub(crate) reset_type: String,
    pub(crate) status: String,
    pub(crate) granted_at: f64,
    pub(crate) expires_at: Option<f64>,
    pub(crate) title: Option<String>,
    pub(crate) description: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RateLimitResetCredits {
    pub(crate) available_count: u64,
    pub(crate) credits: Option<Vec<ResetCredit>>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AccountUsageSummary {
    pub(crate) lifetime_tokens: Option<u64>,
    pub(crate) peak_daily_tokens: Option<u64>,
    pub(crate) longest_running_turn_sec: Option<u64>,
    pub(crate) current_streak_days: Option<u64>,
    pub(crate) longest_streak_days: Option<u64>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DailyUsageBucket {
    pub(crate) start_date: String,
    pub(crate) tokens: u64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AccountUsageReadResult {
    pub(crate) summary: Option<AccountUsageSummary>,
    pub(crate) daily_usage_buckets: Option<Vec<DailyUsageBucket>>,
    // Thread-level usage is intentionally not forwarded. It can contain stable
    // identifiers and is not needed by the dashboard.
    pub(crate) thread_usage: Option<()>,
}

#[derive(Debug)]
pub(crate) struct AccountState {
    pub(crate) account_present: bool,
    pub(crate) requires_openai_auth: bool,
}

enum ReaderMessage {
    Json(Value),
    ProtocolFailure,
    End,
}

pub(crate) struct AppServerClient {
    child: Child,
    stdin: ChildStdin,
    receiver: Receiver<ReaderMessage>,
    next_request_id: u64,
    rpc_methods: Vec<String>,
    pub(crate) server_user_agent: String,
}

impl AppServerClient {
    pub(crate) fn connect() -> Result<Self, AppServerError> {
        let executable = locate_codex_executable()?;
        let mut command = Command::new(executable);
        command
            .args(["app-server", "--stdio"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // Keep refreshes invisible even when the desktop app was started from
            // Explorer rather than a terminal.
            command.creation_flags(0x0800_0000);
        }

        let mut child = command.spawn().map_err(|_| AppServerError::Io)?;
        let stdin = child.stdin.take().ok_or(AppServerError::Io)?;
        let stdout = child.stdout.take().ok_or(AppServerError::Io)?;
        let stderr = child.stderr.take().ok_or(AppServerError::Io)?;
        let (sender, receiver) = mpsc::channel();

        thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines() {
                match line {
                    Ok(line) if line.trim().is_empty() => continue,
                    Ok(line) => match serde_json::from_str::<Value>(&line) {
                        Ok(value) if value.is_object() => {
                            if sender.send(ReaderMessage::Json(value)).is_err() {
                                return;
                            }
                        }
                        _ => {
                            let _ = sender.send(ReaderMessage::ProtocolFailure);
                            return;
                        }
                    },
                    Err(_) => {
                        let _ = sender.send(ReaderMessage::ProtocolFailure);
                        return;
                    }
                }
            }
            let _ = sender.send(ReaderMessage::End);
        });

        // Codex may write local paths or account-adjacent diagnostics to stderr.
        // Drain it so the child cannot block, but never retain or log the bytes.
        thread::spawn(move || {
            let mut reader = BufReader::new(stderr);
            let _ = io::copy(&mut reader, &mut io::sink());
        });

        let mut client = Self {
            child,
            stdin,
            receiver,
            next_request_id: 1,
            rpc_methods: Vec::new(),
            server_user_agent: String::new(),
        };
        let initialized = client.request(
            "initialize",
            Some(json!({
              "clientInfo": {
                "name": "codex-usage-monitor",
                "title": "Codex Usage Monitor",
                "version": env!("CARGO_PKG_VERSION")
              },
              "capabilities": {
                "experimentalApi": false,
                "requestAttestation": false
              }
            })),
        )?;
        client.server_user_agent = initialized
            .get("userAgent")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or(AppServerError::Protocol)?;
        client.notify("initialized", None)?;
        Ok(client)
    }

    pub(crate) fn account_read(&mut self) -> Result<AccountState, AppServerError> {
        let value = self.request("account/read", Some(json!({ "refreshToken": false })))?;
        let requires_openai_auth = value
            .get("requiresOpenaiAuth")
            .and_then(Value::as_bool)
            .ok_or(AppServerError::Protocol)?;
        Ok(AccountState {
            account_present: value
                .get("account")
                .is_some_and(|account| !account.is_null()),
            requires_openai_auth,
        })
    }

    pub(crate) fn rate_limits_read(
        &mut self,
    ) -> Result<(Vec<NormalizedQuotaWindow>, Option<RateLimitResetCredits>), AppServerError> {
        let value = self.request("account/rateLimits/read", Some(json!({})))?;
        Ok(normalize_rate_limits(&value))
    }

    pub(crate) fn usage_read(&mut self) -> Result<AccountUsageReadResult, AppServerError> {
        let value = self.request("account/usage/read", Some(json!({})))?;
        Ok(sanitize_account_usage(&value))
    }

    pub(crate) fn account_rpc_methods(&self) -> Vec<String> {
        self.rpc_methods
            .iter()
            .filter(|method| method.starts_with("account/"))
            .cloned()
            .collect()
    }

    fn notify(&mut self, method: &str, params: Option<Value>) -> Result<(), AppServerError> {
        if !ALLOWED_NOTIFICATION_METHODS.contains(&method) {
            return Err(AppServerError::Protocol);
        }
        self.rpc_methods.push(method.to_owned());
        let mut message = Map::new();
        message.insert("method".to_owned(), Value::String(method.to_owned()));
        if let Some(params) = params {
            message.insert("params".to_owned(), params);
        }
        write_message(&mut self.stdin, &Value::Object(message))
    }

    fn request(&mut self, method: &str, params: Option<Value>) -> Result<Value, AppServerError> {
        if !ALLOWED_REQUEST_METHODS.contains(&method) {
            return Err(AppServerError::Protocol);
        }
        self.rpc_methods.push(method.to_owned());
        let id = self.next_request_id;
        self.next_request_id += 1;
        let mut message = Map::new();
        message.insert("method".to_owned(), Value::String(method.to_owned()));
        message.insert("id".to_owned(), Value::Number(id.into()));
        if let Some(params) = params {
            message.insert("params".to_owned(), params);
        }
        write_message(&mut self.stdin, &Value::Object(message))?;

        let deadline = Instant::now() + REQUEST_TIMEOUT;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(AppServerError::Timeout);
            }
            match self.receiver.recv_timeout(remaining) {
                Ok(ReaderMessage::Json(value)) => {
                    if value.get("id").and_then(Value::as_u64) != Some(id) {
                        // Notifications are invalidation signals only. A fresh snapshot is
                        // already being read, so there is no raw payload to forward.
                        continue;
                    }
                    if let Some(error) = value.get("error") {
                        return Err(AppServerError::Rpc {
                            code: error.get("code").and_then(Value::as_i64),
                        });
                    }
                    return value.get("result").cloned().ok_or(AppServerError::Protocol);
                }
                Ok(ReaderMessage::ProtocolFailure | ReaderMessage::End) => {
                    return Err(AppServerError::Protocol)
                }
                Err(RecvTimeoutError::Timeout) => return Err(AppServerError::Timeout),
                Err(RecvTimeoutError::Disconnected) => return Err(AppServerError::Io),
            }
        }
    }
}

impl Drop for AppServerClient {
    fn drop(&mut self) {
        let _ = self.stdin.flush();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn write_message(stdin: &mut ChildStdin, value: &Value) -> Result<(), AppServerError> {
    serde_json::to_writer(&mut *stdin, value).map_err(|_| AppServerError::Protocol)?;
    stdin.write_all(b"\n").map_err(|_| AppServerError::Io)?;
    stdin.flush().map_err(|_| AppServerError::Io)
}

fn locate_codex_executable() -> Result<PathBuf, AppServerError> {
    if let Some(explicit) = env::var_os("CODEX_USAGE_CODEX_PATH") {
        let candidate = PathBuf::from(explicit);
        return candidate
            .is_file()
            .then_some(candidate)
            .ok_or(AppServerError::NotFound);
    }

    if let Some(path_value) = env::var_os("PATH") {
        for directory in env::split_paths(&path_value) {
            for executable_name in executable_names() {
                let candidate = directory.join(executable_name);
                if candidate.is_file() {
                    return Ok(candidate);
                }
            }
        }
    }

    #[cfg(windows)]
    if let Some(local_app_data) = env::var_os("LOCALAPPDATA") {
        let bin_directory = PathBuf::from(local_app_data)
            .join("OpenAI")
            .join("Codex")
            .join("bin");
        let direct = bin_directory.join("codex.exe");
        if direct.is_file() {
            return Ok(direct);
        }

        let mut candidates = fs::read_dir(&bin_directory)
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.len() < 8 || !name.chars().all(|character| character.is_ascii_hexdigit()) {
                    return None;
                }
                let executable = entry.path().join("codex.exe");
                executable.is_file().then(|| {
                    let modified = executable
                        .metadata()
                        .and_then(|metadata| metadata.modified())
                        .unwrap_or(SystemTime::UNIX_EPOCH);
                    (modified, executable)
                })
            })
            .collect::<Vec<_>>();
        candidates.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| right.1.cmp(&left.1)));
        if let Some((_, executable)) = candidates.into_iter().next() {
            return Ok(executable);
        }
    }

    Err(AppServerError::NotFound)
}

#[cfg(windows)]
fn executable_names() -> &'static [&'static str] {
    &["codex.exe"]
}

#[cfg(not(windows))]
fn executable_names() -> &'static [&'static str] {
    &["codex"]
}

pub(crate) fn version_from_user_agent(user_agent: &str) -> Option<String> {
    user_agent
        .split(|character: char| {
            !(character.is_ascii_alphanumeric() || matches!(character, '.' | '-'))
        })
        .find(|part| {
            part.chars()
                .next()
                .is_some_and(|character| character.is_ascii_digit())
                && part.chars().filter(|character| *character == '.').count() >= 2
        })
        .map(str::to_owned)
}

fn normalize_rate_limits(
    value: &Value,
) -> (Vec<NormalizedQuotaWindow>, Option<RateLimitResetCredits>) {
    let mut buckets: Vec<(String, &Value)> = value
        .get("rateLimitsByLimitId")
        .and_then(Value::as_object)
        .map(|map| {
            map.iter()
                .filter(|(_, bucket)| bucket.is_object())
                .map(|(limit_id, bucket)| (limit_id.clone(), bucket))
                .collect()
        })
        .unwrap_or_default();
    if buckets.is_empty() {
        if let Some(bucket) = value.get("rateLimits").filter(|bucket| bucket.is_object()) {
            buckets.push(("legacy".to_owned(), bucket));
        }
    }

    let mut windows = Vec::new();
    for (fallback_limit_id, bucket) in buckets {
        let limit_id = safe_text(bucket.get("limitId"))
            .filter(|inner| inner == &fallback_limit_id)
            .unwrap_or(fallback_limit_id);
        let limit_name = safe_text(bucket.get("limitName"));
        let normal_model_slug = safe_text(bucket.get("normalModelSlug"));
        let credits = sanitize_credits(bucket.get("credits"));
        let plan_type = safe_text(bucket.get("planType"));
        let reached_type = safe_text(bucket.get("rateLimitReachedType"));
        for (lane, raw_window) in [
            ("primary", bucket.get("primary")),
            ("secondary", bucket.get("secondary")),
        ] {
            let Some(raw_window) = raw_window else {
                continue;
            };
            let Some(used_percent) = finite_number(raw_window.get("usedPercent")) else {
                continue;
            };
            let Some(window_duration_mins) = finite_number(raw_window.get("windowDurationMins"))
            else {
                continue;
            };
            let Some(resets_at) = finite_number(raw_window.get("resetsAt")) else {
                continue;
            };
            if window_duration_mins <= 0.0 || resets_at < 0.0 {
                continue;
            }
            let bounded_used = used_percent.clamp(0.0, 100.0);
            windows.push(NormalizedQuotaWindow {
                used_percent: bounded_used,
                window_duration_mins,
                resets_at,
                key: format!("{limit_id}:{lane}"),
                limit_id: limit_id.clone(),
                limit_name: limit_name.clone(),
                normal_model_slug: normal_model_slug.clone(),
                lane,
                label: format_window_duration(window_duration_mins),
                remaining_percent: 100.0 - bounded_used,
                credits: credits.clone(),
                plan_type: plan_type.clone(),
                reached_type: reached_type.clone(),
            });
        }
    }
    windows.sort_by(|left, right| {
        quota_window_priority(left)
            .cmp(&quota_window_priority(right))
            .then_with(|| left.key.cmp(&right.key))
    });

    (
        windows,
        sanitize_reset_credits(value.get("rateLimitResetCredits")),
    )
}

fn quota_window_priority(window: &NormalizedQuotaWindow) -> u8 {
    let codex = window.limit_id.eq_ignore_ascii_case("codex")
        || window
            .limit_name
            .as_deref()
            .is_some_and(|name| name.eq_ignore_ascii_case("codex"));
    if codex && window.window_duration_mins == 300.0 {
        return 0;
    }
    if codex && window.window_duration_mins == 10_080.0 {
        return 1;
    }
    let luna_reserve = window.limit_id.eq_ignore_ascii_case("base_model_inference")
        || window
            .limit_name
            .as_deref()
            .is_some_and(|name| name.eq_ignore_ascii_case("gpt-reserve"))
        || window
            .normal_model_slug
            .as_deref()
            .is_some_and(|model| model.eq_ignore_ascii_case("gpt-5.6-luna"));
    if luna_reserve {
        2
    } else {
        3
    }
}

fn sanitize_credits(value: Option<&Value>) -> Option<CreditBalance> {
    let value = value?.as_object()?;
    Some(CreditBalance {
        has_credits: value.get("hasCredits")?.as_bool()?,
        unlimited: value.get("unlimited")?.as_bool()?,
        balance: safe_text(value.get("balance")),
    })
}

fn sanitize_reset_credits(value: Option<&Value>) -> Option<RateLimitResetCredits> {
    let value = value?.as_object()?;
    let available_count = non_negative_integer(value.get("availableCount"))?;
    let credits = value.get("credits").and_then(Value::as_array).map(|items| {
        items
            .iter()
            .filter_map(|candidate| {
                Some(ResetCredit {
                    id: safe_text(candidate.get("id"))?,
                    reset_type: safe_text(candidate.get("resetType"))?,
                    status: safe_text(candidate.get("status"))?,
                    granted_at: finite_number(candidate.get("grantedAt"))?,
                    expires_at: finite_number(candidate.get("expiresAt")),
                    title: safe_text(candidate.get("title")),
                    description: safe_text(candidate.get("description")),
                })
            })
            .collect()
    });
    Some(RateLimitResetCredits {
        available_count,
        credits,
    })
}

fn sanitize_account_usage(value: &Value) -> AccountUsageReadResult {
    let summary = value
        .get("summary")
        .and_then(Value::as_object)
        .map(|summary| AccountUsageSummary {
            lifetime_tokens: non_negative_integer(summary.get("lifetimeTokens")),
            peak_daily_tokens: non_negative_integer(summary.get("peakDailyTokens")),
            longest_running_turn_sec: non_negative_integer(summary.get("longestRunningTurnSec")),
            current_streak_days: non_negative_integer(summary.get("currentStreakDays")),
            longest_streak_days: non_negative_integer(summary.get("longestStreakDays")),
        });
    let daily_usage_buckets = value
        .get("dailyUsageBuckets")
        .and_then(Value::as_array)
        .map(|buckets| {
            buckets
                .iter()
                .filter_map(|bucket| {
                    Some(DailyUsageBucket {
                        start_date: safe_text(bucket.get("startDate"))?,
                        tokens: non_negative_integer(bucket.get("tokens"))?,
                    })
                })
                .collect()
        });
    AccountUsageReadResult {
        summary,
        daily_usage_buckets,
        thread_usage: None,
    }
}

fn safe_text(value: Option<&Value>) -> Option<String> {
    let text = value?.as_str()?.trim();
    if text.is_empty() {
        return None;
    }
    Some(text.chars().take(SAFE_TEXT_LIMIT).collect())
}

fn finite_number(value: Option<&Value>) -> Option<f64> {
    value?.as_f64().filter(|number| number.is_finite())
}

fn non_negative_integer(value: Option<&Value>) -> Option<u64> {
    value?.as_u64()
}

fn format_window_duration(minutes: f64) -> String {
    if minutes >= 1_440.0 && minutes % 1_440.0 == 0.0 {
        return format!("{} 天", (minutes / 1_440.0) as u64);
    }
    if minutes >= 60.0 && minutes % 60.0 == 0.0 {
        return format!("{} 小时", (minutes / 60.0) as u64);
    }
    format!("{} 分钟", minutes as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_remaining_quota_and_uses_map_key_as_identity() {
        let response = json!({
          "rateLimitsByLimitId": {
            "codex": {
              "limitId": "inconsistent",
              "limitName": "Codex",
              "normalModelSlug": "gpt-5.6-sol",
              "planType": "plus",
              "primary": { "usedPercent": 38.5, "windowDurationMins": 300, "resetsAt": 1_800_000_000 },
              "secondary": { "usedPercent": 12, "windowDurationMins": 10080, "resetsAt": 1_800_000_100 }
            }
          }
        });
        let (windows, _) = normalize_rate_limits(&response);
        assert_eq!(windows.len(), 2);
        assert_eq!(windows[0].limit_id, "codex");
        assert_eq!(windows[0].remaining_percent, 61.5);
        assert_eq!(windows[0].normal_model_slug.as_deref(), Some("gpt-5.6-sol"));
        assert_eq!(windows[1].label, "7 天");
    }

    #[test]
    fn sorts_known_quota_windows_ahead_of_unknown_buckets() {
        let response = json!({
          "rateLimitsByLimitId": {
            "base_model_inference": {
              "limitName": "gpt-reserve",
              "normalModelSlug": "gpt-5.6-luna",
              "primary": { "usedPercent": 100, "windowDurationMins": 10080, "resetsAt": 30 }
            },
            "future": {
              "primary": { "usedPercent": 1, "windowDurationMins": 60, "resetsAt": 40 }
            },
            "codex": {
              "primary": { "usedPercent": 15, "windowDurationMins": 300, "resetsAt": 10 },
              "secondary": { "usedPercent": 21, "windowDurationMins": 10080, "resetsAt": 20 }
            }
          }
        });
        let (windows, _) = normalize_rate_limits(&response);
        assert_eq!(
            windows
                .iter()
                .map(|window| window.key.as_str())
                .collect::<Vec<_>>(),
            vec![
                "codex:primary",
                "codex:secondary",
                "base_model_inference:primary",
                "future:primary"
            ]
        );
    }

    #[test]
    fn rpc_allowlist_excludes_inference_and_credit_mutation() {
        for method in ["thread/start", "turn/start", "account/rateLimits/reset"] {
            assert!(!ALLOWED_REQUEST_METHODS.contains(&method));
            assert!(!ALLOWED_NOTIFICATION_METHODS.contains(&method));
        }
        assert_eq!(
            ALLOWED_REQUEST_METHODS,
            &[
                "initialize",
                "account/read",
                "account/rateLimits/read",
                "account/usage/read"
            ]
        );
    }

    #[test]
    fn ignores_thread_usage_and_invalid_daily_buckets() {
        let result = sanitize_account_usage(&json!({
          "summary": { "lifetimeTokens": 42 },
          "dailyUsageBuckets": [
            { "startDate": "2026-08-28", "tokens": 11 },
            { "startDate": "", "tokens": 9 }
          ],
          "threadUsage": { "secret-thread-id": 10 }
        }));
        assert_eq!(result.summary.unwrap().lifetime_tokens, Some(42));
        assert_eq!(result.daily_usage_buckets.unwrap().len(), 1);
        assert_eq!(result.thread_usage, None);
    }

    #[test]
    fn extracts_version_without_forwarding_the_full_user_agent() {
        assert_eq!(
            version_from_user_agent("codex/0.150.0-alpha.8 windows"),
            Some("0.150.0-alpha.8".to_owned())
        );
    }
}
