//! Portable, user-owned model pricing files. Import never writes settings.
use std::{
    collections::HashSet,
    fs,
    io::{Read, Write},
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
};

use chrono::{DateTime, Local, Utc};
use serde::{Deserialize, Serialize};

use crate::{
    pricing::{ModelPricingRule, PeakPricingSchedule, PricingSettings},
    runtime::UsageRuntime,
    settings,
};

const MAX_FILE_BYTES: usize = 1024 * 1024;
const FORMAT: &str = "ai-token-pricing";
const UNIT: &str = "perMillionTokens";

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PricingTransferDocument {
    pub(crate) format: String,
    pub(crate) version: u32,
    pub(crate) unit: String,
    pub(crate) exported_at: String,
    pub(crate) saved_at: Option<String>,
    pub(crate) peak: PeakPricingSchedule,
    pub(crate) models: Vec<ModelPricingRule>,
}

impl PricingTransferDocument {
    fn from_saved(settings: PricingSettings) -> Self {
        Self {
            format: FORMAT.to_owned(),
            version: 2,
            unit: UNIT.to_owned(),
            exported_at: Utc::now().to_rfc3339(),
            saved_at: settings.updated_at,
            peak: settings.peak,
            models: settings.models,
        }
    }

    fn validate(&self) -> Result<(), String> {
        if self.format != FORMAT {
            return Err("这不是 AI Token 用量监控的定价文件。".into());
        }
        if !matches!(self.version, 1 | 2) {
            return Err(format!(
                "暂不支持定价文件版本 {}，请使用版本 1 或 2。",
                self.version
            ));
        }
        if self.unit != UNIT {
            return Err("定价单位必须为每百万 Token（perMillionTokens）。".into());
        }
        if DateTime::parse_from_rfc3339(&self.exported_at).is_err()
            || self
                .saved_at
                .as_deref()
                .is_some_and(|date| DateTime::parse_from_rfc3339(date).is_err())
        {
            return Err("定价文件中的导出或保存时间无效。".into());
        }
        if self.models.len() > 200 {
            return Err("定价文件最多可包含 200 个模型。".into());
        }
        let mut ids = HashSet::new();
        for model in &self.models {
            if !ids.insert(model.model_id.trim().to_lowercase()) {
                return Err(format!("模型 ID 重复（忽略大小写）：{}。", model.model_id));
            }
            for rate in [
                model.input_per_million,
                model.cached_input_per_million,
                model.cache_write_per_million,
                model.output_per_million,
            ]
            .into_iter()
            .flatten()
            {
                if !rate.is_finite() || !(0.0..=1_000_000.0).contains(&rate) {
                    return Err(format!(
                        "模型 {} 的单价必须为 0 到 1,000,000 的数字，或 null。",
                        model.model_id
                    ));
                }
            }
        }
        settings::validate_pricing_settings(&PricingSettings {
            updated_at: None,
            peak: self.peak.clone(),
            models: self.models.clone(),
        })
        .map_err(|_| {
            "定价配置无效，请检查模型名称、币种、高峰时段、时区、倍率和特殊日期。".to_owned()
        })
    }
}

fn parse_document(bytes: &[u8]) -> Result<PricingTransferDocument, String> {
    if bytes.len() > MAX_FILE_BYTES {
        return Err("定价文件不能超过 1 MiB。".into());
    }
    let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes);
    let value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|error| format!("无法解析定价 JSON：{error}"))?;
    // Option<f64> alone would treat a missing price as null. Require explicit
    // fields so a damaged file cannot silently erase a configured price.
    if let Some(models) = value.get("models").and_then(|models| models.as_array()) {
        for model in models {
            if value.get("version").and_then(|v| v.as_u64()) == Some(2) && model.get("longContext").is_none() {
                return Err("版本 2 的模型缺少 longContext；关闭长上下文加价请填写 null。".into());
            }
            for field in [
                "inputPerMillion",
                "cachedInputPerMillion",
                "cacheWritePerMillion",
                "outputPerMillion",
            ] {
                if model.get(field).is_none() {
                    return Err(format!("模型价格缺少字段 {field}；未定价请明确填写 null。"));
                }
            }
        }
    }
    let document: PricingTransferDocument =
        serde_json::from_value(value).map_err(|error| format!("定价文件结构无效：{error}"))?;
    document.validate()?;
    Ok(document)
}

fn read_document(path: &Path) -> Result<PricingTransferDocument, String> {
    let file = fs::File::open(path)
        .map_err(|_| "无法读取定价文件，请检查文件位置和读取权限。".to_owned())?;
    let mut bytes = Vec::new();
    file.take((MAX_FILE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| "读取定价文件失败。".to_owned())?;
    parse_document(&bytes)
}

#[tauri::command]
pub(crate) async fn import_pricing_file(
    window: tauri::Window,
) -> Result<Option<PricingTransferDocument>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let Some(path) = rfd::FileDialog::new()
            .set_parent(&window)
            .set_title("导入模型定价")
            .add_filter("模型定价 JSON", &["json"])
            .pick_file()
        else {
            return Ok(None);
        };
        read_document(&path).map(Some)
    })
    .await
    .map_err(|_| "后台定价导入任务意外结束。".to_owned())?
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PricingExportResult {
    status: &'static str,
    path: Option<String>,
}

#[tauri::command]
pub(crate) async fn export_pricing_file(
    window: tauri::Window,
    runtime: tauri::State<'_, UsageRuntime>,
) -> Result<PricingExportResult, String> {
    let runtime = runtime.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        // Capture saved settings before the user opens the save dialog.
        let saved = runtime
            .read_pricing_settings()
            .map_err(|_| "无法读取已保存的模型定价。".to_owned())?;
        let document = PricingTransferDocument::from_saved(saved);
        document.validate()?;
        let bytes =
            serde_json::to_vec_pretty(&document).map_err(|_| "无法生成定价文件。".to_owned())?;
        let name = format!("AI-Token-模型定价-{}.json", Local::now().format("%Y-%m-%d"));
        let Some(path) = rfd::FileDialog::new()
            .set_parent(&window)
            .set_title("导出已保存的模型定价")
            .add_filter("模型定价 JSON", &["json"])
            .set_file_name(name)
            .save_file()
        else {
            return Ok(PricingExportResult {
                status: "cancelled",
                path: None,
            });
        };
        if !path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
        {
            return Err("请使用 .json 文件扩展名。".into());
        }
        atomic_write(&path, &bytes).map_err(|_| {
            "无法保存定价文件，请检查目标文件是否被占用及保存位置的写入权限。".to_owned()
        })?;
        Ok(PricingExportResult {
            status: "saved",
            path: Some(path.to_string_lossy().into_owned()),
        })
    })
    .await
    .map_err(|_| "后台定价导出任务意外结束。".to_owned())?
}

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
fn atomic_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "missing directory")
    })?;
    let temp = parent.join(format!(
        ".ai-token-pricing-{}-{}-{}.tmp",
        std::process::id(),
        Utc::now().timestamp_millis(),
        NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
    ));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)?;
    let result = (|| {
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn document() -> PricingTransferDocument {
        PricingTransferDocument::from_saved(PricingSettings {
            updated_at: Some("2026-11-07T10:00:00Z".into()),
            peak: PeakPricingSchedule::default(),
            models: vec![ModelPricingRule {
                model_id: "test-model".into(),
                display_name: "测试模型".into(),
                currency: "CNY".into(),
                input_per_million: Some(0.0),
                cached_input_per_million: None,
                cache_write_per_million: Some(0.125),
                output_per_million: Some(12.5),
                peak_enabled: true,
                long_context: Some(crate::pricing::LongContextRule {
                    threshold: 272_000, input_multiplier: 2.0, output_multiplier: 1.5,
                }),
            }],
        })
    }
    #[test]
    fn roundtrip_preserves_all_prices_and_peak_settings() {
        let mut original = document();
        original.peak.time_zone = "Asia/Tokyo".into();
        original.peak.special_dates = vec!["2026-11-07".into()];
        let parsed = parse_document(&serde_json::to_vec(&original).unwrap()).unwrap();
        assert_eq!(original.models, parsed.models);
        assert_eq!(original.peak, parsed.peak);
        assert_eq!(original.saved_at, parsed.saved_at);
        assert_eq!(parsed.version, 2);
        let mut legacy = serde_json::to_value(&original).unwrap();
        legacy["models"][0].as_object_mut().unwrap().remove("longContext");
        assert!(parse_document(&serde_json::to_vec(&legacy).unwrap()).is_err());
        legacy["version"] = json!(1);
        let parsed = parse_document(&serde_json::to_vec(&legacy).unwrap()).unwrap();
        assert_eq!(parsed.version, 1);
        assert!(parsed.models[0].long_context.is_none());
    }
    #[test]
    fn rejects_bad_versions_prices_duplicates_and_missing_fields() {
        for (field, value) in [
            ("version", json!(3)),
            ("unit", json!("perToken")),
            ("format", json!("other")),
            ("exportedAt", json!("yesterday")),
        ] {
            let mut bad = serde_json::to_value(document()).unwrap();
            bad[field] = value;
            assert!(
                parse_document(&serde_json::to_vec(&bad).unwrap()).is_err(),
                "{field}"
            );
        }
        let mut bad = document();
        bad.models[0].input_per_million = Some(-1.0);
        assert!(bad.validate().is_err());
        let mut bad = document();
        let mut duplicate = bad.models[0].clone();
        duplicate.model_id = "TEST-MODEL".into();
        bad.models.push(duplicate);
        assert!(bad.validate().is_err());
        let mut bad = serde_json::to_value(document()).unwrap();
        bad["models"][0]
            .as_object_mut()
            .unwrap()
            .remove("outputPerMillion");
        assert!(parse_document(&serde_json::to_vec(&bad).unwrap()).is_err());
        assert!(parse_document(b"{broken").is_err());
        assert!(parse_document(&vec![b' '; MAX_FILE_BYTES + 1]).is_err());
    }
    #[test]
    fn rejects_bad_schedule_and_model_limit() {
        let mut bad = document();
        bad.peak.time_zone = "bad/zone".into();
        assert!(bad.validate().is_err());
        let mut bad = document();
        bad.models = (0..201)
            .map(|index| {
                let mut model = bad.models[0].clone();
                model.model_id = format!("model-{index}");
                model
            })
            .collect();
        assert!(bad.validate().is_err());
    }
    #[test]
    fn bounded_reads_and_atomic_replacement_leave_no_temp_files() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("prices.json");
        fs::write(&path, b"old").unwrap();
        let bytes = serde_json::to_vec(&document()).unwrap();
        atomic_write(&path, &bytes).unwrap();
        assert_eq!(read_document(&path).unwrap().models, document().models);
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
        let invalid = directory.path().join("missing").join("prices.json");
        assert!(atomic_write(&invalid, b"new").is_err());
        assert_eq!(fs::read(&path).unwrap(), bytes);
        fs::write(&path, vec![b' '; MAX_FILE_BYTES + 1]).unwrap();
        assert!(read_document(&path).is_err());
    }
}
