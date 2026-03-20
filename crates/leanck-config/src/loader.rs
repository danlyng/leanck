use crate::*;
use anyhow::{Context, Result, ensure};
use serde::Serialize;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

/// Configuration file selection and overrides, independent of argument parsing.
#[derive(Debug, Clone, Default)]
pub struct ConfigArgs {
    /// Overlay this TOML file after user/project configuration (or LEANCK_CONFIG).
    pub config: Option<PathBuf>,
    /// Ignore configuration files, including LEANCK_CONFIG.
    pub no_config: bool,
    /// Override any setting using a dotted TOML assignment; repeatable.
    pub overrides: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LoadedConfig {
    pub settings: Settings,
    pub files: Vec<PathBuf>,
    /// File-loading diagnostics, displayed only by health/doctor checks.
    pub warnings: Vec<Warning>,
    /// Dotted setting -> winning source (defaults, filename, --set).
    pub sources: BTreeMap<String, String>,
}

impl ConfigArgs {
    pub fn load(&self, start: &Path) -> Result<LoadedConfig> {
        let user = user_config_path();
        let explicit = self.config.clone().or_else(|| {
            (!self.no_config)
                .then(|| std::env::var_os("LEANCK_CONFIG").map(PathBuf::from))
                .flatten()
        });
        let mut layers = Vec::new();
        if !self.no_config {
            if let Some(user) = user {
                layers.push(Layer::new(user, false));
            }
            if let Some(project) = project_config_path(start)? {
                // The same file is accepted when it is also the explicit layer.
                let discovered = !explicit
                    .as_deref()
                    .is_some_and(|explicit| same_file(&project, explicit));
                layers.push(Layer {
                    discovered_project: discovered,
                    ..Layer::new(project, true)
                });
            }
            if let Some(explicit) = explicit {
                layers.push(Layer::new(explicit, true));
            }
        }
        let mut loaded = load_layers(&layers, &[])?;
        for assignment in &self.overrides {
            loaded.apply_assignment(assignment)?;
        }
        loaded.settings.validate()?;
        Ok(loaded)
    }
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => std::path::absolute(a).ok() == std::path::absolute(b).ok(),
    }
}

/// One configuration file to merge.
struct Layer {
    path: PathBuf,
    required: bool,
    /// A discovered project configuration, whose machine-wide sections are ignored.
    discovered_project: bool,
}

impl Layer {
    fn new(path: PathBuf, required: bool) -> Self {
        Self {
            path,
            required,
            discovered_project: false,
        }
    }
}

/// Ignore machine-wide settings before validating or merging a project layer.
fn ignore_project_machine_wide_sections(
    project: &Path,
    patch: &mut toml::Value,
    machine_wide_sections: &[&str],
) -> Vec<Warning> {
    let Some(table) = patch.as_table_mut() else {
        return Vec::new();
    };
    let user = user_config_path().map_or_else(
        || "~/.config/leanck/config.toml".to_owned(),
        |path| path.display().to_string(),
    );
    machine_wide_sections
        .iter()
        .filter_map(|section| {
            table.remove(*section).map(|_| {
                Warning::new(
                    "ignored_project_machine_wide_section",
                    format!(
                        "[{section}] in project configuration {} is ignored because it is machine-wide; set it in the user configuration {user} (or with --config/LEANCK_CONFIG/--set)",
                        project.display()
                    ),
                )
            })
        })
        .collect()
}

pub fn user_config_path() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".config")))
        .map(|p| p.join("leanck/config.toml"))
}

const PROJECT_CONFIG_NAMES: [&str; 2] = ["leanck.toml", ".leanck.toml"];

/// Nearest leanck.toml or .leanck.toml, bounded by the first Lake project or VCS root.
/// When both exist in the same directory, leanck.toml takes precedence.
/// For a file (including a not-yet-existing file), discovery starts at its parent.
pub fn project_config_path(start: &Path) -> Result<Option<PathBuf>> {
    let start = std::path::absolute(start)?;
    let dir = if start.is_dir() {
        start.as_path()
    } else {
        start
            .parent()
            .context("configuration search has no parent")?
    };
    for dir in dir.ancestors() {
        for name in PROJECT_CONFIG_NAMES {
            let file = dir.join(name);
            if file
                .try_exists()
                .with_context(|| format!("cannot inspect {}", file.display()))?
            {
                return Ok(Some(file));
            }
        }
        if ["lakefile.lean", "lakefile.toml", "lean-toolchain", ".git"]
            .iter()
            .any(|p| dir.join(p).exists())
        {
            break;
        }
    }
    Ok(None)
}

/// Deterministic loader for embedded clients and tests. Optional files may be
/// missing; every other I/O error, unknown key, and invalid value is fatal.
pub fn load_files(files: &[(PathBuf, bool)], overrides: &[String]) -> Result<LoadedConfig> {
    let layers: Vec<Layer> = files
        .iter()
        .map(|(path, required)| Layer::new(path.clone(), *required))
        .collect();
    load_layers(&layers, overrides)
}

fn load_layers(layers: &[Layer], overrides: &[String]) -> Result<LoadedConfig> {
    let mut value = toml::Value::try_from(Settings::default())?;
    let mut sources = BTreeMap::new();
    record_sources(&value, "", "defaults", &mut sources);
    let mut loaded_files = Vec::new();
    let mut warnings = Vec::new();
    for layer in layers {
        let (path, required) = (&layer.path, layer.required);
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if !required && e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => {
                return Err(e)
                    .with_context(|| format!("cannot read configuration {}", path.display()));
            }
        };
        let mut patch: toml::Value =
            toml::from_str(&text).with_context(|| format!("invalid TOML in {}", path.display()))?;
        if layer.discovered_project {
            warnings.extend(ignore_project_machine_wide_sections(
                path,
                &mut patch,
                MACHINE_WIDE_SECTIONS,
            ));
        }
        // Validate each layer's shape before merging so an override cannot hide typos.
        let _: Settings = patch
            .clone()
            .try_into()
            .with_context(|| format!("invalid configuration {}", path.display()))?;
        if let Some(bin) = patch
            .get_mut("client")
            .and_then(|c| c.get_mut("daemon_bin"))
            && let Some(s) = bin.as_str()
        {
            let p = Path::new(s);
            if p.is_relative() {
                *bin = toml::Value::String(
                    std::path::absolute(path)?
                        .parent()
                        .unwrap()
                        .join(p)
                        .to_string_lossy()
                        .into_owned(),
                );
            }
        }
        if let Some(files) = patch
            .get_mut("normalize")
            .and_then(|n| n.get_mut("rules"))
            .and_then(|r| r.get_mut("spelling"))
            .and_then(|r| r.get_mut("files"))
            .and_then(toml::Value::as_array_mut)
        {
            let base = std::path::absolute(path)?.parent().unwrap().to_owned();
            for file in files {
                if let Some(name) = file.as_str()
                    && Path::new(name).is_relative()
                {
                    *file = toml::Value::String(base.join(name).to_string_lossy().into_owned());
                }
            }
        }
        record_sources(&patch, "", &path.display().to_string(), &mut sources);
        merge(&mut value, patch);
        loaded_files.push(std::path::absolute(path)?);
    }
    let settings: Settings = value.try_into().context("invalid merged configuration")?;
    let mut loaded = LoadedConfig {
        settings,
        files: loaded_files,
        warnings,
        sources,
    };
    for assignment in overrides {
        loaded.apply_assignment(assignment)?;
    }
    loaded.settings.validate()?;
    Ok(loaded)
}
impl LoadedConfig {
    fn apply_assignment(&mut self, assignment: &str) -> Result<()> {
        let (key, raw) = assignment
            .split_once('=')
            .context("--set expects KEY=VALUE (TOML syntax; quote strings)")?;
        let key = key.trim();
        ensure!(
            !key.is_empty()
                && key
                    .split('.')
                    .all(|p| !p.is_empty()
                        && p.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')),
            "invalid --set key {key:?}"
        );
        let mut parsed: toml::Table =
            toml::from_str(&format!("value = {raw}")).with_context(|| {
                format!("invalid TOML value in --set {key}; strings require quotes")
            })?;
        ensure!(parsed.len() == 1, "--set accepts one assignment at a time");
        self.apply_value(
            key,
            parsed.remove("value").context("missing --set value")?,
            "--set",
        )
    }
    fn apply_value(&mut self, key: &str, patch: toml::Value, source: &str) -> Result<()> {
        let mut value = toml::Value::try_from(&self.settings)?;
        let parts: Vec<_> = key.split('.').collect();
        let mut target = &mut value;
        for part in &parts[..parts.len() - 1] {
            target = target
                .get_mut(*part)
                .with_context(|| format!("unknown configuration section in {key}"))?;
        }
        let table = target
            .as_table_mut()
            .context("setting parent must be a table")?;
        table.insert(parts.last().unwrap().to_string(), patch.clone());
        self.settings = value
            .try_into()
            .with_context(|| format!("invalid setting {key}"))?;
        self.sources
            .retain(|existing, _| existing != key && !existing.starts_with(&format!("{key}.")));
        // Section replacement can restore omitted fields to defaults. Keep
        // provenance complete for those fields, not just the explicit patch.
        let resolved = toml::Value::try_from(&self.settings)?;
        let mut effective = &resolved;
        for part in &parts {
            effective = effective
                .get(*part)
                .context("resolved setting is missing")?;
        }
        record_sources(effective, key, "defaults", &mut self.sources);
        record_sources(&patch, key, source, &mut self.sources);
        Ok(())
    }
    pub fn to_toml(&self) -> Result<String> {
        Ok(toml::to_string_pretty(&self.settings)?)
    }
}
fn merge(base: &mut toml::Value, patch: toml::Value) {
    match (base, patch) {
        (toml::Value::Table(base), toml::Value::Table(patch)) => {
            for (key, value) in patch {
                if let Some(old) = base.get_mut(&key) {
                    merge(old, value);
                } else {
                    base.insert(key, value);
                }
            }
        }
        (base, patch) => *base = patch,
    }
}
fn record_sources(
    value: &toml::Value,
    prefix: &str,
    source: &str,
    sources: &mut BTreeMap<String, String>,
) {
    if let Some(table) = value.as_table() {
        for (key, value) in table {
            record_sources(
                value,
                &if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{prefix}.{key}")
                },
                source,
                sources,
            );
        }
    } else {
        sources.insert(prefix.into(), source.into());
    }
}

pub fn init_path(path: Option<&Path>, user: bool, workspace: Option<&Path>) -> Result<PathBuf> {
    if user {
        user_config_path().context("cannot locate user configuration; set HOME or XDG_CONFIG_HOME")
    } else if let Some(path) = path {
        Ok(path.to_owned())
    } else {
        let base = workspace.map(Path::to_owned).unwrap_or_default();
        let base = if base.is_file() {
            base.parent().context("workspace has no parent")?
        } else {
            &base
        };
        // Reuse the hidden project configuration if it already exists here.
        let hidden = base.join(".leanck.toml");
        Ok(if !base.join("leanck.toml").exists() && hidden.exists() {
            hidden
        } else {
            base.join("leanck.toml")
        })
    }
}

pub fn starter_config(defaults: &Settings) -> Result<String> {
    let normalize = toml::Value::try_from(&defaults.normalize)?;
    Ok(format!(
        "# Omitted settings inherit their values; uncomment settings to override them.\n\
         # Full example: https://github.com/danlyng/leanck/blob/HEAD/docs/leanck.example.toml\n\
         # Print all built-in defaults with: leanck config defaults\n\
         version = {version}\n\n\
         # [normalize]\n\
         # width = {width}\n\
         # spelling = {spelling} # \"american\", \"british\", or \"off\"\n\n\
         # [normalize.rules.frontmatter]\n\
         # header_template = {header}\n\
         # license_text = {license}\n",
        version = defaults.version,
        width = normalize["width"],
        spelling = normalize["spelling"],
        header = normalize["rules"]["frontmatter"]["header_template"]
            .to_string()
            .replace('\n', "\n# "),
        license = normalize["rules"]["frontmatter"]["license_text"]
            .to_string()
            .replace('\n', "\n# "),
    ))
}

impl Settings {
    /// Pass only daemon-owned settings to a detached child, frozen at launch.
    /// `[instances]` is included because auto-spawned daemons run with `--no-config`.
    pub fn daemon_assignments(&self) -> Vec<String> {
        let mut assignments = Vec::new();
        for (section, value) in [
            ("daemon", toml::Value::try_from(&self.daemon).unwrap()),
            ("cache", toml::Value::try_from(&self.cache).unwrap()),
            ("instances", toml::Value::try_from(&self.instances).unwrap()),
        ] {
            for (key, value) in value.as_table().unwrap() {
                assignments.push(format!("{section}.{key}={value}"));
            }
        }
        assignments
    }
}

#[cfg(test)]
mod scope_tests {
    use super::*;

    #[test]
    fn machine_wide_registry_contains_unique_schema_sections() {
        let defaults = toml::Value::try_from(Settings::default()).unwrap();
        let mut seen = std::collections::BTreeSet::new();
        for section in MACHINE_WIDE_SECTIONS {
            assert!(
                seen.insert(section),
                "duplicate scope registration: {section}"
            );
            assert!(
                defaults.get(*section).is_some_and(toml::Value::is_table),
                "machine-wide section is missing from the schema: {section}"
            );
        }
    }

    #[test]
    fn project_filter_obeys_the_registry_and_preserves_other_sections() {
        let mut patch = toml::from_str(
            "[instances]\nlimit='invalid'\n[daemon]\nunknown=true\n[normalize]\nwidth=83\n",
        )
        .unwrap();
        // Adding a second machine-wide section needs no filtering or warning changes.
        let registry = ["instances", "daemon", "absent"];
        let warnings = ignore_project_machine_wide_sections(
            Path::new("project/leanck.toml"),
            &mut patch,
            &registry,
        );
        assert_eq!(warnings.len(), 2);
        for (warning, section) in warnings.iter().zip(registry) {
            assert_eq!(warning.code, "ignored_project_machine_wide_section");
            assert!(warning.message.contains(&format!("[{section}]")));
            assert!(patch.get(section).is_none());
        }
        let settings: Settings = patch.clone().try_into().unwrap();
        assert_eq!(settings.normalize.width, NormalizeWidth::Finite(83));
        assert!(
            ignore_project_machine_wide_sections(
                Path::new("project/leanck.toml"),
                &mut patch,
                &registry,
            )
            .is_empty()
        );
    }
}
