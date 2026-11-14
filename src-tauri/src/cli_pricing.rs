use std::{
    collections::{BTreeMap, HashSet},
    env, fs,
    io::{ErrorKind, Read},
    path::{Component, Path, PathBuf},
};

use serde::Serialize;
use serde_json::{json, Value};
use tauri::Manager;

use crate::pricing::LongContextRule;

#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(serde::Deserialize, PartialEq))]
#[serde(rename_all = "camelCase")]
pub(crate) struct CliLongContextRule {
    model_id: String,
    long_context: LongContextRule,
}

const NO_INSTALL: &str = "未找到已启用的 cc-statusline 状态栏，可以手动填写长上下文规则。";
const INVALID_CONFIG: &str = "状态栏配置格式无效，请检查该配置，或手动填写长上下文规则。";
const MAX_CONFIG_BYTES: usize = 1024 * 1024;

fn read_config(path: &Path) -> Result<Option<Value>, String> {
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("无法读取状态栏配置文件。".into()),
    };
    let mut bytes = Vec::new();
    file.take((MAX_CONFIG_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| "无法读取状态栏配置文件。")?;
    if bytes.len() > MAX_CONFIG_BYTES {
        return Err("状态栏配置文件超过 1 MiB。".into());
    }
    let value: Value = serde_json::from_slice(&bytes).map_err(|_| INVALID_CONFIG)?;
    if !value.is_object() {
        return Err(INVALID_CONFIG.into());
    }
    Ok(Some(value))
}

// The installer writes 'node "<absolute path>/bin/cc-statusline.js" render'.
// Extract the literal path only. Shell wrappers are never run or expanded.
fn script_path(command: &str) -> Option<PathBuf> {
    fn argument(cmd: &str) -> Option<(&str, &str)> {
        let cmd = cmd.trim_start();
        if let Some(rest) = cmd.strip_prefix('"').or_else(|| cmd.strip_prefix('\'')) {
            let end = rest.find(cmd.chars().next()?)?;
            Some((&rest[..end], &rest[end + 1..]))
        } else {
            let end = cmd.find(char::is_whitespace).unwrap_or(cmd.len());
            Some((&cmd[..end], &cmd[end..]))
        }
    }
    let (node, args) = argument(command)?;
    let executable = Path::new(node).file_name()?.to_str()?;
    if !["node", "node.exe"]
        .iter()
        .any(|n| executable.eq_ignore_ascii_case(n))
    {
        return None;
    }
    let (script, args) = argument(args)?;
    let path = PathBuf::from(script);
    if !path.is_absolute()
        || !path.ends_with("bin/cc-statusline.js")
        || !matches!(args.trim(), "" | "render")
    {
        return None;
    }
    Some(path)
}

fn merge(base: &mut Value, user: Value) {
    match (base, user) {
        (Value::Object(base), Value::Object(user)) => {
            for (key, value) in user {
                merge(base.entry(key).or_insert(Value::Null), value);
            }
        }
        (base, value) => *base = value,
    }
}

fn read_rules(settings_file: &Path, config_dir: &Path) -> Result<Vec<CliLongContextRule>, String> {
    let settings = read_config(settings_file)?.ok_or(NO_INSTALL)?;
    let script = settings
        .pointer("/statusLine/command")
        .and_then(Value::as_str)
        .and_then(script_path)
        .filter(|path| path.is_file())
        .ok_or(NO_INSTALL)?;
    let presets = script
        .parent()
        .and_then(Path::parent)
        .ok_or(NO_INSTALL)?
        .join("presets");
    let user = read_config(&config_dir.join("config.json"))?.unwrap_or_else(|| json!({}));
    let preset_name = user
        .get("preset")
        .and_then(Value::as_str)
        .unwrap_or("default");
    let mut parts = Path::new(preset_name).components();
    if !matches!(parts.next(), Some(Component::Normal(_)))
        || parts.next().is_some()
        || preset_name.contains(['/', '\\'])
    {
        return Err(INVALID_CONFIG.into());
    }
    let selected = presets.join(format!("{preset_name}.json"));
    let preset = match read_config(&selected)? {
        Some(value) => value,
        None if preset_name != "default" => {
            read_config(&presets.join("default.json"))?.ok_or(NO_INSTALL)?
        }
        None => return Err(NO_INSTALL.into()),
    };
    let mut modules = preset.get("modules").cloned().unwrap_or(Value::Null);
    if let Some(value) = user.get("modules") {
        merge(&mut modules, value.clone());
    }
    let billing = &modules["billing"];
    let mut rules = BTreeMap::new();
    if let Some(prices) = billing.get("pricing") {
        for (id, model) in prices.as_object().ok_or(INVALID_CONFIG)? {
            let Some(rule) = model.get("long_context").filter(|v| !v.is_null()) else {
                continue;
            };
            if model.get("verified") != Some(&Value::Bool(true)) {
                continue;
            }
            let rule = LongContextRule {
                threshold: rule["threshold"].as_i64().ok_or(INVALID_CONFIG)?,
                input_multiplier: rule["in_mult"].as_f64().ok_or(INVALID_CONFIG)?,
                output_multiplier: rule["out_mult"].as_f64().ok_or(INVALID_CONFIG)?,
            };
            if !rule.is_valid() {
                return Err(INVALID_CONFIG.into());
            }
            rules.insert(id.clone(), rule);
        }
    }
    if let Some(aliases) = billing.get("aliases") {
        let aliases = aliases.as_object().ok_or(INVALID_CONFIG)?;
        for alias in aliases.keys() {
            let mut id = alias.as_str();
            let mut visited = HashSet::new();
            while let Some(next) = aliases.get(id) {
                if !visited.insert(id) {
                    return Err(INVALID_CONFIG.into());
                }
                id = next.as_str().ok_or(INVALID_CONFIG)?;
            }
            match rules.get(id).cloned() {
                Some(rule) => {
                    rules.insert(alias.clone(), rule);
                }
                None => {
                    rules.remove(alias);
                }
            }
        }
    }
    if rules.is_empty() {
        return Err("当前状态栏预设没有可导入的长上下文规则。".into());
    }
    Ok(rules
        .into_iter()
        .map(|(model_id, long_context)| CliLongContextRule {
            model_id,
            long_context,
        })
        .collect())
}

fn installed_rules(home: &Path) -> Result<Vec<CliLongContextRule>, String> {
    let env_path = |name| {
        env::var_os(name)
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
    };
    let settings = env_path("CLAUDE_CONFIG_DIR")
        .unwrap_or_else(|| home.join(".claude"))
        .join("settings.json");
    let config_dir = env_path("CCSL_CONFIG_DIR").unwrap_or_else(|| {
        let root = if cfg!(windows) {
            env_path("APPDATA").unwrap_or_else(|| home.join("AppData/Roaming"))
        } else {
            env_path("XDG_CONFIG_HOME").unwrap_or_else(|| home.join(".config"))
        };
        root.join("cc-statusline")
    });
    read_rules(&settings, &config_dir)
}

#[tauri::command]
pub(crate) async fn read_claude_long_context_rules(
    app: tauri::AppHandle,
) -> Result<Vec<CliLongContextRule>, String> {
    let home = app.path().home_dir().map_err(|_| NO_INSTALL)?;
    tauri::async_runtime::spawn_blocking(move || installed_rules(&home))
        .await
        .map_err(|_| "后台状态栏配置读取任务意外结束。")?
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    #[ignore = "requires an installed cc-statusline and expected rules exported from its active billing configuration"]
    fn installed_rules_match_cli_config() {
        let home = env::var_os("USERPROFILE")
            .or_else(|| env::var_os("HOME"))
            .unwrap();
        let expected: Vec<CliLongContextRule> =
            serde_json::from_str(&env::var("CCSL_TEST_EXPECTED_RULES").unwrap()).unwrap();
        let actual = installed_rules(Path::new(&home)).unwrap();
        assert_eq!(actual, expected);
    }

    #[test]
    fn active_preset_and_user_overrides_supply_only_effective_rules() {
        let temp = tempdir().unwrap();
        let dir = temp.path();
        let script = dir.join("a package/bin/cc-statusline.js");
        let presets = dir.join("a package/presets");
        fs::create_dir_all(script.parent().unwrap()).unwrap();
        fs::create_dir(&presets).unwrap();
        fs::write(&script, "never execute").unwrap();
        let settings = dir.join("settings.json");
        let command = format!("node \"{}\" render", script.display());
        fs::write(
            &settings,
            json!({"statusLine":{"command":command}}).to_string(),
        )
        .unwrap();
        fs::write(presets.join("default.json"), "{}").unwrap();
        let model =
            json!({"verified":true,"long_context":{"threshold":100,"in_mult":2,"out_mult":1.5}});
        fs::write(presets.join("custom.json"), json!({"modules":{"billing":{"pricing":{"model":model,"off":model},"aliases":{"legacy":"alias","alias":"model"}}}}).to_string()).unwrap();
        fs::write(dir.join("config.json"), json!({"preset":"custom","modules":{"billing":{"pricing":{"model":{"long_context":{"in_mult":3}},"off":{"long_context":null}}}}}).to_string()).unwrap();
        let rules = read_rules(&settings, dir).unwrap();
        assert_eq!(
            rules
                .iter()
                .map(|r| r.model_id.as_str())
                .collect::<Vec<_>>(),
            ["alias", "legacy", "model"]
        );
        assert!(rules.iter().all(|r| r.long_context.input_multiplier == 3.0
            && r.long_context.output_multiplier == 1.5
            && r.long_context.threshold == 100));
        assert_eq!(
            serde_json::to_value(&rules[2]).unwrap(),
            json!({"modelId":"model","longContext":{"threshold":100,"inputMultiplier":3.0,"outputMultiplier":1.5}})
        );
        fs::write(dir.join("config.json"), r#"{"preset":"absent"}"#).unwrap();
        assert!(read_rules(&settings, dir)
            .unwrap_err()
            .contains("没有可导入"));
        fs::write(dir.join("config.json"), "{").unwrap();
        assert_eq!(read_rules(&settings, dir).unwrap_err(), INVALID_CONFIG);
        assert_eq!(
            read_rules(&dir.join("absent"), dir).unwrap_err(),
            NO_INSTALL
        );
        assert!(script_path(&format!("{command} && echo run")).is_none());
    }
}
