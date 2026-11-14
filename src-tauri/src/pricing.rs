//! User-owned API pricing.
//!
//! No model price is compiled into the application. Unknown and not-yet-
//! configured models remain explicitly unpriced until the user saves a rule in
//! the local settings database.

use chrono::{Datelike, Local, NaiveDate, NaiveTime, TimeZone, Utc};
use chrono_tz::Tz;
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
    pub(crate) long_context: Option<LongContextRule>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PeakPricingSchedule {
    pub(crate) windows: Vec<PeakPricingWindow>,
    pub(crate) multiplier: f64,
    pub(crate) time_zone: String,
    pub(crate) exclude_china_holidays: bool,
    pub(crate) special_dates: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PeakPricingWindow {
    pub(crate) weekday_start: u8,
    pub(crate) weekday_end: u8,
    pub(crate) start_time: String,
    pub(crate) end_time: String,
}

impl Default for PeakPricingSchedule {
    fn default() -> Self {
        Self {
            windows: vec![PeakPricingWindow {
                weekday_start: 1,
                weekday_end: 7,
                start_time: "18:00".to_owned(),
                end_time: "23:00".to_owned(),
            }],
            multiplier: 1.5,
            time_zone: "local".to_owned(),
            exclude_china_holidays: false,
            special_dates: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LongContextRule {
    pub(crate) threshold: i64,
    pub(crate) input_multiplier: f64,
    pub(crate) output_multiplier: f64,
}

impl LongContextRule {
    pub(crate) fn is_valid(&self) -> bool {
        (1..=9_007_199_254_740_991).contains(&self.threshold)
            && self.input_multiplier.is_finite() && self.input_multiplier > 0.0
            && self.output_multiplier.is_finite() && self.output_multiplier > 0.0
    }
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
    pub(crate) long_context: Option<LongContextRule>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PricingSettings {
    pub(crate) updated_at: Option<String>,
    pub(crate) peak: PeakPricingSchedule,
    pub(crate) models: Vec<ModelPricingRule>,
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
            long_context: rule.long_context.clone(),
            tiers: vec![PriceCatalogTier {
                label: "基础价格".to_owned(),
                condition: if rule.peak_enabled {
                    if settings.peak.windows.is_empty() {
                        "未设置高峰时段".to_owned()
                    } else {
                        format!(
                            "每周 {} 个高峰时段，匹配时按 {} 倍计费{}",
                            settings.peak.windows.len(),
                            settings.peak.multiplier,
                            if settings.peak.exclude_china_holidays {
                                "；排除中国节假日"
                            } else {
                                ""
                            }
                        )
                    }
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
    /// Complete, inclusive input for one verified request, before any aggregation.
    pub(crate) request_input_tokens: Option<i64>,
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
    pub(crate) long_context_unconfirmed_events: i64,
}

impl CostSummary {
    pub(crate) fn add_assign(&mut self, other: &Self) {
        self.usd += other.usd;
        self.cny += other.cny;
        self.unpriced_tokens += other.unpriced_tokens;
        self.long_context_unconfirmed_events += other.long_context_unconfirmed_events;
    }
}

/// Estimates one indexed event using only the locally persisted manual rule.
pub(crate) fn estimate(
    model_label: &str,
    usage: PriceUsage,
    occurred_at_ms: Option<i64>,
    settings: &PricingSettings,
) -> CostSummary {
    let model = model_label.trim().to_lowercase();
    let Some(rule) = settings.models.iter().find(|rule| rule.model_id.to_lowercase() == model)
        .or_else(|| settings.models.iter().find(|rule| rule.display_name.to_lowercase() == model)) else {
        return CostSummary {
            unpriced_tokens: usage.total_tokens.max(0),
            ..CostSummary::default()
        };
    };

    let mut input_multiplier = 1.0;
    let mut output_multiplier = 1.0;
    let mut unconfirmed = 0;
    if let Some(long) = &rule.long_context {
        match usage.request_input_tokens {
            Some(input) if input > long.threshold => {
                input_multiplier = long.input_multiplier;
                output_multiplier = long.output_multiplier;
            }
            None if usage.total_tokens > 0 => unconfirmed = 1,
            _ => {}
        }
    }
    let mut amount = 0.0;
    // Some sources preserve a reported total even when only part of it has a breakdown.
    let mut unclassified = usage.total_tokens.max(0);
    let mut unpriced = 0_i64;
    for (tokens, rate, multiplier) in [
        (usage.input_tokens.max(0), rule.input_per_million, input_multiplier),
        (
            usage.cached_input_tokens.max(0),
            rule.cached_input_per_million,
            input_multiplier,
        ),
        (
            usage.cache_write_5m_tokens.max(0)
                + usage.cache_write_1h_tokens.max(0)
                + usage.cache_write_unknown_tokens.max(0),
            rule.cache_write_per_million,
            input_multiplier,
        ),
        (usage.output_tokens.max(0), rule.output_per_million, output_multiplier),
    ] {
        unclassified = unclassified.saturating_sub(tokens);
        if tokens == 0 {
            continue;
        }
        if let Some(rate) = rate {
            amount += tokens as f64 * rate / 1_000_000.0 * multiplier;
        } else {
            unpriced += tokens;
        }
    }
    unpriced += unclassified.max(0);

    if rule.peak_enabled && is_peak_time(occurred_at_ms, &settings.peak) {
        amount *= settings.peak.multiplier;
    }
    let (usd, cny) = if rule.currency == "CNY" { (0.0, amount) } else { (amount, 0.0) };
    CostSummary { usd, cny, unpriced_tokens: unpriced, long_context_unconfirmed_events: unconfirmed }
}

fn is_peak_time(occurred_at_ms: Option<i64>, schedule: &PeakPricingSchedule) -> bool {
    let Some(timestamp) = occurred_at_ms.and_then(|value| Utc.timestamp_millis_opt(value).single())
    else {
        return false;
    };
    let (date, time, weekday) = if schedule.time_zone == "local" {
        let value = timestamp.with_timezone(&Local);
        (
            value.date_naive(),
            value.time(),
            value.weekday().number_from_monday() as u8,
        )
    } else {
        let Ok(zone) = schedule.time_zone.parse::<Tz>() else {
            return false;
        };
        let value = timestamp.with_timezone(&zone);
        (
            value.date_naive(),
            value.time(),
            value.weekday().number_from_monday() as u8,
        )
    };
    let day = date.format("%Y-%m-%d").to_string();
    if schedule.special_dates.iter().any(|special| special == &day)
        || (schedule.exclude_china_holidays && is_china_holiday(date))
    {
        return false;
    }
    schedule.windows.iter().any(|window| {
        let selected_day = if window.weekday_start <= window.weekday_end {
            (window.weekday_start..=window.weekday_end).contains(&weekday)
        } else {
            weekday >= window.weekday_start || weekday <= window.weekday_end
        };
        if !selected_day {
            return false;
        }
        let (Ok(start), Ok(end)) = (
            NaiveTime::parse_from_str(&window.start_time, "%H:%M"),
            NaiveTime::parse_from_str(&window.end_time, "%H:%M"),
        ) else {
            return false;
        };
        if start == end {
            true
        } else if start < end {
            time >= start && time < end
        } else {
            time >= start || time < end
        }
    })
}

// Published nationwide holiday breaks. Weekend make-up workdays remain weekends
// for weekday-based pricing. Years outside this table use weekly rules only.
// 2024: https://www.gov.cn/zhengce/content/202310/content_6911527.htm
// 2025: https://www.gov.cn/zhengce/zhengceku/202411/content_6986383.htm
// 2026: https://www.gov.cn/zhengce/zhengceku/202511/content_7047091.htm
const CHINA_HOLIDAY_BREAKS: &[(&str, &str)] = &[
    ("2024-01-01", "2024-01-01"),
    ("2024-02-10", "2024-02-17"),
    ("2024-04-04", "2024-04-06"),
    ("2024-05-01", "2024-05-05"),
    ("2024-06-08", "2024-06-10"),
    ("2024-09-15", "2024-09-17"),
    ("2024-10-01", "2024-10-07"),
    ("2025-01-01", "2025-01-01"),
    ("2025-01-28", "2025-02-04"),
    ("2025-04-04", "2025-04-06"),
    ("2025-05-01", "2025-05-05"),
    ("2025-05-31", "2025-06-02"),
    ("2025-10-01", "2025-10-08"),
    ("2026-01-01", "2026-01-03"),
    ("2026-02-15", "2026-02-23"),
    ("2026-04-04", "2026-04-06"),
    ("2026-05-01", "2026-05-05"),
    ("2026-06-19", "2026-06-21"),
    ("2026-09-25", "2026-09-27"),
    ("2026-10-01", "2026-10-07"),
];

fn is_china_holiday(date: NaiveDate) -> bool {
    let day = date.format("%Y-%m-%d").to_string();
    CHINA_HOLIDAY_BREAKS
        .iter()
        .any(|(start, end)| *start <= day.as_str() && day.as_str() <= *end)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn timestamp_ms(value: &str) -> i64 {
        chrono::DateTime::parse_from_rfc3339(value)
            .expect("valid timestamp")
            .timestamp_millis()
    }

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
                windows: vec![PeakPricingWindow {
                    weekday_start: 1,
                    weekday_end: 7,
                    start_time: "00:00".to_owned(),
                    end_time: "23:59".to_owned(),
                }],
                multiplier: 2.0,
                ..PeakPricingSchedule::default()
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
                long_context: None,
            }],
        }
    }

    #[test]
    fn model_id_wins_over_another_rules_display_name_regardless_of_order() {
        let mut configured = settings();
        let mut other = configured.models[0].clone();
        other.model_id = "other-model".into();
        other.display_name = "manual-model".into();
        other.input_per_million = Some(999.0);
        configured.models.insert(0, other);
        let cost = estimate("MANUAL-MODEL", small_usage(), None, &configured);
        assert!((cost.usd - 0.0244).abs() < 0.000_001);
        // Old display-name aliases remain usable after all IDs are checked.
        assert!(estimate("Manual model", small_usage(), None, &configured).usd > 0.0);
    }

    #[test]
    fn uses_only_manual_rules_and_keeps_missing_rates_unpriced() {
        let mut sample = small_usage();
        sample.cache_write_unknown_tokens = 100;
        sample.total_tokens += 160; // Also includes 60 tokens with no reported classification.
        let cost = estimate("manual-model", sample, Some(1), &settings());
        assert!((cost.usd - 0.0244).abs() < 0.000_001);
        assert_eq!(cost.unpriced_tokens, 160);
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
    fn weekly_windows_holidays_and_special_dates_follow_beijing_calendar() {
        let schedule = PeakPricingSchedule {
            windows: vec![
                PeakPricingWindow {
                    weekday_start: 1,
                    weekday_end: 5,
                    start_time: "09:00".to_owned(),
                    end_time: "12:00".to_owned(),
                },
                PeakPricingWindow {
                    weekday_start: 1,
                    weekday_end: 5,
                    start_time: "14:00".to_owned(),
                    end_time: "18:00".to_owned(),
                },
            ],
            time_zone: "Asia/Shanghai".to_owned(),
            exclude_china_holidays: true,
            special_dates: vec!["2026-09-28".to_owned()],
            ..PeakPricingSchedule::default()
        };
        for (time, expected) in [
            ("2026-09-29T08:59:00+08:00", false),
            ("2026-09-29T09:00:00+08:00", true),
            ("2026-09-29T12:00:00+08:00", false),
            ("2026-09-29T14:00:00+08:00", true),
            ("2026-09-29T18:00:00+08:00", false),
            ("2026-09-28T10:00:00+08:00", false), // user exception
            ("2026-10-01T10:00:00+08:00", false), // announced holiday
            ("2026-10-03T10:00:00+08:00", false), // weekend
            ("2026-10-10T10:00:00+08:00", false), // make-up work Saturday
        ] {
            assert_eq!(
                is_peak_time(Some(timestamp_ms(time)), &schedule),
                expected,
                "{time}"
            );
        }
        assert!(!is_peak_time(None, &schedule));
    }

    #[test]
    fn city_zones_follow_their_local_calendar_and_daylight_saving_time() {
        let mut schedule = PeakPricingSchedule {
            windows: vec![PeakPricingWindow {
                weekday_start: 1,
                weekday_end: 5,
                start_time: "09:00".to_owned(),
                end_time: "10:00".to_owned(),
            }],
            time_zone: "America/Los_Angeles".to_owned(),
            ..PeakPricingSchedule::default()
        };
        for (time, expected) in [
            ("2026-01-12T17:30:00Z", true), // 09:30 PST
            ("2026-01-12T16:30:00Z", false),
            ("2026-07-13T16:30:00Z", true), // 09:30 PDT
            ("2026-07-13T17:30:00Z", false),
        ] {
            assert_eq!(
                is_peak_time(Some(timestamp_ms(time)), &schedule),
                expected,
                "{time}"
            );
        }

        schedule.time_zone = "Europe/London".to_owned();
        for (time, expected) in [
            ("2026-01-12T09:30:00Z", true), // 09:30 GMT
            ("2026-07-13T08:30:00Z", true), // 09:30 BST
            ("2026-07-13T09:30:00Z", false),
        ] {
            assert_eq!(
                is_peak_time(Some(timestamp_ms(time)), &schedule),
                expected,
                "{time}"
            );
        }

        schedule.time_zone = "Asia/Tokyo".to_owned();
        assert!(is_peak_time(
            Some(timestamp_ms("2026-07-13T00:30:00Z")),
            &schedule
        ));
        schedule.special_dates.push("2026-07-13".to_owned());
        assert!(!is_peak_time(
            Some(timestamp_ms("2026-07-13T00:30:00Z")),
            &schedule
        ));
        schedule.time_zone = "unknown/city".to_owned();
        assert!(!is_peak_time(
            Some(timestamp_ms("2026-07-13T00:30:00Z")),
            &schedule
        ));
    }

    #[test]
    fn overnight_windows_use_the_event_day_and_overlaps_do_not_stack() {
        let mut configured = settings();
        configured.models[0].peak_enabled = true;
        configured.peak = PeakPricingSchedule {
            windows: vec![
                PeakPricingWindow {
                    weekday_start: 1,
                    weekday_end: 5,
                    start_time: "23:00".to_owned(),
                    end_time: "02:00".to_owned(),
                },
                PeakPricingWindow {
                    weekday_start: 1,
                    weekday_end: 5,
                    start_time: "23:30".to_owned(),
                    end_time: "01:00".to_owned(),
                },
            ],
            time_zone: "Asia/Shanghai".to_owned(),
            ..PeakPricingSchedule::default()
        };
        let friday = timestamp_ms("2026-09-18T23:45:00+08:00");
        assert!(is_peak_time(Some(friday), &configured.peak));
        assert!(!is_peak_time(
            Some(timestamp_ms("2026-09-19T01:00:00+08:00")),
            &configured.peak
        ));
        let normal = estimate("manual-model", small_usage(), None, &configured);
        let peak = estimate("manual-model", small_usage(), Some(friday), &configured);
        assert!((peak.usd - normal.usd * configured.peak.multiplier).abs() < 0.000_001);
    }

    #[test]
    fn catalog_is_derived_from_manual_settings() {
        let entries = catalog(&settings());
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].source_label, "本机手动设置");
    }

    #[test]
    fn long_context_uses_the_whole_request_and_combines_with_peak() {
        let mut configured = settings();
        let model = &mut configured.models[0];
        model.long_context = Some(LongContextRule { threshold: 272_000, input_multiplier: 2.0, output_multiplier: 1.5 });
        model.input_per_million = Some(10.0);
        model.cached_input_per_million = Some(1.0);
        model.cache_write_per_million = Some(12.5);
        model.output_per_million = Some(50.0);
        model.peak_enabled = true;
        let mut event = PriceUsage {
            input_tokens: 1_000, cached_input_tokens: 270_000, cache_write_5m_tokens: 700,
            cache_write_1h_tokens: 200, cache_write_unknown_tokens: 100, output_tokens: 100,
            total_tokens: 272_100, request_input_tokens: Some(272_000),
        };
        let short = estimate("manual-model", event, None, &configured);
        assert!((short.usd - 0.2975).abs() < 1e-10);
        let mut two_short_requests = short.clone();
        two_short_requests.add_assign(&short);
        assert!((two_short_requests.usd - 0.595).abs() < 1e-10);
        event.cached_input_tokens += 1;
        event.total_tokens += 1;
        event.request_input_tokens = Some(272_001);
        let long = estimate("manual-model", event, None, &configured);
        assert!((long.usd - 0.592502).abs() < 1e-10);
        configured.peak.time_zone = "UTC".into();
        let peak = estimate("manual-model", event, Some(timestamp_ms("2026-11-13T12:00:00Z")), &configured);
        assert!((peak.usd - 2.0 * long.usd).abs() < 1e-10);
        event.request_input_tokens = None;
        let uncertain = estimate("manual-model", event, None, &configured);
        assert!((uncertain.usd - 0.297501).abs() < 1e-10);
        assert_eq!(uncertain.long_context_unconfirmed_events, 1);
        configured.models[0].long_context = None;
        let disabled = estimate("manual-model", event, None, &configured);
        assert_eq!(disabled.usd, uncertain.usd);
        assert_eq!(disabled.long_context_unconfirmed_events, 0);
    }
}
