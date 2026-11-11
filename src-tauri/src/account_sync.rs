//! Account cache is independent of local indexing. Identity hashes never cross IPC.
use crate::{
    app_server::{AccountUsageReadResult, NormalizedQuotaWindow, RateLimitResetCredits},
    snapshot::DashboardSnapshot,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ResourceSync {
    pub(crate) last_successful_at: Option<String>,
    pub(crate) stale: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AccountSync {
    pub(crate) status: String,
    pub(crate) quota: ResourceSync,
    pub(crate) usage: ResourceSync,
}

impl Default for AccountSync {
    fn default() -> Self {
        Self {
            status: "idle".into(),
            quota: ResourceSync {
                stale: true,
                ..Default::default()
            },
            usage: ResourceSync {
                stale: true,
                ..Default::default()
            },
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(crate) struct AccountCache {
    pub(crate) credential_fingerprint: Option<String>,
    identity: Option<String>,
    pub(crate) sync: AccountSync,
    quota_windows: Vec<NormalizedQuotaWindow>,
    reset_credits: Option<RateLimitResetCredits>,
    account_usage: Option<AccountUsageReadResult>,
    codex_version: Option<String>,
    diagnostics: Option<crate::snapshot::AccountReadDiagnostics>,
}

impl AccountCache {
    pub(crate) fn invalidate_credentials(&mut self, fingerprint: Option<String>) {
        // Missing credentials cannot establish ownership of a persisted cache.
        if fingerprint.is_none() || self.credential_fingerprint != fingerprint {
            *self = Self {
                credential_fingerprint: fingerprint,
                ..Self::default()
            };
        }
    }

    pub(crate) fn mark_stale(&mut self, status: &str) {
        self.sync.status = status.into();
        self.sync.quota.stale = true;
        self.sync.usage.stale = true;
    }

    pub(crate) fn merge(&mut self, result: &DashboardSnapshot) {
        if result.status == "unauthenticated"
            || (result.account_identity.is_some() && self.identity != result.account_identity)
        {
            let fingerprint = self.credential_fingerprint.clone();
            *self = Self {
                credential_fingerprint: fingerprint,
                ..Self::default()
            };
        }
        if result.account_identity.is_some() {
            self.identity = result.account_identity.clone();
        }
        self.codex_version = result.codex_version.clone();
        self.diagnostics = result.account_diagnostics.clone();
        self.mark_stale(result.status);
        if result.quota_read_ok {
            self.quota_windows = result.quota_windows.clone();
            self.reset_credits = result.reset_credits.clone();
            self.sync.quota = ResourceSync {
                last_successful_at: result.fetched_at.clone(),
                stale: false,
            };
        }
        if result.usage_read_ok {
            self.account_usage = result.account_usage.clone();
            self.sync.usage = ResourceSync {
                last_successful_at: result.fetched_at.clone(),
                stale: false,
            };
        }
        if result.quota_read_ok != result.usage_read_ok {
            self.sync.status = "partial".into();
        }
    }

    pub(crate) fn quota_refresh_due(&self, now_ms: u64) -> bool {
        let last = self
            .sync
            .quota
            .last_successful_at
            .as_deref()
            .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
            .map(|value| value.timestamp_millis().max(0) as u64);
        self.sync.quota.stale
            || last.map_or(true, |last| now_ms.saturating_sub(last) >= 60_000)
            || self
                .quota_windows
                .iter()
                .any(|quota| quota_expired(quota, now_ms))
    }

    pub(crate) fn apply(&self, snapshot: &mut DashboardSnapshot) {
        let mut sync = self.sync.clone();
        sync.quota.stale |= self
            .quota_windows
            .iter()
            .any(|quota| quota_expired(quota, chrono::Utc::now().timestamp_millis().max(0) as u64));
        snapshot.account_sync = Some(sync);
        snapshot.quota_windows = self.quota_windows.clone();
        snapshot.reset_credits = self.reset_credits.clone();
        snapshot.account_usage = self.account_usage.clone();
        snapshot.codex_version = self.codex_version.clone();
        snapshot.account_diagnostics = self.diagnostics.clone();
        if let Some(diagnostics) = snapshot.index_diagnostics.as_mut() {
            diagnostics.account_duration_ms =
                self.diagnostics.as_ref().map_or(0, |d| d.duration_ms);
        }
    }
}

fn quota_expired(quota: &NormalizedQuotaWindow, now_ms: u64) -> bool {
    let reset_ms = if quota.resets_at < 1_000_000_000_000. {
        quota.resets_at * 1_000.
    } else {
        quota.resets_at
    };
    reset_ms.is_finite() && reset_ms > 0. && reset_ms <= now_ms as f64
}

#[cfg(test)]
mod tests {
    use super::*;
    fn successful() -> DashboardSnapshot {
        let mut snapshot = DashboardSnapshot::empty("ready", "2026-09-27T00:00:00Z".into(), "");
        snapshot.account_identity = Some("account-a".into());
        snapshot.quota_read_ok = true;
        snapshot.usage_read_ok = true;
        snapshot.account_usage = Some(AccountUsageReadResult {
            summary: None,
            daily_usage_buckets: Some(vec![]),
            thread_usage: None,
        });
        snapshot
    }
    #[test]
    fn expired_quota_is_stale_until_a_new_server_window_arrives() {
        let now = chrono::Utc::now();
        let now_ms = now.timestamp_millis().max(0) as u64;
        let mut snapshot = successful();
        snapshot.fetched_at = Some(now.to_rfc3339());
        snapshot.quota_windows = vec![serde_json::from_value(serde_json::json!({
            "usedPercent": 95, "remainingPercent": 5, "windowDurationMins": 300,
            "resetsAt": now.timestamp() - 1, "key": "codex:primary", "limitId": "codex",
            "limitName": null, "normalModelSlug": null, "lane": "primary", "label": "5 小时",
            "credits": null, "planType": "plus", "reachedType": null
        }))
        .unwrap()];
        let mut cache = AccountCache::default();
        cache.merge(&snapshot);
        assert!(cache.quota_refresh_due(now_ms));
        let mut displayed = successful();
        cache.apply(&mut displayed);
        assert!(displayed.account_sync.as_ref().unwrap().quota.stale);
        assert_eq!(displayed.quota_windows[0].remaining_percent, 5.);
        snapshot.quota_windows[0].used_percent = 0.;
        snapshot.quota_windows[0].remaining_percent = 100.;
        snapshot.quota_windows[0].resets_at = (now.timestamp() + 10_800) as f64;
        cache.merge(&snapshot);
        cache.apply(&mut displayed);
        assert!(!cache.quota_refresh_due(now_ms));
        assert!(!displayed.account_sync.as_ref().unwrap().quota.stale);
        assert_eq!(displayed.quota_windows[0].remaining_percent, 100.);
        assert!(cache.quota_refresh_due(now_ms + 60_001));
    }

    #[test]
    fn account_completion_cannot_replace_newer_local_data() {
        let mut cache = AccountCache::default();
        cache.merge(&successful());
        let mut local = DashboardSnapshot::empty("ready", "new-local-time".into(), "local-message");
        local.revision = 42;
        local.last_successful_at = Some("new-local-time".into());
        cache.apply(&mut local);
        assert_eq!(local.revision, 42);
        assert_eq!(local.fetched_at.as_deref(), Some("new-local-time"));
        assert_eq!(local.last_successful_at.as_deref(), Some("new-local-time"));
        assert_eq!(local.message.as_deref(), Some("local-message"));
    }

    #[test]
    fn persisted_account_cache_is_stale_on_restart_and_identity_bound() {
        let mut cache = AccountCache::default();
        cache.invalidate_credentials(Some("credential-a".into()));
        cache.merge(&successful());
        let bytes = serde_json::to_vec(&cache).unwrap();
        let mut loaded: AccountCache = serde_json::from_slice(&bytes).unwrap();
        loaded.invalidate_credentials(Some("credential-a".into()));
        loaded.mark_stale("idle");
        assert!(loaded.account_usage.is_some());
        assert!(loaded.sync.usage.stale);
        loaded.invalidate_credentials(Some("credential-b".into()));
        assert!(loaded.account_usage.is_none());
    }

    #[test]
    fn partial_and_offline_reads_preserve_each_last_success() {
        let mut cache = AccountCache::default();
        cache.merge(&successful());
        let mut partial = successful();
        partial.usage_read_ok = false;
        partial.account_usage = None;
        partial.fetched_at = Some("later".into());
        cache.merge(&partial);
        assert!(cache.account_usage.is_some());
        assert!(cache.sync.usage.stale);
        assert!(!cache.sync.quota.stale);
        assert_eq!(
            cache.sync.quota.last_successful_at.as_deref(),
            Some("later")
        );
        assert_ne!(
            cache.sync.usage.last_successful_at.as_deref(),
            Some("later")
        );
        cache.merge(&DashboardSnapshot::empty("offline", "later".into(), ""));
        assert!(cache.account_usage.is_some());
        assert!(cache.sync.quota.stale && cache.sync.usage.stale);
    }
    #[test]
    fn logout_identity_change_and_missing_credentials_clear_old_data() {
        let mut cache = AccountCache::default();
        cache.merge(&successful());
        cache.merge(&DashboardSnapshot::empty(
            "unauthenticated",
            "later".into(),
            "",
        ));
        assert!(cache.account_usage.is_none());
        cache.merge(&successful());
        let mut other = DashboardSnapshot::empty("error", "later".into(), "");
        other.account_identity = Some("account-b".into());
        cache.merge(&other);
        assert!(cache.account_usage.is_none());
        cache.merge(&successful());
        cache.invalidate_credentials(None);
        assert!(cache.account_usage.is_none());
    }
}
