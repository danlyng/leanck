use super::*;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
static NEXT: AtomicU64 = AtomicU64::new(0);
#[test]
fn discovered_project_update_policy_is_ignored_before_validation() {
    let dir = Scratch::new();
    let user = dir.file("user.toml", "[updates]\ncheck=false\n");
    for name in ["leanck.toml", ".leanck.toml"] {
        let project = dir.file(
            name,
            "[updates]\nautomatic='invalid type'\nunknown_key=true\n",
        );
        let loaded = ConfigArgs {
            config: Some(user.clone()),
            ..Default::default()
        }
        .load(&dir.0)
        .unwrap();
        assert!(!loaded.settings.updates.check);
        assert!(!loaded.settings.updates.automatic);
        assert_eq!(loaded.sources["updates.check"], user.display().to_string());
        assert_eq!(loaded.sources["updates.automatic"], "defaults");
        assert_eq!(loaded.warnings.len(), 1);
        assert_eq!(
            loaded.warnings[0].code,
            "ignored_project_machine_wide_section"
        );
        assert!(loaded.warnings[0].message.contains("[updates]"));
        assert!(
            loaded.warnings[0]
                .message
                .contains(&project.display().to_string())
        );
        // Explicitly selecting the same project file trusts and validates it.
        assert!(
            ConfigArgs {
                config: Some(project.clone()),
                ..Default::default()
            }
            .load(&dir.0)
            .is_err()
        );
        std::fs::write(&project, "[updates]\ncheck=false\nautomatic=true\n").unwrap();
        let explicit = ConfigArgs {
            config: Some(project.clone()),
            ..Default::default()
        }
        .load(&dir.0)
        .unwrap();
        assert!(explicit.settings.updates.automatic);
        assert!(explicit.warnings.is_empty());
        let overridden = ConfigArgs {
            config: Some(user.clone()),
            overrides: vec!["updates.automatic=true".into()],
            ..Default::default()
        }
        .load(&dir.0)
        .unwrap();
        assert!(overridden.settings.updates.automatic);
        assert_eq!(overridden.sources["updates.automatic"], "--set");
        std::fs::remove_file(project).unwrap();
    }
    assert!(load_files(&[], &["updates.check_interval_secs=0".into()]).is_err());
}
struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!(
            "leanck-config-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&p).unwrap();
        Self(p)
    }
    fn file(&self, name: &str, text: &str) -> PathBuf {
        let path = self.0.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
        path
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
#[test]
fn layers_merge_fields_replace_arrays_and_report_winners() {
    let dir = Scratch::new();
    let user = dir.file("user.toml", "[normalize]\nwidth=72\nheader=false\n[loogle]\nendpoints=['https://one.test/json', 'https://two.test/json']\n");
    let project = dir.file(
        "project.toml",
        "[normalize]\nwidth=88\n[loogle]\nendpoints=[]\n",
    );
    let loaded = load_files(
        &[(user.clone(), true), (project.clone(), true)],
        &["normalize.width=100".into(), "normalize.header=true".into()],
    )
    .unwrap();
    assert_eq!(loaded.settings.normalize.width, 100);
    assert!(loaded.settings.normalize.header);
    assert!(loaded.settings.loogle.endpoints.is_empty());
    assert_eq!(loaded.sources["normalize.width"], "--set");
    assert_eq!(
        loaded.sources["loogle.endpoints"],
        project.display().to_string()
    );
    assert_eq!(loaded.sources["daemon.max_inflight"], "defaults");
    assert_eq!(loaded.files, vec![user, project]);
}
#[test]
fn defaults_roundtrip_and_daemon_snapshot_preserve_settings() {
    let dir = Scratch::new();
    let mut settings = Settings::default();
    settings.daemon.max_inflight = 3;
    settings.daemon.idle_timeout_ms = 0;
    settings.cache.enabled = false;
    settings.instances.limit = 3;
    settings.normalize.rules.frontmatter.license_text = "A project license.".into();
    let path = dir.file("config.toml", &toml::to_string_pretty(&settings).unwrap());
    let loaded = load_files(&[(path, true)], &[]).unwrap();
    assert_eq!(
        serde_json::to_value(&loaded.settings).unwrap(),
        serde_json::to_value(&settings).unwrap()
    );
    let child = load_files(&[], &settings.daemon_assignments()).unwrap();
    assert_eq!(child.settings.daemon.max_inflight, 3);
    assert_eq!(child.settings.daemon.idle_timeout_ms, 0);
    assert!(!child.settings.cache.enabled);
    assert_eq!(child.settings.instances.limit, 3);
    assert_eq!(child.settings.instances.on_limit, OnLimit::EvictIdle);
    let assignments = settings.daemon_assignments();
    assert!(assignments.contains(&"instances.limit=3".to_owned()));
    assert!(assignments.contains(&"instances.on_limit=\"evict_idle\"".to_owned()));
}
#[test]
fn instances_defaults_range_and_policy_names() {
    let defaults = Settings::default().instances;
    assert_eq!(defaults.limit, 8);
    assert_eq!(defaults.on_limit, OnLimit::EvictIdle);
    for assignment in [
        "instances.limit=4097",
        "instances.limit=-1",
        "instances.on_limit='evict'",
        "instances.on_limit=true",
        "instances.unknown=1",
    ] {
        assert!(
            load_files(&[], &[assignment.into()]).is_err(),
            "accepted {assignment}"
        );
    }
    for (name, policy) in [
        ("warn", OnLimit::Warn),
        ("refuse", OnLimit::Refuse),
        ("evict_idle", OnLimit::EvictIdle),
    ] {
        let loaded = load_files(
            &[],
            &[
                format!("instances.on_limit='{name}'"),
                "instances.limit=4096".into(),
            ],
        )
        .unwrap();
        assert_eq!(loaded.settings.instances.on_limit, policy);
        assert_eq!(policy.as_str(), name);
        assert_eq!(loaded.settings.instances.limit, 4096);
    }
    assert_eq!(
        load_files(&[], &["instances.limit=0".into()])
            .unwrap()
            .settings
            .instances
            .limit,
        0
    );
}
#[test]
fn project_configuration_ignores_instances_and_records_a_warning() {
    let dir = Scratch::new();
    dir.file("project/lakefile.toml", "");
    let baseline = ConfigArgs::default().load(&dir.0.join("project")).unwrap();
    let project = dir.file(
        "project/leanck.toml",
        "[normalize]\nwidth=90\n[instances]\nlimit = 2\n",
    );
    let loaded = ConfigArgs::default().load(&dir.0.join("project")).unwrap();
    assert_eq!(
        loaded.settings.instances.limit,
        baseline.settings.instances.limit
    );
    assert_eq!(
        loaded.sources["instances.limit"],
        baseline.sources["instances.limit"]
    );
    assert_eq!(loaded.settings.normalize.width, 90);
    assert_eq!(loaded.warnings.len(), 1);
    let warning = &loaded.warnings[0];
    assert_eq!(warning.code, "ignored_project_machine_wide_section");
    assert!(warning.message.contains("leanck/config.toml"));
    assert!(warning.message.contains(&project.display().to_string()));
    // An explicit layer that is the project file itself is accepted.
    let loaded = ConfigArgs {
        config: Some(project.clone()),
        ..Default::default()
    }
    .load(&dir.0.join("project"))
    .unwrap();
    assert_eq!(loaded.settings.instances.limit, 2);
    assert!(loaded.warnings.is_empty());
    // The entire ignored section is removed before validation, including typos/types.
    std::fs::write(&project, "[instances]\nlimit='invalid'\nunknown=true\n").unwrap();
    assert_eq!(
        ConfigArgs::default()
            .load(&dir.0.join("project"))
            .unwrap()
            .warnings
            .len(),
        1
    );
    // Other project settings are still validated.
    std::fs::write(&project, "[instances]\nlimit=2\n[normalize]\nwidht=90\n").unwrap();
    assert!(ConfigArgs::default().load(&dir.0.join("project")).is_err());
    // Other explicit layers and overrides may set [instances].
    std::fs::write(&project, "[normalize]\nwidth = 90\n").unwrap();
    let explicit = dir.file(
        "explicit.toml",
        "[instances]\nlimit = 5\non_limit = 'warn'\n",
    );
    let loaded = ConfigArgs {
        config: Some(explicit),
        overrides: vec!["instances.limit=6".into()],
        ..Default::default()
    }
    .load(&dir.0.join("project"))
    .unwrap();
    assert_eq!(loaded.settings.instances.limit, 6);
    assert_eq!(loaded.settings.instances.on_limit, OnLimit::Warn);
    assert_eq!(loaded.settings.normalize.width, 90);
}
#[test]
fn invalid_keys_types_ranges_urls_and_globs_fail() {
    let dir = Scratch::new();
    for text in [
        "version=2",
        "[normalise]\nwidth=80",
        "[normalize]\nwidht=80",
        "[normalize]\nwidth=0",
        "[normalize]\nspelling='german'",
        "[normalize]\nexclude=['[']",
        "[loogle]\nendpoints=['file:///etc/hosts']",
        "[loogle]\nendpoints=['https://user:secret@example.test/json']",
        "[loogle]\nendpoints=['https://example.test/json?key=x']",
        "[client]\nstartup_timeout_ms=0",
        "[daemon]\nmax_inflight=0",
        "[build]\nmax_output_bytes=-1",
        "[normalize]\nheader='false'",
    ] {
        let path = dir.file("bad.toml", text);
        assert!(load_files(&[(path, true)], &[]).is_err(), "accepted {text}");
    }
    for assignment in [
        "normalize.widht=1",
        "normalize.width='80'",
        "normalize.spelling=american",
        "=1",
        "normalize..width=2",
        "normalize.width=3\nsecond=2",
    ] {
        assert!(
            load_files(&[], &[assignment.into()]).is_err(),
            "accepted {assignment}"
        );
    }
}
#[test]
fn missing_optional_files_are_allowed_but_required_and_unreadable_are_errors() {
    let dir = Scratch::new();
    assert!(load_files(&[(dir.0.join("missing"), false)], &[]).is_ok());
    assert!(load_files(&[(dir.0.join("missing"), true)], &[]).is_err());
    assert!(load_files(&[(dir.0.clone(), false)], &[]).is_err());
}
#[test]
fn discovery_is_nearest_and_stops_at_project_or_git_boundary() {
    let dir = Scratch::new();
    let outer = dir.file("leanck.toml", "");
    let marker = dir.file("project/lakefile.toml", "");
    let file = dir.file("project/src/Main.lean", "");
    assert_eq!(project_config_path(&file).unwrap(), None);
    let project = dir.file("project/leanck.toml", "");
    assert_eq!(project_config_path(&file).unwrap(), Some(project));
    let nearest = dir.file("project/src/leanck.toml", "");
    assert_eq!(project_config_path(&file).unwrap(), Some(nearest));
    assert!(marker.exists() && outer.exists());
}
#[test]
fn hidden_project_config_obeys_nearest_boundary_and_filename_precedence() {
    for marker in ["lakefile.lean", "lakefile.toml", "lean-toolchain", ".git"] {
        let dir = Scratch::new();
        dir.file(".leanck.toml", "");
        dir.file(&format!("project/{marker}"), "");
        let file = dir.file("project/src/Main.lean", "");
        assert_eq!(project_config_path(&file).unwrap(), None);
        let hidden = dir.file("project/.leanck.toml", "");
        assert_eq!(project_config_path(&file).unwrap(), Some(hidden.clone()));
        let visible = dir.file("project/leanck.toml", "");
        assert_eq!(project_config_path(&file).unwrap(), Some(visible));
        let nearest = dir.file("project/src/.leanck.toml", "");
        assert_eq!(project_config_path(&file).unwrap(), Some(nearest.clone()));
        assert_eq!(
            project_config_path(&dir.0.join("project/src/New.lean")).unwrap(),
            Some(nearest)
        );
        // Hidden configs ignore the same machine-wide settings.
        std::fs::remove_file(dir.0.join("project/leanck.toml")).unwrap();
        std::fs::write(&hidden, "[instances]\nlimit=2\n").unwrap();
        let loaded = ConfigArgs::default().load(&dir.0.join("project")).unwrap();
        assert_eq!(loaded.warnings.len(), 1);
        assert_eq!(
            loaded.warnings[0].code,
            "ignored_project_machine_wide_section"
        );
        let loaded = ConfigArgs {
            config: Some(hidden),
            ..Default::default()
        }
        .load(&dir.0.join("project"))
        .unwrap();
        assert_eq!(loaded.settings.instances.limit, 2);
    }
}
#[test]
fn paths_are_relative_to_the_file_that_defines_them() {
    let dir = Scratch::new();
    let path = dir.file("sub/config.toml", "[client]\ndaemon_bin='bin/leanckd'");
    let loaded = load_files(&[(path, true)], &[]).unwrap();
    assert_eq!(
        loaded.settings.client.daemon_bin,
        Some(dir.0.join("sub/bin/leanckd"))
    );
}
#[test]
fn no_config_still_accepts_typed_overrides() {
    let dir = Scratch::new();
    dir.file("leanck.toml", "not valid toml");
    let loaded = ConfigArgs {
        no_config: true,
        overrides: vec!["normalize.width=91".into()],
        ..Default::default()
    }
    .load(&dir.0)
    .unwrap();
    assert!(loaded.files.is_empty());
    assert_eq!(loaded.settings.normalize.width, 91);
}

#[test]
fn documented_template_matches_builtin_defaults() {
    let documented: Settings =
        toml::from_str(include_str!("../../../docs/leanck.example.toml")).unwrap();
    assert_eq!(
        serde_json::to_value(documented).unwrap(),
        serde_json::to_value(Settings::default()).unwrap()
    );
}

#[test]
fn replacing_a_section_reports_defaults_for_omitted_fields() {
    let loaded = load_files(
        &[],
        &[
            "normalize.header=false".into(),
            "normalize={width=80}".into(),
        ],
    )
    .unwrap();
    assert!(loaded.settings.normalize.header);
    assert_eq!(loaded.sources["normalize.header"], "defaults");
    assert_eq!(loaded.sources["normalize.width"], "--set");
}

#[test]
fn rich_policy_values_are_validated() {
    for assignment in [
        "normalize.rules.headings.level=7",
        "normalize.rules.frontmatter.order=['header','header']",
        "normalize.rules.frontmatter.order=['docstring','header']",
        "normalize.rules.frontmatter.order=['imports','module']",
        "normalize.rules.frontmatter.order=['section','module']",
        "normalize.rules.frontmatter.order=['namespace','module']",
        "normalize.rules.frontmatter.order=['namespace','imports']",
        "normalize.rules.frontmatter.order=['section','imports']",
        "normalize.rules.frontmatter.order=['unknown']",
        "normalize.rules.frontmatter.header_template='/-! bad -/'",
        "normalize.rules.frontmatter.header_template='/- {unknown} -/'",
        "normalize.rules.frontmatter.license_text='bad -/ code'",
        "normalize.rules.attributes.placement='ignored'",
        "normalize.rules.whitespace.max_blank_lines=255",
    ] {
        assert!(
            load_files(&[], &[assignment.into()]).is_err(),
            "accepted {assignment}"
        );
    }
    let dir = Scratch::new();
    let path = dir.file(
        "settings/config.toml",
        "[normalize.rules.spelling]\nfiles=['dict.json']\n",
    );
    let config = load_files(&[(path, true)], &[]).unwrap();
    assert_eq!(
        config.settings.normalize.rules.spelling.files,
        vec![dir.0.join("settings/dict.json")]
    );
}

#[test]
fn partial_and_empty_frontmatter_orders_are_valid() {
    for assignment in [
        "normalize.rules.frontmatter.order=[]",
        "normalize.rules.frontmatter.order=['header']",
        "normalize.rules.frontmatter.order=['docstring','section']",
        "normalize.rules.frontmatter.order=['module','imports','docstring']",
    ] {
        load_files(&[], &[assignment.into()]).unwrap_or_else(|e| panic!("{assignment}: {e}"));
    }
    for key in ["section_relocation", "namespace_relocation"] {
        assert!(load_files(&[], &[format!("normalize.{key}=false")]).is_err());
    }
}

#[test]
fn off_is_the_disabled_value_for_every_normalization_policy() {
    for key in [
        "normalize.spelling",
        "normalize.rules.headings.case",
        "normalize.rules.attributes.placement",
        "normalize.rules.attributes.overrides.simp",
        "normalize.rules.imports.order",
        "normalize.rules.comments.module",
        "normalize.rules.comments.declaration",
        "normalize.rules.comments.section",
        "normalize.rules.whitespace.line_endings",
        "normalize.rules.whitespace.final_newline",
    ] {
        let loaded = load_files(&[], &[format!("{key}='off'")]).unwrap();
        let value = serde_json::to_value(&loaded.settings).unwrap();
        let pointer = format!("/{}", key.replace('.', "/"));
        assert_eq!(value.pointer(&pointer).unwrap(), "off", "{key}");
        let serialized = toml::to_string_pretty(&loaded.settings).unwrap();
        let roundtrip: Settings = toml::from_str(&serialized).unwrap();
        assert_eq!(serde_json::to_value(roundtrip).unwrap(), value, "{key}");
        assert!(
            load_files(&[], &[format!("{key}='preserve'")]).is_err(),
            "obsolete value accepted for {key}"
        );
    }
}

#[test]
fn configuration_warnings_identify_inactive_or_ignored_policies_without_rejecting_them() {
    assert!(Settings::default().warnings().is_empty());
    for (assignments, code) in [
        (
            vec!["normalize.rules.frontmatter.blank_lines=3"],
            "frontmatter_blank_line_cap",
        ),
        (
            vec!["normalize.rules.headings.level=3"],
            "inactive_heading_rules",
        ),
        (
            vec![
                "normalize.rules.frontmatter.license_text='custom license'",
                "normalize.rules.frontmatter.header_template='/- {authors} -/'",
            ],
            "unused_license_text",
        ),
        (
            vec![
                "normalize.headings=true",
                "normalize.rules.headings.level=3",
                "normalize.rules.headings.template='/-! ## {title} -/'",
            ],
            "unused_heading_level",
        ),
        (
            vec!["normalize.rules.spelling.use_builtin=false"],
            "empty_spelling_dictionary",
        ),
        (
            vec![
                "normalize.spelling='off'",
                "normalize.rules.spelling.files=['missing.json']",
            ],
            "inactive_custom_dictionaries",
        ),
        (
            vec![
                "normalize.spelling='off'",
                "normalize.rules.spelling.replacements={colour='hue'}",
            ],
            "inactive_custom_dictionaries",
        ),
        (
            vec!["client.transport_timeout_ms=100"],
            "short_transport_timeout",
        ),
    ] {
        let loaded = load_files(
            &[],
            &assignments.iter().map(|s| (*s).into()).collect::<Vec<_>>(),
        )
        .unwrap();
        let warnings = loaded.settings.warnings();
        assert!(
            warnings.iter().any(|w| w.code == code),
            "{assignments:?}: {warnings:?}"
        );
        assert!(warnings.iter().all(|w| w.line.is_none()));
        if code == "inactive_custom_dictionaries" {
            assert!(
                warnings
                    .iter()
                    .any(|w| w.message.contains("use_builtin=false"))
            );
        }
    }
}

#[test]
fn inactive_or_default_policy_details_do_not_generate_misleading_warnings() {
    for assignments in [
        vec!["normalize.rules.headings.level=2"],
        vec![
            "normalize.headings=true",
            "normalize.rules.headings.level=3",
        ],
        vec!["normalize.rules.frontmatter.license_text='custom license'"],
        vec![
            "normalize.rules.spelling.use_builtin=false",
            "normalize.rules.spelling.replacements={colour='hue'}",
        ],
        vec![
            "normalize.spelling='off'",
            "normalize.rules.spelling.use_builtin=false",
        ],
        vec!["client.transport_timeout_ms=300000"],
        vec!["client.transport_timeout_ms=100", "client.timeout_ms=50"],
        vec![
            "normalize.rules.frontmatter.blank_lines=3",
            "normalize.frontmatter=false",
        ],
    ] {
        let loaded = load_files(
            &[],
            &assignments.iter().map(|s| (*s).into()).collect::<Vec<_>>(),
        )
        .unwrap();
        assert!(
            loaded.settings.warnings().is_empty(),
            "{assignments:?}: {:?}",
            loaded.settings.warnings()
        );
    }
}

#[test]
fn author_exclusions_are_inspectable_replaceable_and_validated() {
    let defaults = Settings::default()
        .normalize
        .rules
        .frontmatter
        .ignore_authors;
    for email in [
        "noreply@anthropic.com",
        "noreply@openai.com",
        "cursoragent@cursor.com",
        "aider@aider.chat",
    ] {
        assert!(defaults.iter().any(|entry| entry == email));
    }
    let loaded = load_files(
        &[],
        &["normalize.rules.frontmatter.ignore_authors=[]".into()],
    )
    .unwrap();
    assert!(
        loaded
            .settings
            .normalize
            .rules
            .frontmatter
            .ignore_authors
            .is_empty()
    );
    assert_eq!(
        loaded.sources["normalize.rules.frontmatter.ignore_authors"],
        "--set"
    );
    let loaded = load_files(
        &[],
        &["normalize.rules.frontmatter.ignore_authors=['Project Bot <bot@example.test>']".into()],
    )
    .unwrap();
    assert_eq!(
        loaded.settings.normalize.rules.frontmatter.ignore_authors,
        ["Project Bot <bot@example.test>"]
    );
    for value in [
        "['']",
        "['   ']",
        "[\"Agent\\nInjected\"]",
        "[42]",
        "'Agent'",
    ] {
        assert!(
            load_files(
                &[],
                &[format!(
                    "normalize.rules.frontmatter.ignore_authors={value}"
                )]
            )
            .is_err(),
            "{value}"
        );
    }
}

#[test]
fn author_maps_merge_by_name_and_allow_explicit_replacement() {
    let dir = Scratch::new();
    let user = dir.file(
        "user.toml",
        "[normalize.rules.frontmatter.author_map]\n'Alice Alias'='Alice'\nBob='Robert'\n",
    );
    let project = dir.file(
        "project.toml",
        "[normalize.rules.frontmatter.author_map]\n'Alice Alias'='Alice Example'\n",
    );
    let loaded = load_files(&[(user.clone(), true), (project.clone(), true)], &[]).unwrap();
    let map = &loaded.settings.normalize.rules.frontmatter.author_map;
    assert_eq!(map["Alice Alias"], "Alice Example");
    assert_eq!(map["Bob"], "Robert");
    assert_eq!(
        loaded.sources["normalize.rules.frontmatter.author_map.Alice Alias"],
        project.display().to_string()
    );
    assert_eq!(
        loaded.sources["normalize.rules.frontmatter.author_map.Bob"],
        user.display().to_string()
    );
    let loaded = load_files(
        &[(user, true)],
        &["normalize.rules.frontmatter.author_map={}".into()],
    )
    .unwrap();
    assert!(
        loaded
            .settings
            .normalize
            .rules
            .frontmatter
            .author_map
            .is_empty()
    );
    let loaded = load_files(
        &[],
        &["normalize.rules.frontmatter.author_map={'Git Name'='Display Name'}".into()],
    )
    .unwrap();
    assert_eq!(
        loaded.settings.normalize.rules.frontmatter.author_map["Git Name"],
        "Display Name"
    );
    assert_eq!(
        loaded.sources["normalize.rules.frontmatter.author_map.Git Name"],
        "--set"
    );
    for value in [
        "{'Name'=''}",
        "{'Name'='   '}",
        "{''='Name'}",
        "{'   '='Name'}",
        "{'Name'='unsafe -/ text'}",
        "{'Name'='unsafe /- text'}",
        "{Name=\"line\\nbreak\"}",
        "{\"line\\nbreak\"='Name'}",
        "{Name=42}",
        "[]",
        "'Name'",
    ] {
        assert!(
            load_files(
                &[],
                &[format!("normalize.rules.frontmatter.author_map={value}")]
            )
            .is_err(),
            "accepted {value}"
        );
    }
}

#[test]
fn invalid_existing_header_policy_defaults_to_warn_and_accepts_only_named_actions() {
    assert_eq!(
        Settings::default()
            .normalize
            .rules
            .frontmatter
            .invalid_existing,
        InvalidExistingHeader::Warn
    );
    for (name, policy) in [
        ("replace", InvalidExistingHeader::Replace),
        ("warn", InvalidExistingHeader::Warn),
        ("ignore", InvalidExistingHeader::Ignore),
    ] {
        let loaded = load_files(
            &[],
            &[format!(
                "normalize.rules.frontmatter.invalid_existing='{name}'"
            )],
        )
        .unwrap();
        assert_eq!(
            loaded.settings.normalize.rules.frontmatter.invalid_existing,
            policy
        );
        let text = toml::to_string_pretty(&loaded.settings).unwrap();
        let restored: Settings = toml::from_str(&text).unwrap();
        assert_eq!(
            restored.normalize.rules.frontmatter.invalid_existing,
            policy
        );
    }
    for value in ["false", "true", "'off'", "'invalid'"] {
        assert!(
            load_files(
                &[],
                &[format!(
                    "normalize.rules.frontmatter.invalid_existing={value}"
                )]
            )
            .is_err()
        );
    }
}
