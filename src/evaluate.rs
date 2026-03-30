//! libloot evaluation (sync). Called from MCP tools via `spawn_blocking`.

use anyhow::{anyhow, Result};
use libloot::metadata::{MessageContent, PluginMetadata};
use libloot::{libloot_revision, libloot_version, EvalMode, Game, GameType, MergeMode, Plugin};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// After this, `load_plugin_headers` is done for the full current load order (needed for LOOT conditions).
struct PreparedEval {
    game: Game,
    current: Vec<String>,
    masterlist_path_str: String,
    prelude_loaded: bool,
    userlist_loaded: bool,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "snake_case")]
pub struct EvalRequest {
    pub game_type: String,
    pub game_path: String,
    #[serde(default)]
    pub game_local_path: Option<String>,
    #[serde(default)]
    pub loot_data_path: Option<String>,
    #[serde(default)]
    pub loot_game_folder: Option<String>,
    #[serde(default)]
    pub masterlist_path: Option<String>,
    #[serde(default)]
    pub prelude_path: Option<String>,
    #[serde(default)]
    pub userlist_path: Option<String>,
    /// Directories scanned for .esp/.esm/.esl (non-recursive). Listed first win over later dirs and over `game/Data`.
    #[serde(default)]
    pub additional_data_paths: Option<Vec<String>>,
    /// MO2 `mods` folder; combined with `game_local_path`/modlist.txt to append each enabled mod's `Data` (or mod root) in MO2 priority order.
    #[serde(default)]
    pub mo2_mods_path: Option<String>,
    /// If true, fill `plugins` with evaluated masterlist/userlist YAML per plugin (large payload). Default false: only load order, messages, and sorting.
    #[serde(default)]
    pub include_plugin_metadata: bool,
    /// When `include_plugin_metadata` is true: `full` = `metadata_yaml` per plugin; `problems` = warn/error messages, incompatibilities, optional requirements/load_after (no YAML).
    #[serde(default)]
    pub plugin_metadata_content: PluginMetadataContent,
    /// Skip this many plugins from the start of `load_order_current` when filling `plugins` (stable order).
    #[serde(default)]
    pub plugin_metadata_offset: u32,
    /// Max plugins in `plugins` map; omit or `null` = all from offset to end.
    #[serde(default)]
    pub plugin_metadata_limit: Option<u32>,
    /// If true, add `master_header_issues`: TES4 masters missing from load order or loaded after the dependent plugin.
    #[serde(default)]
    pub include_master_header_issues: bool,
    /// In `problems` content mode, also emit `requirements` and `load_after` file lists (larger).
    #[serde(default)]
    pub plugin_problems_include_requirements_load_after: bool,
    /// Filter `general_messages` by minimum severity (`say` = all, `warn` = warn+error, `error` = error only).
    #[serde(default)]
    pub general_messages_min_severity: GeneralMessageMinSeverity,
    /// For `loot_load_order`: if true, always use libloadorder (slow with many MO2 dirs). If false, read `loadorder.txt` / `plugins.txt` from `game_local_path` when present.
    #[serde(default)]
    pub load_order_use_libloadorder: bool,
}

/// How per-plugin LOOT metadata is exposed when `include_plugin_metadata` is true.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginMetadataContent {
    #[default]
    Full,
    Problems,
}

/// Minimum severity included in `general_messages`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GeneralMessageMinSeverity {
    #[default]
    Say,
    Warn,
    Error,
}

#[derive(Debug, Serialize)]
pub struct EvalResponse {
    pub libloot_version: String,
    pub libloot_revision: String,
    pub masterlist_path: String,
    pub prelude_loaded: bool,
    pub userlist_loaded: bool,
    pub load_order_current: Vec<String>,
    pub load_order_suggested: Vec<String>,
    pub load_order_ambiguous: Option<bool>,
    pub general_messages: Vec<MsgOut>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub plugins: BTreeMap<String, PluginOut>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub master_header_issues: Vec<MasterHeaderIssueOut>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plugin_metadata_page: Option<PluginMetadataPageOut>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct PluginMetadataResponse {
    pub libloot_version: String,
    pub libloot_revision: String,
    pub masterlist_path: String,
    pub prelude_loaded: bool,
    pub userlist_loaded: bool,
    /// Evaluated YAML per requested plugin that was found in the load order.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub plugins: BTreeMap<String, PluginOut>,
    /// Requested names that are not in the current load order (or no header loaded).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub not_found: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Load order list: either from profile text files (fast) or libloadorder (slow but validates against disk).
#[derive(Debug, Serialize)]
pub struct LoadOrderReadResponse {
    pub load_order: Vec<String>,
    /// Set only when `source` is `libloadorder`.
    pub load_order_ambiguous: Option<bool>,
    pub plugin_count: usize,
    /// `loadorder_txt`, `plugins_txt`, or `libloadorder`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MsgOut {
    pub severity: String,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub condition: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct PluginOut {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub header_version: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub crc: Option<u32>,
    pub is_master: bool,
    pub is_light: bool,
    pub bash_tags_in_plugin_header: Vec<String>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub metadata_yaml: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata_problems: Option<PluginMetadataProblemsOut>,
}

#[derive(Debug, Serialize)]
pub struct PluginMetadataProblemsOut {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub messages: Vec<MsgOut>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub incompatibilities: Vec<FileRefOut>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub requirements: Vec<FileRefOut>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub load_after: Vec<FileRefOut>,
}

#[derive(Debug, Serialize)]
pub struct FileRefOut {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct MasterHeaderIssueOut {
    pub plugin: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub missing_masters: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub masters_after_plugin: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct PluginMetadataPageOut {
    pub total: usize,
    pub offset: usize,
    pub returned: usize,
    pub has_more: bool,
}

pub fn parse_game_type(s: &str) -> Result<GameType> {
    let t = s.trim();
    let t = match t {
        "Skyrim Special Edition" | "SkyrimSE" | "SSE" => GameType::SkyrimSE,
        "Skyrim" | "TES5" => GameType::Skyrim,
        "Skyrim VR" | "SkyrimVR" => GameType::SkyrimVR,
        "Fallout4" | "Fallout 4" => GameType::Fallout4,
        "Fallout4VR" | "Fallout 4 VR" => GameType::Fallout4VR,
        "FalloutNV" | "Fallout New Vegas" | "FNV" => GameType::FalloutNV,
        "Fallout3" | "Fallout 3" => GameType::Fallout3,
        "Morrowind" => GameType::Morrowind,
        "Oblivion" => GameType::Oblivion,
        "Oblivion Remastered" | "OblivionRemastered" => GameType::OblivionRemastered,
        "Starfield" => GameType::Starfield,
        "OpenMW" => GameType::OpenMW,
        _ => return Err(anyhow!("unknown game_type: {:?} (use e.g. SkyrimSE, Fallout4)", s)),
    };
    Ok(t)
}

fn default_loot_folder(gt: GameType) -> String {
    let s = match gt {
        GameType::SkyrimSE => "Skyrim Special Edition",
        GameType::Skyrim => "Skyrim",
        GameType::SkyrimVR => "Skyrim VR",
        GameType::Fallout4 => "Fallout4",
        GameType::Fallout4VR => "Fallout4 VR",
        GameType::FalloutNV => "FalloutNV",
        GameType::Fallout3 => "Fallout3",
        GameType::Morrowind => "Morrowind",
        GameType::Oblivion => "Oblivion",
        GameType::OblivionRemastered => "Oblivion Remastered",
        GameType::Starfield => "Starfield",
        GameType::OpenMW => "OpenMW",
        _ => return format!("{}", gt),
    };
    s.to_string()
}

/// LOOT v0.22+ stores per-game files under `<loot>/games/<Game Name>/`; older builds used `<loot>/<Game Name>/`.
fn resolve_game_loot_dir(loot_root: &Path, folder: &str) -> PathBuf {
    let via_games = loot_root.join("games").join(folder);
    let legacy = loot_root.join(folder);
    if via_games.join("masterlist.yaml").exists() {
        via_games
    } else if legacy.join("masterlist.yaml").exists() {
        legacy
    } else if loot_root.join("games").is_dir() {
        via_games
    } else {
        legacy
    }
}

/// Shared prelude lives in `<loot>/prelude/prelude.yaml` on newer LOOT layouts.
fn default_prelude_path(loot_root: &Path, game_dir: &Path) -> PathBuf {
    let root_prelude = loot_root.join("prelude").join("prelude.yaml");
    if root_prelude.exists() {
        root_prelude
    } else {
        game_dir.join("prelude.yaml")
    }
}

/// Matches libloot `data_path()` / libloadorder plugins directory layout.
fn plugins_data_root(game_type: GameType, game_path: &Path) -> PathBuf {
    match game_type {
        GameType::Morrowind => game_path.join("Data Files"),
        GameType::OpenMW => game_path.join("resources/vfs"),
        GameType::OblivionRemastered => game_path.join("OblivionRemastered/Content/Dev/ObvData/Data"),
        _ => game_path.join("Data"),
    }
}

fn first_existing_plugin_file(dir: &Path, plugin_name: &str) -> Option<PathBuf> {
    let p = dir.join(plugin_name);
    if p.exists() {
        return Some(p);
    }
    let ghost = format!("{}.ghost", plugin_name);
    let g = dir.join(ghost);
    if g.exists() {
        return Some(g);
    }
    None
}

/// libloot resolves relative plugin paths only under main `Data/`; with MO2, files live under
/// `additional_data_paths`. Pass absolute paths here so validation and loading match libloadorder.
fn absolute_plugin_paths_for_libloot(
    game: &Game,
    game_type: GameType,
    game_path: &Path,
    load_order_names: &[String],
) -> Vec<PathBuf> {
    if game.additional_data_paths().is_empty() {
        return load_order_names.iter().map(PathBuf::from).collect();
    }

    let main = plugins_data_root(game_type, game_path);
    let additional = game.additional_data_paths();

    load_order_names
        .iter()
        .map(|name| {
            if matches!(game_type, GameType::Starfield) {
                if first_existing_plugin_file(&main, name).is_none() {
                    return main.join(name);
                }
            }

            if matches!(game_type, GameType::OpenMW) {
                return additional
                    .iter()
                    .rev()
                    .find_map(|d| first_existing_plugin_file(d, name))
                    .unwrap_or_else(|| main.join(name));
            }

            for dir in additional.iter() {
                if let Some(p) = first_existing_plugin_file(dir, name) {
                    return p;
                }
            }
            first_existing_plugin_file(&main, name).unwrap_or_else(|| main.join(name))
        })
        .collect()
}

fn parse_mo2_enabled_mods(modlist: &Path) -> Result<Vec<String>> {
    let text = std::fs::read_to_string(modlist)
        .map_err(|e| anyhow!("read {}: {}", modlist.display(), e))?;
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(rest) = line.strip_prefix('+') {
            let name = rest.trim();
            if !name.is_empty() {
                out.push(name.to_string());
            }
        }
    }
    Ok(out)
}

/// One plugin-search root per enabled MO2 mod, highest MO2 priority first (for libloadorder `pick_plugin_path`).
fn mo2_additional_plugin_roots(mods_root: &Path, profile_dir: &Path) -> Result<Vec<PathBuf>> {
    let modlist = profile_dir.join("modlist.txt");
    if !modlist.is_file() {
        return Err(anyhow!(
            "modlist.txt not found under profile {} (required when mo2_mods_path is set)",
            profile_dir.display()
        ));
    }
    let enabled = parse_mo2_enabled_mods(&modlist)?;
    let mut roots = Vec::new();
    for name in enabled.into_iter().rev() {
        let mdir = mods_root.join(&name);
        let data = mdir.join("Data");
        let root = if data.is_dir() { data } else { mdir };
        if root.is_dir() {
            roots.push(root);
        }
    }
    Ok(roots)
}

fn collect_additional_data_paths(req: &EvalRequest) -> Result<Vec<PathBuf>> {
    let mut extra: Vec<PathBuf> = Vec::new();
    if let Some(ref paths) = req.additional_data_paths {
        for s in paths {
            let s = s.trim();
            if !s.is_empty() {
                extra.push(PathBuf::from(s));
            }
        }
    }
    if let Some(ref mroot) = req.mo2_mods_path {
        let mroot = mroot.trim();
        if !mroot.is_empty() {
            let local = req
                .game_local_path
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| {
                    anyhow!("mo2_mods_path requires game_local_path (MO2 profile directory containing modlist.txt)")
                })?;
            let mo2_roots = mo2_additional_plugin_roots(Path::new(mroot), Path::new(local))?;
            extra.extend(mo2_roots);
        }
    }
    Ok(extra)
}

fn default_loot_data_path() -> Result<PathBuf> {
    if let Ok(p) = std::env::var("LOOT_DATA_PATH") {
        let p = p.trim();
        if !p.is_empty() {
            return Ok(PathBuf::from(p));
        }
    }
    if cfg!(windows) {
        let base = std::env::var("LOCALAPPDATA").unwrap_or_else(|_| ".".into());
        Ok(PathBuf::from(base).join("LOOT"))
    } else {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
        let xdg =
            std::env::var("XDG_DATA_HOME").unwrap_or_else(|_| format!("{}/.local/share", home));
        Ok(PathBuf::from(xdg).join("LOOT"))
    }
}

fn message_text(m: &libloot::metadata::Message) -> String {
    m.content()
        .iter()
        .map(|c: &MessageContent| c.text().to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

fn message_severity(t: libloot::metadata::MessageType) -> &'static str {
    use libloot::metadata::MessageType;
    match t {
        MessageType::Say => "say",
        MessageType::Warn => "warn",
        MessageType::Error => "error",
    }
}

fn severity_str_rank(s: &str) -> u8 {
    match s {
        "warn" => 1,
        "error" => 2,
        _ => 0,
    }
}

fn min_severity_rank(m: GeneralMessageMinSeverity) -> u8 {
    match m {
        GeneralMessageMinSeverity::Say => 0,
        GeneralMessageMinSeverity::Warn => 1,
        GeneralMessageMinSeverity::Error => 2,
    }
}

fn filter_general_messages(msgs: Vec<MsgOut>, min: GeneralMessageMinSeverity) -> Vec<MsgOut> {
    let r = min_severity_rank(min);
    msgs
        .into_iter()
        .filter(|m| severity_str_rank(&m.severity) >= r)
        .collect()
}

fn load_order_index_case_insensitive(current: &[String]) -> HashMap<String, usize> {
    let mut m = HashMap::new();
    for (i, name) in current.iter().enumerate() {
        m.entry(name.to_ascii_lowercase()).or_insert(i);
    }
    m
}

fn collect_master_header_issues(game: &Game, current: &[String]) -> Vec<MasterHeaderIssueOut> {
    let idx = load_order_index_case_insensitive(current);
    let mut out = Vec::new();
    for name in current {
        let Some(p) = game.plugin(name) else {
            continue;
        };
        let Ok(masters) = p.masters() else {
            continue;
        };
        if masters.is_empty() {
            continue;
        }
        let Some(&self_i) = idx.get(&name.to_ascii_lowercase()) else {
            continue;
        };
        let mut missing_masters = Vec::new();
        let mut masters_after_plugin = Vec::new();
        for m in masters {
            match idx.get(&m.to_ascii_lowercase()) {
                None => missing_masters.push(m),
                Some(&mi) if mi < self_i => {}
                Some(_) => masters_after_plugin.push(m),
            }
        }
        if missing_masters.is_empty() && masters_after_plugin.is_empty() {
            continue;
        }
        out.push(MasterHeaderIssueOut {
            plugin: name.clone(),
            missing_masters,
            masters_after_plugin,
        });
    }
    out
}

fn file_to_out(f: &libloot::metadata::File) -> FileRefOut {
    FileRefOut {
        name: f.name().as_str().to_string(),
        display_name: f.display_name().map(|s| s.to_string()),
    }
}

fn problems_from_plugin_metadata(
    meta: &PluginMetadata,
    include_req_la: bool,
) -> PluginMetadataProblemsOut {
    use libloot::metadata::MessageType;
    let messages: Vec<MsgOut> = meta
        .messages()
        .iter()
        .filter(|m| matches!(m.message_type(), MessageType::Warn | MessageType::Error))
        .map(|m| MsgOut {
            severity: message_severity(m.message_type()).to_string(),
            text: message_text(m),
            condition: m.condition().map(|s| s.to_string()),
        })
        .collect();
    let incompatibilities: Vec<FileRefOut> = meta.incompatibilities().iter().map(file_to_out).collect();
    let (requirements, load_after) = if include_req_la {
        (
            meta.requirements().iter().map(file_to_out).collect(),
            meta.load_after_files().iter().map(file_to_out).collect(),
        )
    } else {
        (Vec::new(), Vec::new())
    };
    PluginMetadataProblemsOut {
        messages,
        incompatibilities,
        requirements,
        load_after,
    }
}

fn problems_out_is_empty(p: &PluginMetadataProblemsOut) -> bool {
    p.messages.is_empty()
        && p.incompatibilities.is_empty()
        && p.requirements.is_empty()
        && p.load_after.is_empty()
}

fn get_evaluated_plugin_metadata(game: &Game, name: &str) -> Result<Option<PluginMetadata>, String> {
    let db = game.database();
    let db = db.read().map_err(|_| "LOOT database lock poisoned".to_string())?;
    match db.plugin_metadata(name, MergeMode::WithUserMetadata, EvalMode::Evaluate) {
        Ok(m) => Ok(m),
        Err(e) => Err(format!("{:#}", e)),
    }
}

fn plugin_names_page<'a>(
    current: &'a [String],
    offset: u32,
    limit: Option<u32>,
) -> Vec<&'a String> {
    let off = offset as usize;
    let it = current.iter().skip(off);
    match limit {
        None => it.collect(),
        Some(0) => Vec::new(),
        Some(l) => it.take(l as usize).collect(),
    }
}

fn base_plugin_out(game: &Game, name: &str, p: &Arc<Plugin>) -> PluginOut {
    PluginOut {
        active: Some(game.is_plugin_active(name)),
        version: p.version().map(|s| s.to_string()),
        header_version: p.header_version(),
        crc: p.crc(),
        is_master: p.is_master(),
        is_light: p.is_light_plugin(),
        bash_tags_in_plugin_header: p.bash_tags().to_vec(),
        metadata_yaml: String::new(),
        metadata_problems: None,
    }
}

fn fill_plugin_metadata_output(
    game: &Game,
    name: &str,
    content: PluginMetadataContent,
    include_req_la: bool,
    mut base: PluginOut,
) -> Result<Option<PluginOut>, anyhow::Error> {
    match content {
        PluginMetadataContent::Full => {
            match get_evaluated_plugin_metadata(game, name) {
                Ok(Some(m)) => {
                    base.metadata_yaml = m.as_yaml();
                }
                Ok(None) => {}
                Err(e) => {
                    base.metadata_yaml = format!("# metadata error: {}", e);
                }
            }
            Ok(Some(base))
        }
        PluginMetadataContent::Problems => {
            match get_evaluated_plugin_metadata(game, name) {
                Ok(Some(m)) => {
                    let prob = problems_from_plugin_metadata(&m, include_req_la);
                    if problems_out_is_empty(&prob) {
                        return Ok(None);
                    }
                    base.metadata_problems = Some(prob);
                    Ok(Some(base))
                }
                Ok(None) => Ok(None),
                Err(e) => {
                    base.metadata_problems = Some(PluginMetadataProblemsOut {
                        messages: vec![MsgOut {
                            severity: "error".to_string(),
                            text: format!("metadata error: {}", e),
                            condition: None,
                        }],
                        incompatibilities: vec![],
                        requirements: vec![],
                        load_after: vec![],
                    });
                    Ok(Some(base))
                }
            }
        }
    }
}

pub fn read_load_order(req: EvalRequest) -> LoadOrderReadResponse {
    match run_read_load_order(req) {
        Ok(out) => out,
        Err(e) => LoadOrderReadResponse {
            load_order: vec![],
            load_order_ambiguous: None,
            plugin_count: 0,
            source: None,
            note: None,
            error: Some(format!("{:#}", e)),
        },
    }
}

fn read_profile_text_lossy(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path).map_err(|e| anyhow!("read {}: {}", path.display(), e))?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// MO2 / LOOT `loadorder.txt`: one plugin per line; `#` comments; UTF-8 or lossy decode.
fn parse_loadorder_txt(content: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        out.push(line.to_string());
    }
    out
}

/// Skyrim SE / FO4 style `plugins.txt`: optional `*` prefix for active; same rules as libloadorder asterisk lines.
fn parse_plugins_txt_order(content: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let name = line.strip_prefix('*').map(str::trim).unwrap_or(line).trim();
        if !name.is_empty() {
            out.push(name.to_string());
        }
    }
    out
}

fn read_load_order_from_profile_dir(dir: &Path) -> Option<(Vec<String>, &'static str)> {
    let loadorder = dir.join("loadorder.txt");
    if loadorder.is_file() {
        if let Ok(text) = read_profile_text_lossy(&loadorder) {
            let v = parse_loadorder_txt(&text);
            if !v.is_empty() {
                return Some((v, "loadorder_txt"));
            }
        }
    }
    for fname in ["plugins.txt", "Plugins.txt"] {
        let p = dir.join(fname);
        if p.is_file() {
            if let Ok(text) = read_profile_text_lossy(&p) {
                let v = parse_plugins_txt_order(&text);
                if !v.is_empty() {
                    return Some((v, "plugins_txt"));
                }
            }
        }
    }
    None
}

fn run_read_load_order_via_libloadorder(req: &EvalRequest) -> Result<LoadOrderReadResponse> {
    let gt = parse_game_type(&req.game_type)?;
    let game_path = PathBuf::from(&req.game_path);

    let mut game = if let Some(ref lp) = req.game_local_path {
        let lp = lp.trim();
        if lp.is_empty() {
            Game::new(gt, &game_path)
        } else {
            Game::with_local_path(gt, &game_path, Path::new(lp))
        }
    } else {
        Game::new(gt, &game_path)
    }
    .map_err(|e| anyhow!("Game init: {}", e))?;

    let additional = collect_additional_data_paths(req)?;
    if !additional.is_empty() {
        game
            .set_additional_data_paths(additional)
            .map_err(|e| anyhow!("set_additional_data_paths: {}", e))?;
    }

    game.load_current_load_order_state()
        .map_err(|e| anyhow!("load_current_load_order_state: {}", e))?;

    let ambiguous = game
        .is_load_order_ambiguous()
        .map_err(|e| anyhow!("is_load_order_ambiguous: {}", e))?;

    let load_order: Vec<String> = game.load_order().into_iter().map(|s| s.to_string()).collect();
    let plugin_count = load_order.len();

    Ok(LoadOrderReadResponse {
        load_order,
        load_order_ambiguous: Some(ambiguous),
        plugin_count,
        source: Some("libloadorder".to_string()),
        note: None,
        error: None,
    })
}

fn run_read_load_order(req: EvalRequest) -> Result<LoadOrderReadResponse> {
    if req.load_order_use_libloadorder {
        return run_read_load_order_via_libloadorder(&req);
    }

    let Some(dir) = req
        .game_local_path
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
    else {
        return Err(anyhow!(
            "game_local_path is required for fast load order (read loadorder.txt / plugins.txt). \
             Set load_order_use_libloadorder true to use libloadorder only (slow with many MO2 mod paths)."
        ));
    };

    if let Some((load_order, src)) = read_load_order_from_profile_dir(&dir) {
        let plugin_count = load_order.len();
        return Ok(LoadOrderReadResponse {
            load_order,
            load_order_ambiguous: None,
            plugin_count,
            source: Some(src.to_string()),
            note: Some(
                "load_order_ambiguous is only computed when source is libloadorder".to_string(),
            ),
            error: None,
        });
    }

    run_read_load_order_via_libloadorder(&req)
}

fn resolve_plugin_name_in_load_order(current: &[String], requested: &str) -> Option<String> {
    let q = requested.trim();
    if q.is_empty() {
        return None;
    }
    current
        .iter()
        .find(|c| c.eq_ignore_ascii_case(q))
        .cloned()
}

/// Evaluated masterlist/userlist YAML for specific plugins only (same game setup as `loot_evaluate`, no sort/messages).
pub fn evaluate_plugin_metadata(req: EvalRequest, plugin_names: Vec<String>) -> PluginMetadataResponse {
    match run_plugin_metadata(req, plugin_names) {
        Ok(out) => out,
        Err(e) => PluginMetadataResponse {
            libloot_version: libloot_version().to_string(),
            libloot_revision: libloot_revision().to_string(),
            masterlist_path: String::new(),
            prelude_loaded: false,
            userlist_loaded: false,
            plugins: BTreeMap::new(),
            not_found: vec![],
            error: Some(format!("{:#}", e)),
        },
    }
}

fn run_plugin_metadata(req: EvalRequest, plugin_names: Vec<String>) -> Result<PluginMetadataResponse> {
    let trimmed: Vec<String> = plugin_names
        .into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if trimmed.is_empty() {
        return Err(anyhow!("plugin_names must contain at least one non-empty name"));
    }

    let prep = prepare_eval_game(&req)?;
    let game = prep.game;

    let mut seen = std::collections::BTreeSet::<String>::new();
    let mut not_found = Vec::new();
    let mut plugins = BTreeMap::new();

    for requested in trimmed {
        let Some(actual) = resolve_plugin_name_in_load_order(&prep.current, &requested) else {
            not_found.push(requested);
            continue;
        };
        if !seen.insert(actual.clone()) {
            continue;
        }
        let Some(p) = game.plugin(&actual) else {
            not_found.push(actual);
            continue;
        };
        let base = base_plugin_out(&game, &actual, &p);
        if let Some(out) = fill_plugin_metadata_output(
            &game,
            &actual,
            req.plugin_metadata_content,
            req.plugin_problems_include_requirements_load_after,
            base,
        )? {
            plugins.insert(actual, out);
        }
    }

    Ok(PluginMetadataResponse {
        libloot_version: libloot_version().to_string(),
        libloot_revision: libloot_revision().to_string(),
        masterlist_path: prep.masterlist_path_str,
        prelude_loaded: prep.prelude_loaded,
        userlist_loaded: prep.userlist_loaded,
        plugins,
        not_found,
        error: None,
    })
}

fn prepare_eval_game(req: &EvalRequest) -> Result<PreparedEval> {
    let gt = parse_game_type(&req.game_type)?;
    let game_path = PathBuf::from(&req.game_path);

    let mut game = if let Some(ref lp) = req.game_local_path {
        let lp = lp.trim();
        if lp.is_empty() {
            Game::new(gt, &game_path)
        } else {
            Game::with_local_path(gt, &game_path, Path::new(lp))
        }
    } else {
        Game::new(gt, &game_path)
    }
    .map_err(|e| anyhow!("Game init: {}", e))?;

    let additional = collect_additional_data_paths(req)?;
    if !additional.is_empty() {
        game
            .set_additional_data_paths(additional)
            .map_err(|e| anyhow!("set_additional_data_paths: {}", e))?;
    }

    let loot_root = if let Some(ref p) = req.loot_data_path {
        let p = p.trim();
        if p.is_empty() {
            default_loot_data_path()?
        } else {
            PathBuf::from(p)
        }
    } else {
        default_loot_data_path()?
    };

    let folder = req
        .loot_game_folder
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .unwrap_or_else(|| default_loot_folder(gt));

    let game_dir = resolve_game_loot_dir(&loot_root, &folder);
    let default_ml = game_dir.join("masterlist.yaml");
    let default_prelude = default_prelude_path(&loot_root, &game_dir);
    let default_user = game_dir.join("userlist.yaml");

    let masterlist = req
        .masterlist_path
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or(default_ml);

    let prelude = req
        .prelude_path
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or(default_prelude);

    let userlist = req
        .userlist_path
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or(default_user);

    let masterlist_path_str = masterlist.to_string_lossy().into_owned();

    let mut prelude_loaded = false;
    {
        let db = game.database();
        let mut db = db
            .write()
            .map_err(|_| anyhow!("LOOT database lock poisoned"))?;

        if !masterlist.exists() {
            return Err(anyhow!(
                "masterlist not found at {} (run LOOT once to download, or set masterlist_path)",
                masterlist.display()
            ));
        }

        if prelude.exists() {
            db.load_masterlist_with_prelude(&masterlist, &prelude)
                .map_err(|e| anyhow!("load_masterlist_with_prelude: {}", e))?;
            prelude_loaded = true;
        } else {
            db.load_masterlist(&masterlist)
                .map_err(|e| anyhow!("load_masterlist: {}", e))?;
        }

        if userlist.exists() {
            db.load_userlist(&userlist)
                .map_err(|e| anyhow!("load_userlist: {}", e))?;
        }
    }

    let userlist_loaded = userlist.exists();

    game.load_current_load_order_state()
        .map_err(|e| anyhow!("load_current_load_order_state: {}", e))?;

    let current: Vec<String> = game.load_order().into_iter().map(|s| s.to_string()).collect();
    if current.is_empty() {
        return Err(anyhow!(
            "no load order loaded: libloadorder resolves plugins only as files in game/Data and additional_data_paths (non-recursive). MO2 often leaves game/Data empty — set mo2_mods_path to the MO2 mods folder (with game_local_path = profile dir containing modlist.txt), and/or additional_data_paths to flat plugin directories."
        ));
    }

    let path_refs = absolute_plugin_paths_for_libloot(&game, gt, &game_path, &current);
    let refs: Vec<&Path> = path_refs.iter().map(|p| p.as_path()).collect();
    game.load_plugin_headers(&refs)
        .map_err(|e| anyhow!("load_plugin_headers: {}", e))?;

    Ok(PreparedEval {
        game,
        current,
        masterlist_path_str,
        prelude_loaded,
        userlist_loaded,
    })
}

/// Run LOOT evaluation. On failure returns `EvalResponse` with `error` set (for JSON + MCP isError).
pub fn evaluate(req: EvalRequest) -> EvalResponse {
    match run(req) {
        Ok(out) => out,
        Err(e) => EvalResponse {
            libloot_version: libloot_version().to_string(),
            libloot_revision: libloot_revision().to_string(),
            masterlist_path: String::new(),
            prelude_loaded: false,
            userlist_loaded: false,
            load_order_current: vec![],
            load_order_suggested: vec![],
            load_order_ambiguous: None,
            general_messages: vec![],
            plugins: BTreeMap::new(),
            master_header_issues: vec![],
            plugin_metadata_page: None,
            error: Some(format!("{:#}", e)),
        },
    }
}

fn run(req: EvalRequest) -> Result<EvalResponse> {
    let prep = prepare_eval_game(&req)?;
    let game = prep.game;
    let current = prep.current;
    let masterlist_path_str = prep.masterlist_path_str;
    let prelude_loaded = prep.prelude_loaded;
    let userlist_loaded = prep.userlist_loaded;

    let ambiguous = game
        .is_load_order_ambiguous()
        .map_err(|e| anyhow!("is_load_order_ambiguous: {}", e))?;

    let name_refs: Vec<&str> = current.iter().map(|s| s.as_str()).collect();
    let suggested = game
        .sort_plugins(&name_refs)
        .map_err(|e| anyhow!("sort_plugins: {}", e))?;

    let gen = {
        let db = game.database();
        let db = db
            .read()
            .map_err(|_| anyhow!("LOOT database lock poisoned"))?;
        db.general_messages(MergeMode::WithUserMetadata, EvalMode::Evaluate)
            .map_err(|e| anyhow!("general_messages: {}", e))?
    };

    let general_messages: Vec<MsgOut> = gen
        .iter()
        .map(|m| MsgOut {
            severity: message_severity(m.message_type()).to_string(),
            text: message_text(m),
            condition: m.condition().map(|s| s.to_string()),
        })
        .collect();
    let general_messages = filter_general_messages(general_messages, req.general_messages_min_severity);

    let master_header_issues = if req.include_master_header_issues {
        collect_master_header_issues(&game, &current)
    } else {
        vec![]
    };

    let mut plugins = BTreeMap::new();
    let mut plugin_metadata_page = None;

    if req.include_plugin_metadata {
        let total = current.len();
        let offset = req.plugin_metadata_offset as usize;
        let names = plugin_names_page(&current, req.plugin_metadata_offset, req.plugin_metadata_limit);
        plugin_metadata_page = Some(PluginMetadataPageOut {
            total,
            offset,
            returned: names.len(),
            has_more: offset.saturating_add(names.len()) < total,
        });

        for name in names {
            let Some(p) = game.plugin(name) else {
                continue;
            };
            let base = base_plugin_out(&game, name, &p);
            if let Some(out) = fill_plugin_metadata_output(
                &game,
                name,
                req.plugin_metadata_content,
                req.plugin_problems_include_requirements_load_after,
                base,
            )? {
                plugins.insert(name.clone(), out);
            }
        }
    }

    Ok(EvalResponse {
        libloot_version: libloot_version().to_string(),
        libloot_revision: libloot_revision().to_string(),
        masterlist_path: masterlist_path_str,
        prelude_loaded,
        userlist_loaded,
        load_order_current: current,
        load_order_suggested: suggested,
        load_order_ambiguous: Some(ambiguous),
        general_messages,
        plugins,
        master_header_issues,
        plugin_metadata_page,
        error: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn parse_game_type_aliases() {
        assert!(matches!(
            parse_game_type("SkyrimSE").unwrap(),
            GameType::SkyrimSE
        ));
        assert!(matches!(
            parse_game_type("Fallout 4").unwrap(),
            GameType::Fallout4
        ));
    }

    #[test]
    fn parse_mo2_modlist_order_and_reverse_priority() {
        let dir = tempfile::tempdir().unwrap();
        let ml = dir.path().join("modlist.txt");
        let mut f = std::fs::File::create(&ml).unwrap();
        writeln!(f, "# hi").unwrap();
        writeln!(f, "+LowPriority").unwrap();
        writeln!(f, "-Off").unwrap();
        writeln!(f, "+HighPriority").unwrap();
        drop(f);

        let mods = dir.path().join("mods");
        std::fs::create_dir_all(mods.join("LowPriority/Data")).unwrap();
        std::fs::create_dir_all(mods.join("HighPriority/Data")).unwrap();

        let roots = mo2_additional_plugin_roots(&mods, dir.path()).unwrap();
        assert_eq!(roots.len(), 2);
        assert!(roots[0].ends_with("HighPriority/Data"));
        assert!(roots[1].ends_with("LowPriority/Data"));
    }

    #[test]
    fn parse_loadorder_txt_skips_comments_and_blank() {
        let s = "# c\n\nSkyrim.esm\n\n# x\nFoo.esp\n";
        assert_eq!(
            parse_loadorder_txt(s),
            vec!["Skyrim.esm".to_string(), "Foo.esp".to_string()]
        );
    }

    #[test]
    fn parse_plugins_txt_order_asterisk_and_inactive() {
        let s = "# h\n*A.esp\nB.esl\n\n* C.esp \n";
        assert_eq!(
            parse_plugins_txt_order(s),
            vec!["A.esp".to_string(), "B.esl".to_string(), "C.esp".to_string()]
        );
    }

    #[test]
    fn read_load_order_from_profile_prefers_loadorder_txt() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("loadorder.txt"), "a.esp\nb.esp\n").unwrap();
        std::fs::write(dir.path().join("plugins.txt"), "*x.esp\n").unwrap();
        let (v, src) = read_load_order_from_profile_dir(dir.path()).unwrap();
        assert_eq!(src, "loadorder_txt");
        assert_eq!(v, vec!["a.esp", "b.esp"]);
    }

    #[test]
    fn read_load_order_from_profile_plugins_only() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Plugins.txt"), "*m1.esp\nm2.esl\n").unwrap();
        let (v, src) = read_load_order_from_profile_dir(dir.path()).unwrap();
        assert_eq!(src, "plugins_txt");
        assert_eq!(v, vec!["m1.esp", "m2.esl"]);
    }

    #[test]
    fn plugin_names_page_slice() {
        let cur: Vec<String> = (0..5).map(|i| format!("p{i}.esp")).collect();
        let a: Vec<_> = plugin_names_page(&cur, 0, None).into_iter().cloned().collect();
        assert_eq!(a.len(), 5);
        let b: Vec<_> = plugin_names_page(&cur, 2, Some(2)).into_iter().cloned().collect();
        assert_eq!(b, vec!["p2.esp".to_string(), "p3.esp".to_string()]);
        let c: Vec<_> = plugin_names_page(&cur, 4, Some(10)).into_iter().cloned().collect();
        assert_eq!(c, vec!["p4.esp".to_string()]);
        assert!(plugin_names_page(&cur, 0, Some(0)).is_empty());
    }

    #[test]
    fn filter_general_messages_by_severity() {
        let msgs = vec![
            MsgOut {
                severity: "say".to_string(),
                text: "a".to_string(),
                condition: None,
            },
            MsgOut {
                severity: "warn".to_string(),
                text: "b".to_string(),
                condition: None,
            },
            MsgOut {
                severity: "error".to_string(),
                text: "c".to_string(),
                condition: None,
            },
        ];
        let w = filter_general_messages(msgs.clone(), GeneralMessageMinSeverity::Warn);
        assert_eq!(w.len(), 2);
        let e = filter_general_messages(msgs, GeneralMessageMinSeverity::Error);
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].severity, "error");
    }
}
