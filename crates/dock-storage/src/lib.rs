//! Glide-Dock configuration persistence.
//!
//! Responsibilities:
//! * locate `%APPDATA%\Glide-Dock\config.json`
//! * load with recovery from corrupted JSON (backup + defaults)
//! * migrate old schema versions by editing `serde_json::Value` only
//! * save atomically (temp file + flush + replace)

#![warn(missing_docs)]

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use dock_core::{CONFIG_VERSION, DockConfig};
use serde_json::Value;

/// Errors that can escape a load/save operation.
#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    /// Filesystem operation failed.
    #[error("I/O error on {path}: {message}")]
    Io {
        /// Path involved in the failure.
        path: PathBuf,
        /// Human readable reason.
        message: String,
    },
    /// The configuration root was not a JSON object.
    #[error("configuration root must be a JSON object")]
    NotAnObject,
    /// Serialization failed.
    #[error("failed to serialize configuration: {0}")]
    Serialize(String),
    /// A newer version cannot be migrated.
    #[error("configuration version {0} is newer than this build")]
    UnsupportedVersion(u32),
}

/// Result of loading the configuration.
#[derive(Debug, Clone)]
pub struct LoadOutcome {
    /// Loaded (and normalized) configuration.
    pub config: DockConfig,
    /// Non fatal problems, already logged.
    pub warnings: Vec<String>,
    /// Backup created for a corrupt file.
    pub backup: Option<PathBuf>,
    /// `true` when the file came from a newer build and must not be written.
    pub read_only: bool,
    /// `true` when a migration was applied in memory.
    pub migrated: bool,
}

/// A configuration file handle.
#[derive(Debug, Clone)]
pub struct ConfigStore {
    path: PathBuf,
}

impl ConfigStore {
    /// Creates a store for an explicit file path.
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// Creates a store at `%APPDATA%\Glide-Dock\config.json`.
    #[must_use]
    pub fn default_location() -> Self {
        Self::new(config_dir().join("config.json"))
    }

    /// Path of the configuration file.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Directory containing the configuration file.
    #[must_use]
    pub fn directory(&self) -> PathBuf {
        self.path
            .parent()
            .map_or_else(|| PathBuf::from("."), Path::to_path_buf)
    }

    /// Loads the configuration, repairing and backing up as needed.
    ///
    /// Missing files yield the default configuration. Corrupt files are copied
    /// to `config.corrupt-<unix>.json` and replaced in memory by defaults.
    pub fn load(&self) -> Result<LoadOutcome, StorageError> {
        let mut warnings = Vec::new();
        if !self.path.exists() {
            return Ok(LoadOutcome {
                config: DockConfig::default(),
                warnings,
                backup: None,
                read_only: false,
                migrated: false,
            });
        }

        let bytes = fs::read(&self.path).map_err(|error| StorageError::Io {
            path: self.path.clone(),
            message: error.to_string(),
        })?;

        let value: Value = match serde_json::from_slice(&bytes) {
            Ok(value) => value,
            Err(error) => {
                let message = format!("config.json is not valid JSON: {error}");
                tracing::error!(path = %self.path.display(), %error, "configuration is corrupt");
                warnings.push(message);
                let backup = self.backup_corrupt().ok();
                if let Some(path) = &backup {
                    warnings.push(format!("corrupt file backed up to {}", path.display()));
                }
                return Ok(LoadOutcome {
                    config: DockConfig::default(),
                    warnings,
                    backup,
                    read_only: false,
                    migrated: false,
                });
            }
        };

        let version = match value.get("version") {
            None => 0,
            Some(Value::Number(number)) => match number.as_u64() {
                Some(version) if version <= u64::from(u32::MAX) => version as u32,
                _ => {
                    // A non-integer or absurd version is not safe to migrate;
                    // treat the file like a future version and keep it intact.
                    let message =
                        "config.json has a non-integer version; opening read-only".to_string();
                    tracing::warn!(path = %self.path.display(), "configuration version is invalid");
                    warnings.push(message);
                    let config = parse_ignoring_version(&value);
                    return Ok(LoadOutcome {
                        config,
                        warnings,
                        backup: None,
                        read_only: true,
                        migrated: false,
                    });
                }
            },
            Some(_) => {
                let message =
                    "config.json has a non-integer version; opening read-only".to_string();
                tracing::warn!(path = %self.path.display(), "configuration version is invalid");
                warnings.push(message);
                let config = parse_ignoring_version(&value);
                return Ok(LoadOutcome {
                    config,
                    warnings,
                    backup: None,
                    read_only: true,
                    migrated: false,
                });
            }
        };

        if !value.is_object() {
            let message = "config.json root is not a JSON object".to_string();
            tracing::error!(path = %self.path.display(), "configuration is invalid");
            warnings.push(message);
            let backup = self.backup_corrupt().ok();
            if let Some(path) = &backup {
                warnings.push(format!("invalid file backed up to {}", path.display()));
            }
            return Ok(LoadOutcome {
                config: DockConfig::default(),
                warnings,
                backup,
                read_only: false,
                migrated: false,
            });
        }

        if version > CONFIG_VERSION {
            let mut config: DockConfig = serde_json::from_value(value.clone()).unwrap_or_default();
            config.normalize();
            tracing::warn!(version, "configuration is from a newer build; read-only");
            warnings.push(format!(
                "configuration version {version} is newer than supported version \
                 {CONFIG_VERSION}; changes will not be saved"
            ));
            return Ok(LoadOutcome {
                config,
                warnings,
                backup: None,
                read_only: true,
                migrated: false,
            });
        }

        let migrated = version < CONFIG_VERSION;
        let migrated_value = if migrated {
            migrate_value(value, version)?
        } else {
            value
        };

        let mut config: DockConfig = match serde_json::from_value(migrated_value) {
            Ok(config) => config,
            Err(error) => {
                // A structurally broken but syntactically valid file is
                // recovered like corrupt JSON rather than aborting startup.
                let message = format!("config.json has an invalid structure: {error}");
                tracing::error!(path = %self.path.display(), %error, "configuration is invalid");
                warnings.push(message);
                let backup = self.backup_corrupt().ok();
                if let Some(path) = &backup {
                    warnings.push(format!("invalid file backed up to {}", path.display()));
                }
                return Ok(LoadOutcome {
                    config: DockConfig::default(),
                    warnings,
                    backup,
                    read_only: false,
                    migrated: false,
                });
            }
        };
        config.normalize();

        Ok(LoadOutcome {
            config,
            warnings,
            backup: None,
            read_only: false,
            migrated,
        })
    }

    /// Writes the configuration atomically.
    ///
    /// The temporary file lives in the same directory as the target so the
    /// rename stays on one volume and is atomic.
    pub fn save(&self, config: &DockConfig) -> Result<(), StorageError> {
        let directory = self.directory();
        fs::create_dir_all(&directory).map_err(|error| StorageError::Io {
            path: directory.clone(),
            message: error.to_string(),
        })?;

        let mut bytes = serde_json::to_vec_pretty(config)
            .map_err(|error| StorageError::Serialize(error.to_string()))?;
        bytes.push(b'\n');

        let temp_path = self.path.with_extension("json.tmp");
        {
            let mut file = fs::File::create(&temp_path).map_err(|error| StorageError::Io {
                path: temp_path.clone(),
                message: error.to_string(),
            })?;
            file.write_all(&bytes).map_err(|error| StorageError::Io {
                path: temp_path.clone(),
                message: error.to_string(),
            })?;
            file.sync_all().map_err(|error| StorageError::Io {
                path: temp_path.clone(),
                message: error.to_string(),
            })?;
        }

        fs::rename(&temp_path, &self.path).map_err(|error| {
            let _ = fs::remove_file(&temp_path);
            StorageError::Io {
                path: self.path.clone(),
                message: error.to_string(),
            }
        })?;

        tracing::info!(
            path = %self.path.display(),
            bytes = bytes.len(),
            items = config.items.len(),
            version = config.version,
            "configuration saved"
        );
        Ok(())
    }

    /// Copies the current file to `config.corrupt-<unix>.json`.
    fn backup_corrupt(&self) -> Result<PathBuf, StorageError> {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis())
            .unwrap_or(0);
        let file_name = self
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("config.json");
        let backup = self
            .directory()
            .join(format!("{file_name}.corrupt-{stamp}.json"));
        fs::copy(&self.path, &backup).map_err(|error| StorageError::Io {
            path: backup,
            message: error.to_string(),
        })?;
        Ok(self
            .directory()
            .join(format!("{file_name}.corrupt-{stamp}.json")))
    }
}

/// Returns `%APPDATA%\Glide-Dock`, falling back to the current directory.
#[must_use]
pub fn config_dir() -> PathBuf {
    if let Ok(app_data) = std::env::var("APPDATA") {
        if !app_data.trim().is_empty() {
            return PathBuf::from(app_data).join("Glide-Dock");
        }
    }
    PathBuf::from("Glide-Dock")
}

/// Returns `%LOCALAPPDATA%\Glide-Dock\logs`, falling back to the current dir.
#[must_use]
pub fn log_dir() -> PathBuf {
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        if !local.trim().is_empty() {
            return PathBuf::from(local).join("Glide-Dock").join("logs");
        }
    }
    config_dir().join("logs")
}

/// Parses a configuration whose `version` field is unusable, preserving every
/// other field. The in-memory version is normalized to the current schema; the
/// file itself is never written because the load is read-only.
fn parse_ignoring_version(value: &Value) -> DockConfig {
    let mut sanitized = value.clone();
    if let Some(object) = sanitized.as_object_mut() {
        object.remove("version");
    }
    let mut config: DockConfig = serde_json::from_value(sanitized).unwrap_or_default();
    config.version = CONFIG_VERSION;
    config.normalize();
    config
}

/// Applies the migration chain to a JSON value.
///
/// Migrations never deserialize into the strongly typed model, so unknown
/// fields written by other builds survive the round trip.
fn migrate_value(mut value: Value, from: u32) -> Result<Value, StorageError> {
    let Some(object) = value.as_object_mut() else {
        return Err(StorageError::NotAnObject);
    };

    let mut version = from;
    while version < CONFIG_VERSION {
        match version {
            0 => migrate_v0_to_v1(object),
            1 => migrate_v1_to_v2(object),
            2 => migrate_v2_to_v3(object),
            other => return Err(StorageError::UnsupportedVersion(other)),
        }
        version += 1;
        object.insert("version".to_string(), Value::from(version));
    }
    Ok(value)
}

fn migrate_v0_to_v1(object: &mut serde_json::Map<String, Value>) {
    object
        .entry("monitor".to_string())
        .or_insert_with(|| serde_json::json!({ "display_id": "", "offset": 0.0 }));

    if !object.contains_key("next_item_id") {
        let max_id = object
            .get("items")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.get("id").and_then(Value::as_u64))
                    .max()
                    .unwrap_or(0)
            })
            .unwrap_or(0);
        object.insert("next_item_id".to_string(), Value::from(max_id + 1));
    }

    if let Some(items) = object.get_mut("items").and_then(Value::as_array_mut) {
        for item in items {
            if let Some(kind) = item.get_mut("kind") {
                if let Some(text) = kind.as_str() {
                    let lowered = text.to_ascii_lowercase();
                    *kind = Value::from(lowered);
                }
            }
        }
    }
}

/// Moves the v1 spring/magnification numbers into the v2 `motion` section.
///
/// Unknown fields are preserved. If a `motion` object already exists (for
/// example a hand-edited file) it wins over the derived defaults.
fn migrate_v1_to_v2(object: &mut serde_json::Map<String, Value>) {
    let mut factor = 1.35_f64;
    let mut range = 2.6_f64;
    let mut stiffness = 560.0_f64;
    let mut damping = 0.72_f64;

    if let Some(behavior) = object.get_mut("behavior").and_then(Value::as_object_mut) {
        if let Some(value) = behavior.remove("magnification_factor") {
            factor = value.as_f64().unwrap_or(factor);
        }
        if let Some(value) = behavior.remove("magnification_range") {
            range = value.as_f64().unwrap_or(range);
        }
        if let Some(value) = behavior.remove("stiffness") {
            stiffness = value.as_f64().unwrap_or(stiffness);
        }
        if let Some(value) = behavior.remove("damping") {
            damping = value.as_f64().unwrap_or(damping);
        }
    }

    let zeta = damping.clamp(0.35, 1.0);
    let omega = stiffness.max(1.0).sqrt();
    let hover_s = (3.912 / (zeta * omega)).clamp(0.08, 0.4);
    let hover_ms = (hover_s * 1000.0).round().clamp(80.0, 400.0) as u64;
    let elasticity = ((1.0 - zeta) / 0.4).clamp(0.0, 1.0);
    let elasticity = (elasticity * 100.0).round() / 100.0;

    object.entry("motion".to_string()).or_insert_with(|| {
        serde_json::json!({
            "enabled": true,
            "respect_reduced_motion": true,
            "hover_preset": "glide_zoom",
            "launch_preset": "glide_launch",
            "max_scale": factor.clamp(1.05, 3.5),
            "range_slots": range.clamp(0.5, 8.0),
            "hover_ms": hover_ms,
            "elasticity": elasticity,
        })
    });
}

/// v2 → v3 adds explicit launch types. Items without `launch_type` default to
/// `auto`; `DockConfig::normalize` resolves them from their target/shortcut.
fn migrate_v2_to_v3(_object: &mut serde_json::Map<String, Value>) {}

#[cfg(test)]
mod tests {
    use super::*;
    use dock_core::{DockItem, ItemKind, LaunchType};

    fn temp_store(name: &str) -> (ConfigStore, PathBuf) {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        // Keep test files inside the workspace so restrictive build sandboxes
        // (which deny writes to %TEMP%) can still run the suite.
        let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-data");
        let dir = base.join(format!("glide-dock-test-{name}-{unique}"));
        let _ = fs::remove_dir_all(&dir);
        let store = ConfigStore::new(dir.join("config.json"));
        (store, dir)
    }

    #[test]
    fn missing_file_yields_defaults() {
        let (store, dir) = temp_store("missing");
        let outcome = store.load().expect("load");
        assert!(outcome.config.items.is_empty());
        assert!(!outcome.read_only);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn save_and_load_round_trip() {
        let (store, dir) = temp_store("roundtrip");
        let mut config = DockConfig::default();
        let id = config.allocate_id();
        config.items.push(DockItem {
            id,
            kind: ItemKind::Application,
            display_name: "Editor".into(),
            target_path: "C:\\Tools\\editor.exe".into(),
            ..DockItem::default()
        });
        store.save(&config).expect("save");

        let outcome = store.load().expect("load");
        assert_eq!(outcome.config.items.len(), 1);
        assert_eq!(outcome.config.items[0].display_name, "Editor");
        assert!(!outcome.migrated);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn corrupt_json_is_backed_up() {
        let (store, dir) = temp_store("corrupt");
        fs::create_dir_all(&dir).expect("mkdir");
        fs::write(store.path(), b"{ not json").expect("write");
        let outcome = store.load().expect("load");
        assert!(outcome.config.items.is_empty());
        assert!(outcome.backup.is_some());
        assert!(outcome.backup.as_ref().expect("backup").exists());
        assert!(
            fs::read_dir(&dir)
                .expect("dir")
                .flatten()
                .any(|entry| entry.file_name().to_string_lossy().contains("corrupt"))
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn future_version_is_read_only() {
        let (store, dir) = temp_store("future");
        fs::create_dir_all(&dir).expect("mkdir");
        fs::write(
            store.path(),
            br#"{ "version": 99, "items": [], "appearance": { "icon_size": 64.0 } }"#,
        )
        .expect("write");
        let outcome = store.load().expect("load");
        assert!(outcome.read_only);
        assert_eq!(outcome.config.appearance.icon_size, 64.0);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn non_integer_version_is_read_only() {
        let (store, dir) = temp_store("bad-version");
        fs::create_dir_all(&dir).expect("mkdir");
        fs::write(
            store.path(),
            br#"{ "version": "2", "items": [], "appearance": { "icon_size": 64.0 } }"#,
        )
        .expect("write");
        let outcome = store.load().expect("load");
        assert!(outcome.read_only);
        assert_eq!(outcome.config.appearance.icon_size, 64.0);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn v2_items_get_launch_types_on_load() {
        let (store, dir) = temp_store("launch-types");
        fs::create_dir_all(&dir).expect("mkdir");
        fs::write(
            store.path(),
            br#"{
                "version": 2,
                "items": [
                    { "id": 1, "kind": "application", "display_name": "Notepad",
                      "target_path": "C:\\Windows\\notepad.exe" },
                    { "id": 2, "kind": "application", "display_name": "Calculator",
                      "target_path": "Microsoft.WindowsCalculator_8wekyb3d8bbwe!App" }
                ]
            }"#,
        )
        .expect("write");
        let outcome = store.load().expect("load");
        assert!(outcome.migrated);
        assert_eq!(outcome.config.version, 3);
        assert_eq!(outcome.config.items[0].launch_type, LaunchType::Exe);
        assert_eq!(outcome.config.items[1].launch_type, LaunchType::PackagedApp);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn version_zero_migrates() {
        let (store, dir) = temp_store("migrate");
        fs::create_dir_all(&dir).expect("mkdir");
        fs::write(
            store.path(),
            br#"{
                "items": [
                    { "id": 7, "kind": "Application", "display_name": "App",
                      "target_path": "C:\\app.exe" }
                ]
            }"#,
        )
        .expect("write");
        let outcome = store.load().expect("load");
        assert!(outcome.migrated);
        assert_eq!(outcome.config.version, CONFIG_VERSION);
        assert_eq!(outcome.config.items.len(), 1);
        assert_eq!(outcome.config.items[0].kind, ItemKind::Application);
        assert_eq!(outcome.config.next_item_id, 8);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn partial_fields_do_not_fail() {
        let (store, dir) = temp_store("partial");
        fs::create_dir_all(&dir).expect("mkdir");
        fs::write(
            store.path(),
            br#"{ "version": 1, "items": [ { "id": 1 } ] }"#,
        )
        .expect("write");
        let outcome = store.load().expect("load");
        assert!(
            outcome.config.items.is_empty(),
            "pathless items are dropped"
        );
        assert!(!outcome.read_only);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn non_object_root_is_recovered() {
        let (store, dir) = temp_store("array-root");
        fs::create_dir_all(&dir).expect("mkdir");
        fs::write(store.path(), br#"[1, 2, 3]"#).expect("write");
        let outcome = store.load().expect("load");
        assert!(outcome.config.items.is_empty());
        assert!(outcome.backup.is_some());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn config_file_is_pretty_printed() {
        let (store, dir) = temp_store("pretty");
        store.save(&DockConfig::default()).expect("save");
        let text = fs::read_to_string(store.path()).expect("read");
        assert!(text.contains("\"version\": 3"));
        assert!(text.ends_with('\n'));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn v1_motion_fields_move_into_motion_section() {
        let (store, dir) = temp_store("migrate-motion");
        fs::create_dir_all(&dir).expect("mkdir");
        fs::write(
            store.path(),
            br#"{
                "version": 1,
                "behavior": {
                    "magnification": true,
                    "magnification_factor": 1.60,
                    "magnification_range": 3.2,
                    "stiffness": 760.0,
                    "damping": 0.58,
                    "launch_animation": true,
                    "show_tooltips": true,
                    "custom_flag": 7
                },
                "items": []
            }"#,
        )
        .expect("write");
        let outcome = store.load().expect("load");
        assert!(outcome.migrated);
        assert_eq!(outcome.config.version, 3);
        assert!((outcome.config.motion.max_scale - 1.60).abs() < 0.001);
        assert!((outcome.config.motion.range_slots - 3.2).abs() < 0.001);
        assert_eq!(outcome.config.motion.hover_preset.id(), "glide_zoom");
        assert_eq!(outcome.config.motion.launch_preset.id(), "glide_launch");
        assert!(outcome.config.motion.hover_ms >= 80);
        assert!(outcome.config.motion.elasticity > 0.9);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn v1_unknown_behavior_and_motion_fields_survive_migration() {
        let (store, dir) = temp_store("migrate-unknown");
        fs::create_dir_all(&dir).expect("mkdir");
        fs::write(
            store.path(),
            br#"{
                "version": 1,
                "behavior": { "custom_flag": 7 },
                "motion": { "hover_preset": "bounce", "future_field": "keep" },
                "items": []
            }"#,
        )
        .expect("write");
        let outcome = store.load().expect("load");
        assert!(outcome.migrated);
        assert_eq!(outcome.config.motion.hover_preset.id(), "bounce");
        assert_eq!(outcome.config.motion.launch_preset.id(), "glide_launch");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn unknown_motion_preset_falls_back_to_default() {
        let (store, dir) = temp_store("unknown-preset");
        fs::create_dir_all(&dir).expect("mkdir");
        fs::write(
            store.path(),
            br#"{ "version": 2, "motion": { "hover_preset": "disco", "launch_preset": "moon" }, "items": [] }"#,
        )
        .expect("write");
        let outcome = store.load().expect("load");
        assert_eq!(outcome.config.motion.hover_preset.id(), "glide_zoom");
        assert_eq!(outcome.config.motion.launch_preset.id(), "glide_launch");
        let _ = fs::remove_dir_all(dir);
    }
}
