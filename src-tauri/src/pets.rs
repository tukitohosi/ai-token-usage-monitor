use base64::Engine;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::Read,
    path::{Component, Path, PathBuf},
    sync::{Arc, Mutex},
};
use tauri::{AppHandle, Emitter, Manager, Window};

pub(crate) const STATE_EVENT: &str = "pet://state";
const MAX_IMAGE: u64 = 20 * 1024 * 1024;
const MAX_MANIFEST: u64 = 64 * 1024;
const BUILTIN_NAME: &str = "狐仙地灵";
const BUILTIN_DESCRIPTION: &str = "应用内置的白狐耳狐仙地灵，陪伴你查看 AI 用量。";
const BUILTIN_SPRITESHEET: &[u8] =
    include_bytes!("../resources/pets/foxian-diling/spritesheet.webp");

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PetDescriptor {
    pub id: String,
    pub display_name: String,
    pub description: String,
    pub sprite_version_number: u8,
    pub spritesheet_path: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PetSource {
    #[serde(flatten)]
    pub descriptor: PetDescriptor,
    pub source_path: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub(crate) struct PetPosition {
    pub x: i32,
    pub y: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PetPreferences {
    pub enabled: bool,
    pub selected_pet_id: Option<String>,
    pub size: u32,
    pub theme: String,
    pub position: Option<PetPosition>,
}

impl Default for PetPreferences {
    fn default() -> Self {
        Self {
            enabled: false,
            selected_pet_id: None,
            size: 128,
            theme: "app".into(),
            position: None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PetState {
    pub preferences: PetPreferences,
    pub pets: Vec<PetDescriptor>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Manifest {
    display_name: Option<String>,
    description: Option<String>,
    sprite_version_number: Option<u8>,
    spritesheet_path: Option<String>,
}

struct ValidatedPet {
    descriptor: PetDescriptor,
    bytes: Vec<u8>,
    extension: &'static str,
}

#[derive(Clone)]
pub(crate) struct PetRuntime {
    directory: PathBuf,
    roots: Vec<PathBuf>,
    preferences: Arc<Mutex<PetPreferences>>,
    preference_gate: Arc<Mutex<()>>,
    import_gate: Arc<Mutex<()>>,
}

fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>, String> {
    let file = fs::File::open(path).map_err(|_| "无法读取宠物文件。".to_string())?;
    if !file
        .metadata()
        .map_err(|_| "无法读取宠物文件信息。")?
        .is_file()
    {
        return Err("请选择宠物文件。".into());
    }
    let mut bytes = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "宠物文件读取失败。")?;
    if bytes.len() as u64 > limit {
        return Err(format!("宠物文件超过大小限制（{} KiB）。", limit / 1024));
    }
    Ok(bytes)
}

fn clean_text(text: Option<String>, fallback: &str, limit: usize) -> Result<String, String> {
    let text = text.unwrap_or_else(|| fallback.into());
    let text = text.trim();
    if text.is_empty() || text.chars().count() > limit || text.chars().any(|c| c.is_control()) {
        return Err("宠物名称或说明无效。".into());
    }
    Ok(text.into())
}

fn validate_source(source: &Path) -> Result<ValidatedPet, String> {
    let source = source.canonicalize().map_err(|_| "找不到所选宠物文件。")?;
    let is_manifest = source
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("json"));
    let fallback = source
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("自定义宠物");
    let (manifest, image_path) = if is_manifest {
        let manifest: Manifest = serde_json::from_slice(&read_bounded(&source, MAX_MANIFEST)?)
            .map_err(|_| "pet.json 格式无效。")?;
        let relative = PathBuf::from(
            manifest
                .spritesheet_path
                .as_deref()
                .unwrap_or("spritesheet.webp"),
        );
        if relative.as_os_str().is_empty()
            || relative
                .components()
                .any(|c| !matches!(c, Component::Normal(_)))
        {
            return Err("宠物图集必须是宠物目录内的相对路径。".into());
        }
        let parent = source.parent().ok_or("宠物目录无效。")?;
        let image = parent
            .join(relative)
            .canonicalize()
            .map_err(|_| "找不到宠物图集。")?;
        if !image.starts_with(parent) {
            return Err("宠物图集不能位于宠物目录之外。".into());
        }
        (Some(manifest), image)
    } else {
        (None, source.clone())
    };
    let bytes = read_bounded(&image_path, MAX_IMAGE)?;
    let format = image::guess_format(&bytes).map_err(|_| "宠物图集必须是 PNG 或 WebP 图片。")?;
    let extension = match format {
        image::ImageFormat::Png => "png",
        image::ImageFormat::WebP => "webp",
        _ => return Err("宠物图集只支持 PNG 或 WebP。".into()),
    };
    if !image_path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case(extension))
    {
        return Err("宠物图片扩展名与实际格式不一致。".into());
    }
    // Read dimensions before decoding so malformed files cannot allocate unbounded images.
    let dimensions = image::ImageReader::with_format(std::io::Cursor::new(&bytes), format)
        .into_dimensions()
        .map_err(|_| "宠物图集已损坏。")?;
    let version = match dimensions {
        (1536, 1872) => 1,
        (1536, 2288) => 2,
        _ => return Err("宠物图集尺寸不兼容：v1 应为 1536×1872，v2 应为 1536×2288。".into()),
    };
    if manifest
        .as_ref()
        .and_then(|m| m.sprite_version_number)
        .is_some_and(|v| v != version)
    {
        return Err("宠物版本与图集尺寸不一致，仅支持 v1 和 v2。".into());
    }
    image::load_from_memory_with_format(&bytes, format).map_err(|_| "宠物图集无法完整解码。")?;
    let name = clean_text(
        manifest.as_ref().and_then(|m| m.display_name.clone()),
        fallback,
        100,
    )?;
    let description = manifest
        .as_ref()
        .and_then(|m| m.description.clone())
        .unwrap_or_default();
    if description.chars().count() > 2000
        || description.chars().any(|c| c.is_control() && c != '\n')
    {
        return Err("宠物说明过长或含无效字符。".into());
    }
    let id = pet_id(&bytes, &name);
    Ok(ValidatedPet {
        descriptor: PetDescriptor {
            id,
            display_name: name,
            description,
            sprite_version_number: version,
            spritesheet_path: image_path.to_string_lossy().into_owned(),
        },
        bytes,
        extension,
    })
}

fn pet_id(bytes: &[u8], name: &str) -> String {
    let mut hash = blake3::Hasher::new();
    hash.update(bytes);
    hash.update(name.as_bytes());
    format!("pet-{}", &hash.finalize().to_hex()[..24])
}

fn validate_preferences(p: &PetPreferences) -> Result<(), String> {
    if !(96..=192).contains(&p.size) || !matches!(p.theme.as_str(), "app" | "light" | "dark") {
        return Err("宠物大小或主题设置无效。".into());
    }
    if p.selected_pet_id.as_ref().is_some_and(|id| {
        !id.starts_with("pet-") || id.len() != 28 || !id[4..].bytes().all(|c| c.is_ascii_hexdigit())
    }) {
        return Err("宠物选择无效。".into());
    }
    Ok(())
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let temp = path.with_extension(format!("{}.tmp", std::process::id()));
    let result = (|| {
        use std::io::Write;
        let mut file = fs::File::create(&temp).map_err(|_| "无法保存宠物设置。")?;
        file.write_all(bytes)
            .and_then(|_| file.sync_all())
            .map_err(|_| "宠物设置写入失败。")?;
        fs::rename(&temp, path).map_err(|_| "无法替换宠物设置文件。")
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result.map_err(str::to_owned)
}

impl PetRuntime {
    pub(crate) fn new(app_data: PathBuf, codex_home: PathBuf, user_home: PathBuf) -> Self {
        let directory = app_data.join("pets");
        let _ = fs::create_dir_all(&directory);
        let mut roots = vec![
            codex_home.join("pets"),
            user_home.join(".codex").join("pets"),
        ];
        roots.dedup();
        let saved = read_bounded(&directory.join("preferences.json"), MAX_MANIFEST)
            .ok()
            .and_then(|b| serde_json::from_slice::<PetPreferences>(&b).ok())
            .filter(|p| validate_preferences(p).is_ok())
            .unwrap_or_default();
        let runtime = Self {
            directory,
            roots,
            preferences: Arc::new(Mutex::new(saved)),
            preference_gate: Arc::new(Mutex::new(())),
            import_gate: Arc::new(Mutex::new(())),
        };
        let builtin = runtime
            .ensure_builtin()
            .map_err(|error| {
                log::warn!("Built-in pet could not be restored: {error}");
                error
            })
            .ok();
        let state = runtime.state();
        if !state
            .pets
            .iter()
            .any(|p| Some(&p.id) == state.preferences.selected_pet_id.as_ref())
        {
            let mut prefs = runtime
                .preferences
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            prefs.enabled = false;
            prefs.selected_pet_id = builtin.map(|pet| pet.id);
            drop(prefs);
            let _ = runtime.persist();
        }
        runtime
    }

    fn ensure_builtin(&self) -> Result<PetDescriptor, String> {
        let id = pet_id(BUILTIN_SPRITESHEET, BUILTIN_NAME);
        let destination = self.directory.join(&id);
        fs::create_dir_all(&destination).map_err(|_| "无法创建内置宠物目录。")?;
        let image_path = destination.join("spritesheet.webp");
        // Restore an absent or damaged bundled asset without depending on Codex.
        // Reuse the import identity so an existing identical pet is not duplicated.
        if read_bounded(&image_path, MAX_IMAGE).ok().as_deref() != Some(BUILTIN_SPRITESHEET) {
            atomic_write(&image_path, BUILTIN_SPRITESHEET)?;
        }
        let mut descriptor = PetDescriptor {
            id,
            display_name: BUILTIN_NAME.into(),
            description: BUILTIN_DESCRIPTION.into(),
            sprite_version_number: 2,
            spritesheet_path: image_path.to_string_lossy().into_owned(),
        };
        let manifest_path = destination.join("pet.json");
        if let Some(existing) = read_bounded(&manifest_path, MAX_MANIFEST)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<PetDescriptor>(&bytes).ok())
            .filter(|pet| {
                pet.id == descriptor.id
                    && pet.display_name == BUILTIN_NAME
                    && pet.sprite_version_number == 2
                    && pet.description.chars().count() <= 2000
                    && !pet.description.chars().any(|c| c.is_control() && c != '\n')
            })
        {
            descriptor.description = existing.description;
        }
        let manifest = serde_json::to_vec_pretty(&descriptor).map_err(|_| "内置宠物信息无效。")?;
        if read_bounded(&manifest_path, MAX_MANIFEST).ok().as_deref() != Some(manifest.as_slice()) {
            atomic_write(&manifest_path, &manifest)?;
        }
        Ok(descriptor)
    }

    pub(crate) fn state(&self) -> PetState {
        let mut pets = Vec::new();
        if let Ok(entries) = fs::read_dir(&self.directory) {
            for entry in entries.flatten() {
                let manifest = entry.path().join("pet.json");
                if !manifest.is_file() {
                    continue;
                }
                if let Ok(bytes) = read_bounded(&manifest, MAX_MANIFEST) {
                    if let Ok(mut descriptor) = serde_json::from_slice::<PetDescriptor>(&bytes) {
                        let expected = entry.path().join("spritesheet.webp");
                        let png = entry.path().join("spritesheet.png");
                        let sprite = if expected.is_file() { expected } else { png };
                        if sprite.is_file() && descriptor.id == entry.file_name().to_string_lossy()
                        {
                            descriptor.spritesheet_path = sprite.to_string_lossy().into_owned();
                            pets.push(descriptor);
                        }
                    }
                }
            }
        }
        pets.sort_by(|a, b| a.display_name.cmp(&b.display_name));
        PetState {
            preferences: self
                .preferences
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .clone(),
            pets,
        }
    }

    fn persist(&self) -> Result<(), String> {
        fs::create_dir_all(&self.directory).map_err(|_| "无法创建宠物目录。")?;
        let prefs = self.preferences.lock().unwrap_or_else(|p| p.into_inner());
        atomic_write(
            &self.directory.join("preferences.json"),
            &serde_json::to_vec_pretty(&*prefs).map_err(|_| "宠物设置无效。")?,
        )
    }

    pub(crate) fn save_preferences(&self, p: PetPreferences) -> Result<PetState, String> {
        let _gate = self
            .preference_gate
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        validate_preferences(&p)?;
        if (p.enabled || p.selected_pet_id.is_some())
            && !self
                .state()
                .pets
                .iter()
                .any(|pet| Some(&pet.id) == p.selected_pet_id.as_ref())
        {
            return Err("请先导入并选择一个可用宠物。".into());
        }
        let old = self
            .preferences
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        *self.preferences.lock().unwrap_or_else(|p| p.into_inner()) = p;
        if let Err(error) = self.persist() {
            *self.preferences.lock().unwrap_or_else(|p| p.into_inner()) = old;
            return Err(error);
        }
        Ok(self.state())
    }

    pub(crate) fn save_position(&self, position: PetPosition) -> Result<(), String> {
        let _gate = self
            .preference_gate
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        self.preferences
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .position = Some(position);
        self.persist()
    }

    pub(crate) fn discover(&self, extra: Option<&Path>) -> Vec<PetSource> {
        let roots = extra
            .map(|p| vec![p.to_path_buf(), p.join("pets")])
            .unwrap_or_else(|| self.roots.clone());
        let mut sources = Vec::new();
        let mut paths = std::collections::HashSet::new();
        for root in roots {
            let mut manifests = vec![root.join("pet.json")];
            if let Ok(entries) = fs::read_dir(&root) {
                manifests.extend(
                    entries
                        .flatten()
                        .take(200)
                        .map(|e| e.path().join("pet.json")),
                );
            }
            for path in manifests {
                let Ok(path) = path.canonicalize() else {
                    continue;
                };
                if !paths.insert(path.clone()) {
                    continue;
                }
                if let Ok(pet) = validate_source(&path) {
                    sources.push(PetSource {
                        descriptor: pet.descriptor,
                        source_path: path.to_string_lossy().into_owned(),
                    });
                }
            }
        }
        sources.sort_by(|a, b| a.descriptor.display_name.cmp(&b.descriptor.display_name));
        sources
    }

    fn import(&self, source: &Path) -> Result<PetState, String> {
        let _gate = self.import_gate.lock().unwrap_or_else(|p| p.into_inner());
        let pet = validate_source(source)?;
        let destination = self.directory.join(&pet.descriptor.id);
        fs::create_dir_all(&destination).map_err(|_| "无法创建宠物资源目录。")?;
        let image_path = destination.join(format!("spritesheet.{}", pet.extension));
        atomic_write(&image_path, &pet.bytes)?;
        let mut descriptor = pet.descriptor;
        descriptor.spritesheet_path = image_path.to_string_lossy().into_owned();
        atomic_write(
            &destination.join("pet.json"),
            &serde_json::to_vec_pretty(&descriptor).map_err(|_| "宠物信息无效。")?,
        )?;
        let mut prefs = self.state().preferences;
        prefs.selected_pet_id = Some(descriptor.id);
        self.save_preferences(prefs)
    }

    pub(crate) fn allow_assets(&self, app: &AppHandle, state: &PetState) -> Result<(), String> {
        // The static scope may be initialized before this application's pet directory exists.
        // Grant only the copied application-owned images, never a selected Codex source directory.
        let root = self
            .directory
            .canonicalize()
            .map_err(|_| "无法读取应用宠物目录。")?;
        let scope = app.asset_protocol_scope();
        for pet in &state.pets {
            let path = Path::new(&pet.spritesheet_path)
                .canonicalize()
                .map_err(|_| "找不到已导入的宠物图集。")?;
            let folder = self
                .directory
                .join(&pet.id)
                .canonicalize()
                .map_err(|_| "宠物资源目录无效。")?;
            if !folder.starts_with(&root)
                || path.parent() != Some(folder.as_path())
                || !matches!(
                    path.file_name().and_then(|v| v.to_str()),
                    Some("spritesheet.png" | "spritesheet.webp")
                )
            {
                return Err("宠物图集必须保存在应用自己的宠物目录。".into());
            }
            scope
                .allow_file(&path)
                .map_err(|_| "无法授权读取已导入的宠物图集。")?;
        }
        Ok(())
    }

    pub(crate) fn publish(&self, app: &AppHandle) {
        let state = self.state();
        if let Err(error) = self.allow_assets(app, &state) {
            log::warn!("Pet asset permission failed: {error}");
        }
        if let Some(tray) = app.try_state::<crate::tray::TrayMenuHandle<tauri::Wry>>() {
            let _ = tray.set_pet_enabled(state.preferences.enabled);
        }
        let _ = app.emit(STATE_EVENT, state);
    }
}

fn require_main(window: &Window) -> Result<(), String> {
    if window.label() == crate::tray::MAIN_WINDOW_LABEL {
        Ok(())
    } else {
        Err("此操作只能从主窗口执行。".into())
    }
}

#[tauri::command]
pub(crate) fn read_pet_state(
    app: AppHandle,
    runtime: tauri::State<'_, PetRuntime>,
) -> Result<PetState, String> {
    let state = runtime.state();
    runtime.allow_assets(&app, &state)?;
    Ok(state)
}

#[tauri::command]
pub(crate) async fn discover_codex_pets(
    window: Window,
    runtime: tauri::State<'_, PetRuntime>,
) -> Result<Vec<PetSource>, String> {
    require_main(&window)?;
    let runtime = runtime.inner().clone();
    tauri::async_runtime::spawn_blocking(move || runtime.discover(None))
        .await
        .map_err(|_| "宠物发现任务意外结束。".into())
}

#[tauri::command(rename_all = "camelCase")]
pub(crate) async fn read_pet_preview(
    window: Window,
    source_path: String,
) -> Result<String, String> {
    require_main(&window)?;
    tauri::async_runtime::spawn_blocking(move || {
        let pet = validate_source(Path::new(&source_path))?;
        Ok(format!(
            "data:image/{};base64,{}",
            pet.extension,
            base64::engine::general_purpose::STANDARD.encode(pet.bytes)
        ))
    })
    .await
    .map_err(|_| "宠物预览任务意外结束。".to_string())?
}

#[tauri::command(rename_all = "camelCase")]
pub(crate) async fn import_codex_pet(
    app: AppHandle,
    window: Window,
    runtime: tauri::State<'_, PetRuntime>,
    source_path: String,
) -> Result<PetState, String> {
    require_main(&window)?;
    let runtime = runtime.inner().clone();
    let worker = runtime.clone();
    let state =
        tauri::async_runtime::spawn_blocking(move || worker.import(Path::new(&source_path)))
            .await
            .map_err(|_| "宠物导入任务意外结束。".to_string())??;
    crate::pet_window::apply(&app, &state)?;
    runtime.publish(&app);
    Ok(state)
}

#[tauri::command]
pub(crate) async fn select_pet_file(
    app: AppHandle,
    window: Window,
    runtime: tauri::State<'_, PetRuntime>,
) -> Result<Option<PetState>, String> {
    require_main(&window)?;
    let runtime = runtime.inner().clone();
    let worker = runtime.clone();
    let state = tauri::async_runtime::spawn_blocking(move || {
        rfd::FileDialog::new()
            .set_parent(&window)
            .add_filter("Codex 宠物", &["json", "png", "webp"])
            .pick_file()
            .map(|path| worker.import(&path))
            .transpose()
    })
    .await
    .map_err(|_| "宠物选择任务意外结束。".to_string())??;
    if let Some(state) = &state {
        crate::pet_window::apply(&app, state)?;
        runtime.publish(&app);
    }
    Ok(state)
}

#[tauri::command]
pub(crate) async fn select_codex_pet_directory(
    window: Window,
    runtime: tauri::State<'_, PetRuntime>,
) -> Result<Option<Vec<PetSource>>, String> {
    require_main(&window)?;
    let runtime = runtime.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        rfd::FileDialog::new()
            .set_parent(&window)
            .set_title("选择 Codex 配置目录或宠物目录")
            .pick_folder()
            .map(|path| runtime.discover(Some(&path)))
    })
    .await
    .map_err(|_| "宠物目录选择任务意外结束。".into())
}

#[tauri::command]
pub(crate) async fn set_pet_preferences(
    app: AppHandle,
    runtime: tauri::State<'_, PetRuntime>,
    mut preferences: PetPreferences,
) -> Result<PetState, String> {
    let old = runtime.state().preferences;
    // Position belongs to native dragging/reset, never a potentially stale settings form.
    preferences.position = old.position;
    let state = runtime.save_preferences(preferences)?;
    if let Err(error) = crate::pet_window::apply(&app, &state) {
        let _ = runtime.save_preferences(old);
        return Err(error);
    }
    runtime.publish(&app);
    Ok(state)
}

#[tauri::command]
pub(crate) async fn reset_pet_position(
    app: AppHandle,
    runtime: tauri::State<'_, PetRuntime>,
) -> Result<PetState, String> {
    let mut prefs = runtime.state().preferences;
    prefs.position = None;
    runtime.save_preferences(prefs)?;
    crate::pet_window::reset_position(&app)?;
    runtime.publish(&app);
    Ok(runtime.state())
}

#[tauri::command]
pub(crate) fn show_pet_dashboard(app: AppHandle) {
    crate::tray::show_main_window(&app);
    let _ = app.emit("app://navigate", "overview");
}

#[cfg(test)]
mod tests {
    use super::*;
    fn image(path: &Path, height: u32) {
        image::RgbaImage::new(1536, height).save(path).unwrap();
    }
    #[test]
    fn builtin_is_available_without_codex_and_restores_its_resources() {
        let temp = tempfile::tempdir().unwrap();
        let app = temp.path().join("app");
        let codex = temp.path().join("absent-codex");
        let home = temp.path().join("absent-home");
        let runtime = PetRuntime::new(app.clone(), codex.clone(), home.clone());
        let state = runtime.state();
        assert_eq!(state.pets.len(), 1);
        let builtin = &state.pets[0];
        println!("Built-in pet ID: {}", builtin.id);
        assert_eq!(builtin.display_name, BUILTIN_NAME);
        assert_eq!(builtin.sprite_version_number, 2);
        assert_eq!(
            state.preferences.selected_pet_id.as_ref(),
            Some(&builtin.id)
        );
        assert!(!state.preferences.enabled);
        let validated = validate_source(Path::new(&builtin.spritesheet_path)).unwrap();
        assert_eq!(validated.bytes, BUILTIN_SPRITESHEET);
        let dimensions = image::ImageReader::with_format(
            std::io::Cursor::new(BUILTIN_SPRITESHEET),
            image::ImageFormat::WebP,
        )
        .into_dimensions()
        .unwrap();
        assert_eq!(dimensions, (1536, 2288));

        let mut preferences = state.preferences;
        preferences.enabled = true;
        preferences.position = Some(PetPosition { x: 85, y: 120 });
        runtime.save_preferences(preferences.clone()).unwrap();
        fs::write(&builtin.spritesheet_path, b"damaged").unwrap();
        fs::remove_file(Path::new(&builtin.spritesheet_path).with_file_name("pet.json")).unwrap();
        let restored = PetRuntime::new(app, codex, home).state();
        assert_eq!(restored.preferences, preferences);
        assert_eq!(restored.pets, state.pets);
        assert_eq!(
            fs::read(&restored.pets[0].spritesheet_path).unwrap(),
            BUILTIN_SPRITESHEET
        );
    }

    #[test]
    fn builtin_reuses_import_identity_and_preserves_another_selected_pet() {
        let temp = tempfile::tempdir().unwrap();
        let app = temp.path().join("app");
        let codex = temp.path().join("codex");
        let home = temp.path().join("home");
        let runtime = PetRuntime::new(app.clone(), codex.clone(), home.clone());
        let builtin = runtime.state().pets[0].clone();
        let source_dir = temp.path().join("builtin-source");
        fs::create_dir(&source_dir).unwrap();
        fs::write(source_dir.join("spritesheet.webp"), BUILTIN_SPRITESHEET).unwrap();
        let manifest = source_dir.join("pet.json");
        fs::write(&manifest, r#"{"displayName":"狐仙地灵","spriteVersionNumber":2,"spritesheetPath":"spritesheet.webp"}"#).unwrap();
        let imported = runtime.import(&manifest).unwrap();
        assert_eq!(imported.pets.len(), 1);
        assert_eq!(
            imported.preferences.selected_pet_id.as_ref(),
            Some(&builtin.id)
        );

        let source = temp.path().join("custom.png");
        image(&source, 1872);
        let imported = runtime.import(&source).unwrap();
        assert_eq!(imported.pets.len(), 2);
        let mut preferences = imported.preferences;
        preferences.enabled = true;
        preferences.size = 160;
        preferences.theme = "dark".into();
        runtime.save_preferences(preferences.clone()).unwrap();
        let loaded = PetRuntime::new(app, codex, home).state();
        assert_eq!(loaded.preferences, preferences);
        assert_eq!(loaded.pets, imported.pets);
    }

    #[test]
    fn import_copies_and_survives_source_removal() {
        let temp = tempfile::tempdir().unwrap();
        let src = temp.path().join("source");
        fs::create_dir(&src).unwrap();
        image(&src.join("spritesheet.png"), 2288);
        fs::write(
            src.join("pet.json"),
            r#"{"displayName":"狐仙","spriteVersionNumber":2,"spritesheetPath":"spritesheet.png"}"#,
        )
        .unwrap();
        let runtime = PetRuntime::new(
            temp.path().join("app"),
            temp.path().join("codex"),
            temp.path().join("home"),
        );
        let state = runtime.import(&src.join("pet.json")).unwrap();
        assert_eq!(state.pets.len(), 2);
        assert!(!state.preferences.enabled);
        fs::remove_dir_all(&src).unwrap();
        assert!(Path::new(&runtime.state().pets[0].spritesheet_path).is_file());
        let mut prefs = state.preferences;
        prefs.enabled = true;
        runtime.save_preferences(prefs).unwrap();
        let loaded = PetRuntime::new(
            temp.path().join("app"),
            temp.path().join("codex"),
            temp.path().join("home"),
        );
        assert!(loaded.state().preferences.enabled);
    }
    #[test]
    fn rejects_escape_bad_size_and_missing_selection() {
        let temp = tempfile::tempdir().unwrap();
        let src = temp.path().join("pet.json");
        fs::write(&src, r#"{"spritesheetPath":"../outside.png"}"#).unwrap();
        assert!(validate_source(&src).is_err());
        image(&temp.path().join("bad.png"), 100);
        assert!(validate_source(&temp.path().join("bad.png")).is_err());
        let runtime = PetRuntime::new(
            temp.path().join("app"),
            temp.path().join("codex"),
            temp.path().join("home"),
        );
        let mut prefs = PetPreferences::default();
        prefs.enabled = true;
        assert!(runtime.save_preferences(prefs).is_err());
        assert!(!runtime.state().preferences.enabled);
    }
    #[test]
    fn detects_v1_and_rejects_wrong_version() {
        let temp = tempfile::tempdir().unwrap();
        image(&temp.path().join("sprite.png"), 1872);
        assert_eq!(
            validate_source(&temp.path().join("sprite.png"))
                .unwrap()
                .descriptor
                .sprite_version_number,
            1
        );
        fs::write(
            temp.path().join("pet.json"),
            r#"{"spriteVersionNumber":2,"spritesheetPath":"sprite.png"}"#,
        )
        .unwrap();
        assert!(validate_source(&temp.path().join("pet.json")).is_err());
    }
    #[test]
    fn invalid_import_preserves_existing_library_and_preferences() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("sprite.png");
        image(&source, 2288);
        let runtime = PetRuntime::new(
            temp.path().join("app"),
            temp.path().join("codex"),
            temp.path().join("home"),
        );
        let before = runtime.import(&source).unwrap();
        fs::write(temp.path().join("bad.webp"), b"not an image").unwrap();
        assert!(runtime.import(&temp.path().join("bad.webp")).is_err());
        let after = runtime.state();
        assert_eq!(before.preferences, after.preferences);
        assert_eq!(before.pets, after.pets);
        let duplicate = runtime.import(&source).unwrap();
        assert_eq!(duplicate.pets.len(), 2);
    }
    #[test]
    #[ignore = "Optional local Codex fixtures; set AI_TOKEN_PET_FIXTURES to a pet library directory."]
    fn imports_local_codex_fixture_library() {
        let fixture = PathBuf::from(
            std::env::var_os("AI_TOKEN_PET_FIXTURES").expect("fixture directory required"),
        );
        let temp = tempfile::tempdir().unwrap();
        let runtime = PetRuntime::new(
            temp.path().join("app"),
            fixture.parent().unwrap().to_path_buf(),
            temp.path().join("home"),
        );
        let sources = runtime.discover(Some(&fixture));
        assert!(!sources.is_empty());
        for source in &sources {
            let state = runtime.import(Path::new(&source.source_path)).unwrap();
            assert!(state.pets.iter().any(|p| p.id == source.descriptor.id));
        }
        let expected_ids: std::collections::HashSet<_> = sources
            .iter()
            .map(|source| source.descriptor.id.clone())
            .chain(std::iter::once(pet_id(BUILTIN_SPRITESHEET, BUILTIN_NAME)))
            .collect();
        assert_eq!(runtime.state().pets.len(), expected_ids.len());
        println!(
            "Imported and decoded {} local Codex pets into an isolated temporary library.",
            sources.len()
        );
    }
}
