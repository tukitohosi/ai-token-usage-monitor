//! User-owned API pricing.
//!
//! No model price is compiled into the application. Unknown and not-yet-
//! configured models remain explicitly unpriced until the user saves a rule in
//! the local settings database.

use chrono::{Local, NaiveTime, TimeZone};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PriceCatalogTier {
    pub(crate) label: String,
    pub(crate) condition: String,
    pub(crate) input_per_million: Option<f64>,
    pub(crate) cached_input_per_million: Option<f64>,
    pub(crate) cache_write_per_million: Option<f64>,
    pub(crate) output_per_million: Option<f64>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PriceCatalogEntry {
    pub(crate) display_name: String,
    pub(crate) model_id: String,
    pub(crate) aliases: Vec<String>,
    pub(crate) currency: String,
    pub(crate) source_label: String,
    pub(crate) tiers: Vec<PriceCatalogTier>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PeakPricingSchedule {
    pub(crate) start_time: String,
    pub(crate) end_time: String,
    pub(crate) multiplier: f64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ModelPricingRule {
    pub(crate) model_id: String,
    pub(crate) display_name: String,
    pub(crate) currency: String,
    pub(crate) input_per_million: Option<f64>,
    pub(crate) cached_input_per_million: Option<f64>,
    pub(crate) cache_write_per_million: Option<f64>,
    pub(crate) output_per_million: Option<f64>,
    pub(crate) peak_enabled: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PricingSettings {
    pub(crate) updated_at: Option<String>,
    pub(crate) peak: PeakPricingSchedule,
    pub(crate) models: Vec<ModelPricingRule>,
}

impl Default for PricingSettings {
    fn default() -> Self {
        Self {
            updated_at: None,
            peak: PeakPricingSchedule {
                start_time: "18:00".to_owned(),
                end_time: "23:00".to_owned(),
                multiplier: 1.5,
            },
            models: Vec::new(),
        }
    }
}

pub(crate) fn catalog(settings: &PricingSettings) -> Vec<PriceCatalogEntry> {
    settings
        .models
        .iter()
        .map(|rule| PriceCatalogEntry {
            display_name: rule.display_name.clone(),
            model_id: rule.model_id.clone(),
            aliases: Vec::new(),
            currency: rule.currency.clone(),
            source_label: "本机手动设置".to_owned(),
            tiers: vec![PriceCatalogTier {
                label: "基础价格".to_owned(),
                condition: if rule.peak_enabled {
                    format!(
                        "{}-{} 按 {} 倍计费",
                        settings.peak.start_time, settings.peak.end_time, settings.peak.multiplier
                    )
                } else {
                    "未启用高峰期倍率".to_owned()
                },
                input_per_million: rule.input_per_million,
                cached_input_per_million: rule.cached_input_per_million,
                cache_write_per_million: rule.cache_write_per_million,
                output_per_million: rule.output_per_million,
            }],
        })
        .collect()
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct PriceUsage {
    pub(crate) input_tokens: i64,
    pub(crate) cached_input_tokens: i64,
    pub(crate) cache_write_5m_tokens: i64,
    pub(crate) cache_write_1h_tokens: i64,
    pub(crate) cache_write_unknown_tokens: i64,
    pub(crate) output_tokens: i64,
    pub(crate) total_tokens: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CostSummary {
    pub(crate) usd: f64,
    pub(crate) cny: f64,
    pub(crate) unpriced_tokens: i64,
}

impl CostSummary {
    pub(crate) fn add_assign(&mut self, other: &Self) {
        self.usd += other.usd;
        self.cny += other.cny;
        self.unpriced_tokens += other.unpriced_tokens;
    }
}

/// Estimates one indexed event using only the locally persisted manual rule.
pub(crate) fn estimate(
    model_label: &str,
    usage: PriceUsage,
    occurred_at_ms: Option<i64>,
    settings: &PricingSettings,
) -> CostSummary {
    let model = model_label.trim();
    let Some(rule) = settings.models.iter().find(|rule| {
        rule.model_id.eq_ignore_ascii_case(model) || rule.display_name.eq_ignore_ascii_case(model)
    }) else {
        return CostSummary {
            unpriced_tokens: usage.total_tokens.max(0),
            ..CostSummary::default()
        };
    };

    let mut amount = 0.0;
    let mut unpriced = 0_i64;
    for (tokens, rate) in [
        (usage.input_tokens.max(0), rule.input_per_million),
        (
            usage.cached_input_tokens.max(0),
            rule.cached_input_per_million,
        ),
        (
            usage.cache_write_5m_tokens.max(0)
                + usage.cache_write_1h_tokens.max(0)
                + usage.cache_write_unknown_tokens.max(0),
            rule.cache_write_per_million,
        ),
        (usage.output_tokens.max(0), rule.output_per_million),
    ] {
        if tokens == 0 {
            continue;
        }
        if let Some(rate) = rate {
            amount += tokens as f64 * rate / 1_000_000.0;
        } else {
            unpriced += tokens;
        }
    }

    if rule.peak_enabled && is_peak_time(occurred_at_ms, &settings.peak) {
        amount *= settings.peak.multiplier;
    }
    if rule.currency == "CNY" {
        CostSummary {
            usd: 0.0,
            cny: amount,
            unpriced_tokens: unpriced,
        }
    } else {
        CostSummary {
            usd: amount,
            cny: 0.0,
            unpriced_tokens: unpriced,
        }
    }
}

fn is_peak_time(occurred_at_ms: Option<i64>, schedule: &PeakPricingSchedule) -> bool {
    let Some(time) = occurred_at_ms
        .and_then(|value| Local.timestamp_millis_opt(value).single())
        .map(|value| value.time())
    else {
        return false;
    };
    let Ok(start) = NaiveTime::parse_from_str(&schedule.start_time, "%H:%M") else {
        return false;
    };
    let Ok(end) = NaiveTime::parse_from_str(&schedule.end_time, "%H:%M") else {
        return false;
    };
    if start == end {
        return true;
    }
    if start < end {
        time >= start && time < end
    } else {
        time >= start || time < end
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn small_usage() -> PriceUsage {
        PriceUsage {
            input_tokens: 1_000,
            cached_input_tokens: 1_000,
            output_tokens: 1_000,
            total_tokens: 3_000,
            ..PriceUsage::default()
        }
    }

    fn settings() -> PricingSettings {
        PricingSettings {
            updated_at: None,
            peak: PeakPricingSchedule {
                start_time: "00:00".to_owned(),
                end_time: "23:59".to_owned(),
                multiplier: 2.0,
            },
            models: vec![ModelPricingRule {
                model_id: "manual-model".to_owned(),
                display_name: "Manual model".to_owned(),
                currency: "USD".to_owned(),
                input_per_million: Some(4.0),
                cached_input_per_million: Some(0.4),
                cache_write_per_million: None,
                output_per_million: Some(20.0),
                peak_enabled: false,
            }],
        }
    }

    #[test]
    fn uses_only_manual_rules_and_keeps_missing_rates_unpriced() {
        let mut sample = small_usage();
        sample.cache_write_unknown_tokens = 100;
        sample.total_tokens += 100;
        let cost = estimate("manual-model", sample, Some(1), &settings());
        assert!((cost.usd - 0.0244).abs() < 0.000_001);
        assert_eq!(cost.unpriced_tokens, 100);
    }

    #[test]
    fn unknown_models_never_fall_back_to_a_compiled_price() {
        let sample = small_usage();
        assert_eq!(
            estimate("gpt-5.6-sol", sample, Some(1), &settings()).unpriced_tokens,
            3_000
        );
    }

    #[test]
    fn peak_multiplier_is_opt_in_per_model() {
        let mut configured = settings();
        configured.models[0].peak_enabled = true;
        let standard = estimate("manual-model", small_usage(), None, &configured);
        let peak = estimate(
            "manual-model",
            small_usage(),
            Some(Local::now().timestamp_millis()),
            &configured,
        );
        assert!(peak.usd >= standard.usd);
    }

    #[test]
    fn catalog_is_derived_from_manual_settings() {
        let entries = catalog(&settings());
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].source_label, "本机手动设置");
    }
}
