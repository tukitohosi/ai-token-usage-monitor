use chrono::Utc;
use serde::Serialize;
use std::time::Instant;

use crate::app_server::{
    version_from_user_agent, AccountUsageReadResult, AppServerClient, AppServerError,
    NormalizedQuotaWindow, RateLimitResetCredits,
};
use crate::multisource::DeviceUsageSummary;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RefreshProgress {
    pub(crate) phase: &'static str,
    pub(crate) label: String,
    pub(crate) processed_sources: i64,
    pub(crate) total_sources: i64,
    pub(crate) current_source: Option<String>,
    pub(crate) processed_files: i64,
    pub(crate) total_files: i64,
}

impl RefreshProgress {
    pub(crate) fn new(
        phase: &'static str,
        label: impl Into<String>,
        processed_sources: i64,
    ) -> Self {
        Self {
            phase,
            label: label.into(),
            processed_sources,
            total_sources: 6,
            current_source: None,
            processed_files: 0,
            total_files: 0,
        }
    }

    pub(crate) fn with_files(
        mut self,
        current_source: impl Into<String>,
        processed_files: i64,
        total_files: i64,
    ) -> Self {
        self.current_source = Some(current_source.into());
        self.processed_files = processed_files;
        self.total_files = total_files;
        self
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SourceHealthSummary {
    pub(crate) id: String,
    pub(crate) label: String,
    pub(crate) status: String,
    pub(crate) source_files: i64,
    pub(crate) indexed_events: i64,
    pub(crate) new_events: Option<i64>,
    pub(crate) skipped_records: Option<i64>,
    pub(crate) last_indexed_at: Option<String>,
    pub(crate) message: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct IndexDatabaseDiagnostic {
    pub(crate) label: &'static str,
    pub(crate) size_bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct IndexDiagnostics {
    pub(crate) total_duration_ms: u64,
    pub(crate) codex_duration_ms: u64,
    pub(crate) sources_duration_ms: u64,
    pub(crate) account_duration_ms: u64,
    pub(crate) databases: Vec<IndexDatabaseDiagnostic>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RefreshSchedule {
    pub(crate) next_refresh_at_ms: Option<u64>,
    pub(crate) interval_seconds: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AccountReadDiagnostics {
    pub(crate) read_at: String,
    pub(crate) duration_ms: u64,
    pub(crate) methods: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DashboardSnapshot {
    pub(crate) status: &'static str,
    pub(crate) fetched_at: Option<String>,
    pub(crate) codex_version: Option<String>,
    pub(crate) quota_windows: Vec<NormalizedQuotaWindow>,
    pub(crate) account_diagnostics: Option<AccountReadDiagnostics>,
    // App Server does not expose subscription renewal dates. Never substitute a
    // quota-window reset timestamp here.
    pub(crate) plan_renewal_at: Option<String>,
    pub(crate) plan_renewal_source: Option<&'static str>,
    pub(crate) reset_credits: Option<RateLimitResetCredits>,
    pub(crate) account_usage: Option<AccountUsageReadResult>,
    pub(crate) device_usage: Option<DeviceUsageSummary>,
    pub(crate) last_successful_at: Option<String>,
    pub(crate) refresh_progress: Option<RefreshProgress>,
    pub(crate) refresh_schedule: Option<RefreshSchedule>,
    pub(crate) source_health: Vec<SourceHealthSummary>,
    pub(crate) index_diagnostics: Option<IndexDiagnostics>,
    pub(crate) message: Option<String>,
}

impl DashboardSnapshot {
    fn empty(status: &'static str, fetched_at: String, message: &str) -> Self {
        Self {
            status,
            fetched_at: Some(fetched_at),
            codex_version: None,
            quota_windows: Vec::new(),
            account_diagnostics: None,
            plan_renewal_at: None,
            plan_renewal_source: None,
            reset_credits: None,
            account_usage: None,
            device_usage: None,
            last_successful_at: None,
            refresh_progress: None,
            refresh_schedule: None,
            source_health: Vec::new(),
            index_diagnostics: None,
            message: Some(message.to_owned()),
        }
    }
}

pub(crate) fn read_dashboard_snapshot() -> DashboardSnapshot {
    let read_started = Instant::now();
    let fetched_at = Utc::now().to_rfc3339();
    let mut client = match AppServerClient::connect() {
        Ok(client) => client,
        Err(error) => {
            return with_account_diagnostics(
                connection_failure(error, fetched_at.clone()),
                fetched_at,
                read_started,
                Vec::new(),
            )
        }
    };
    let codex_version = version_from_user_agent(&client.server_user_agent);

    let account = match client.account_read() {
        Ok(account) => account,
        Err(error) => {
            let mut snapshot = connection_failure(error, fetched_at.clone());
            snapshot.codex_version = codex_version;
            return with_account_diagnostics(
                snapshot,
                fetched_at,
                read_started,
                client.account_rpc_methods(),
            );
        }
    };
    if account.requires_openai_auth && !account.account_present {
        let mut snapshot = DashboardSnapshot::empty(
            "unauthenticated",
            fetched_at.clone(),
            "Codex 尚未登录；登录后可读取账号额度，本机历史统计仍可使用。",
        );
        snapshot.codex_version = codex_version;
        return with_account_diagnostics(
            snapshot,
            fetched_at,
            read_started,
            client.account_rpc_methods(),
        );
    }

    let rate_limits = client.rate_limits_read();
    let account_usage = client.usage_read();
    if rate_limits.is_err() && account_usage.is_err() {
        let unsupported = rate_limits
            .as_ref()
            .is_err_and(AppServerError::is_unsupported_method)
            && account_usage
                .as_ref()
                .is_err_and(AppServerError::is_unsupported_method);
        let mut snapshot = DashboardSnapshot::empty(
            if unsupported { "unsupported" } else { "error" },
            fetched_at.clone(),
            if unsupported {
                "当前 Codex 版本不支持所需的账号用量接口；本机历史统计仍可使用。"
            } else {
                "账号用量接口读取失败；未记录原始错误内容。"
            },
        );
        snapshot.codex_version = codex_version;
        return with_account_diagnostics(
            snapshot,
            fetched_at,
            read_started,
            client.account_rpc_methods(),
        );
    }

    let partial = rate_limits.is_err() || account_usage.is_err();
    let (quota_windows, reset_credits) = rate_limits.unwrap_or_default();
    with_account_diagnostics(
        DashboardSnapshot {
            status: "ready",
            fetched_at: Some(fetched_at.clone()),
            codex_version,
            quota_windows,
            account_diagnostics: None,
            plan_renewal_at: None,
            plan_renewal_source: None,
            reset_credits,
            account_usage: account_usage.ok(),
            device_usage: None,
            last_successful_at: Some(fetched_at.clone()),
            refresh_progress: None,
            refresh_schedule: None,
            source_health: Vec::new(),
            index_diagnostics: None,
            message: partial.then(|| "部分账号用量暂不可用。".to_owned()),
        },
        fetched_at,
        read_started,
        client.account_rpc_methods(),
    )
}

fn with_account_diagnostics(
    mut snapshot: DashboardSnapshot,
    read_at: String,
    started: Instant,
    methods: Vec<String>,
) -> DashboardSnapshot {
    snapshot.account_diagnostics = Some(AccountReadDiagnostics {
        read_at,
        duration_ms: started.elapsed().as_millis().min(u64::MAX as u128) as u64,
        methods,
    });
    snapshot
}

fn connection_failure(error: AppServerError, fetched_at: String) -> DashboardSnapshot {
    match error {
        AppServerError::NotFound => DashboardSnapshot::empty(
            "unsupported",
            fetched_at,
            "未找到可用的 Codex 命令行程序；本机历史统计仍可使用。",
        ),
        AppServerError::Timeout | AppServerError::Io => DashboardSnapshot::empty(
            "offline",
            fetched_at,
            "暂时无法连接 Codex App Server；本机历史统计仍可使用。",
        ),
        AppServerError::Protocol | AppServerError::Rpc { .. } => DashboardSnapshot::empty(
            "error",
            fetched_at,
            "读取 Codex 服务端用量失败；未记录原始错误内容。",
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failures_never_infer_a_plan_renewal() {
        let snapshot = connection_failure(AppServerError::NotFound, "now".to_owned());
        assert_eq!(snapshot.status, "unsupported");
        assert_eq!(snapshot.plan_renewal_at, None);
        assert!(snapshot.account_diagnostics.is_none());
    }

    #[test]
    #[ignore = "requires the locally installed and authenticated Codex app"]
    fn live_snapshot_uses_only_the_sanitized_dashboard_contract() {
        let snapshot = read_dashboard_snapshot();
        assert_eq!(snapshot.status, "ready");
        assert!(snapshot.fetched_at.is_some());
        assert!(snapshot.codex_version.is_some());
        assert!(!snapshot.quota_windows.is_empty());
        assert_eq!(snapshot.plan_renewal_at, None);

        let serialized = serde_json::to_value(snapshot).expect("snapshot serializes");
        let object = serialized.as_object().expect("snapshot is an object");
        assert!(object.contains_key("quotaWindows"));
        assert!(!object.contains_key("account"));
        assert!(!object.contains_key("email"));
    }
}
