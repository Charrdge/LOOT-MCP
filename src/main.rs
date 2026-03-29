//! LOOT MCP server: stdio transport, tools backed by libloot.

mod evaluate;

use evaluate::{evaluate, evaluate_plugin_metadata, read_load_order, EvalRequest};
use rmcp::{
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{CallToolResult, Content, Implementation, ProtocolVersion, ServerCapabilities, ServerInfo},
    schemars::JsonSchema,
    service::RequestContext,
    tool, tool_handler, tool_router,
    ErrorData as McpError, RoleServer, ServerHandler, ServiceExt,
};
use serde::Serialize;
use tokio::io::{stdin, stdout};

const INSTRUCTIONS: &str = "LOOT/libloot read-only. loot_load_order reads loadorder.txt/plugins.txt from game_local_path (fast). load_order_use_libloadorder true = slow libloadorder scan. Full LOOT: loot_evaluate. Per-plugin YAML: loot_plugin_metadata.";

fn env_nonempty(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn or_env_opt(arg: Option<String>, env_name: &str) -> Option<String> {
    arg.filter(|s| !s.trim().is_empty())
        .or_else(|| env_nonempty(env_name))
}

fn or_env_str(arg: String, env_name: &str, fallback: &str) -> String {
    let t = arg.trim();
    if !t.is_empty() {
        return t.to_string();
    }
    env_nonempty(env_name).unwrap_or_else(|| fallback.to_string())
}

#[derive(Debug, serde::Deserialize, JsonSchema)]
pub struct LootEvaluateArgs {
    #[schemars(description = "LOOT game id, e.g. SkyrimSE, Fallout4, or \"Skyrim Special Edition\"")]
    pub game_type: String,
    #[schemars(description = "Game install root (directory with the game executable)")]
    pub game_path: String,
    #[schemars(description = "Game local path: folder with plugins.txt / loadorder.txt. For MO2, use the active profile directory.")]
    #[serde(default)]
    pub game_local_path: Option<String>,
    #[schemars(description = "LOOT application data root (contains per-game subfolders with masterlist). Default: LOOT_DATA_PATH env or OS default.")]
    #[serde(default)]
    pub loot_data_path: Option<String>,
    #[schemars(description = "Subfolder under loot data, e.g. Skyrim Special Edition. Default derived from game_type.")]
    #[serde(default)]
    pub loot_game_folder: Option<String>,
    #[schemars(description = "Explicit path to masterlist.yaml")]
    #[serde(default)]
    pub masterlist_path: Option<String>,
    #[schemars(description = "Explicit path to prelude.yaml")]
    #[serde(default)]
    pub prelude_path: Option<String>,
    #[schemars(description = "Explicit path to userlist.yaml")]
    #[serde(default)]
    pub userlist_path: Option<String>,
    #[schemars(description = "Extra plugin directories (flat: .esp/.esm/.esl in the directory itself, not in subfolders). Earlier entries win over later ones and over game/Data. Use for MO2 merges or manual roots.")]
    #[serde(default)]
    pub additional_data_paths: Option<Vec<String>>,
    #[schemars(description = "Path to Mod Organizer 2 \"mods\" folder. Requires game_local_path = active profile dir (modlist.txt). Appends each enabled mod's Data/ (or mod folder) in MO2 priority order.")]
    #[serde(default)]
    pub mo2_mods_path: Option<String>,
    #[schemars(description = "If true, include a `plugins` map with evaluated LOOT YAML per plugin (very large). Default false: load_order_current, load_order_suggested, general_messages only.")]
    #[serde(default)]
    pub include_plugin_metadata: bool,
    #[schemars(description = "For loot_load_order: if true, use libloadorder (slow with many MO2 paths). Default false: read loadorder.txt or plugins.txt from game_local_path.")]
    #[serde(default)]
    pub load_order_use_libloadorder: bool,
}

#[derive(Debug, serde::Deserialize, JsonSchema)]
pub struct LootPluginMetadataArgs {
    #[serde(flatten)]
    #[schemars(flatten)]
    pub base: LootEvaluateArgs,
    #[schemars(description = "Basenames to fetch evaluated LOOT YAML for (e.g. MyMod.esp). Must be in the current load order.")]
    pub plugin_names: Vec<String>,
}

fn merge_loot_eval_request(args: &LootEvaluateArgs) -> EvalRequest {
    EvalRequest {
        game_type: or_env_str(args.game_type.clone(), "LOOT_MCP_GAME_TYPE", "SkyrimSE"),
        game_path: or_env_str(args.game_path.clone(), "LOOT_MCP_GAME_PATH", ""),
        game_local_path: or_env_opt(args.game_local_path.clone(), "LOOT_MCP_GAME_LOCAL_PATH"),
        loot_data_path: or_env_opt(args.loot_data_path.clone(), "LOOT_DATA_PATH"),
        loot_game_folder: args.loot_game_folder.clone(),
        masterlist_path: args.masterlist_path.clone(),
        prelude_path: args.prelude_path.clone(),
        userlist_path: args.userlist_path.clone(),
        additional_data_paths: args.additional_data_paths.clone(),
        mo2_mods_path: or_env_opt(args.mo2_mods_path.clone(), "LOOT_MCP_MO2_MODS_PATH"),
        include_plugin_metadata: args.include_plugin_metadata,
        load_order_use_libloadorder: args.load_order_use_libloadorder,
    }
}

#[derive(Clone)]
pub struct LootServer {
    tool_router: ToolRouter<LootServer>,
}

#[tool_router]
impl LootServer {
    fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
        }
    }

    #[tool(
        name = "loot_evaluate",
        description = "Read-only: libloot — suggested load order, general messages (default). Set include_plugin_metadata true for per-plugin evaluated YAML (heavy). Does not write plugins.txt."
    )]
    async fn loot_evaluate(
        &self,
        Parameters(args): Parameters<LootEvaluateArgs>,
    ) -> Result<CallToolResult, McpError> {
        let req = merge_loot_eval_request(&args);
        let out = tokio::task::spawn_blocking(move || evaluate(req))
            .await
            .map_err(|e| {
                McpError::internal_error(
                    "evaluate_join",
                    Some(serde_json::json!({ "message": e.to_string() })),
                )
            })?;
        let is_err = out.error.is_some();
        let text = serde_json::to_string_pretty(&out).map_err(|e| {
            McpError::internal_error(
                "json",
                Some(serde_json::json!({ "message": e.to_string() })),
            )
        })?;
        if is_err {
            Ok(CallToolResult::error(vec![Content::text(text)]))
        } else {
            Ok(CallToolResult::success(vec![Content::text(text)]))
        }
    }

    #[tool(
        name = "loot_load_order",
        description = "Read plugin load order from profile loadorder.txt or plugins.txt (fast, default). Falls back to libloadorder if those files are missing/empty. Set load_order_use_libloadorder true to force libloadorder (slow with many MO2 mod dirs). Requires game_local_path unless forcing libloadorder."
    )]
    async fn loot_load_order(
        &self,
        Parameters(args): Parameters<LootEvaluateArgs>,
    ) -> Result<CallToolResult, McpError> {
        let req = merge_loot_eval_request(&args);
        let out = tokio::task::spawn_blocking(move || read_load_order(req))
            .await
            .map_err(|e| {
                McpError::internal_error(
                    "load_order_join",
                    Some(serde_json::json!({ "message": e.to_string() })),
                )
            })?;
        let is_err = out.error.is_some();
        let text = serde_json::to_string_pretty(&out).map_err(|e| {
            McpError::internal_error(
                "json",
                Some(serde_json::json!({ "message": e.to_string() })),
            )
        })?;
        if is_err {
            Ok(CallToolResult::error(vec![Content::text(text)]))
        } else {
            Ok(CallToolResult::success(vec![Content::text(text)]))
        }
    }

    #[tool(
        name = "loot_plugin_metadata",
        description = "Evaluated LOOT YAML (masterlist+userlist) for named plugins only. Same paths as loot_evaluate; loads full load order internally but returns metadata just for plugin_names."
    )]
    async fn loot_plugin_metadata(
        &self,
        Parameters(args): Parameters<LootPluginMetadataArgs>,
    ) -> Result<CallToolResult, McpError> {
        let req = merge_loot_eval_request(&args.base);
        let names = args.plugin_names;
        let out = tokio::task::spawn_blocking(move || evaluate_plugin_metadata(req, names))
            .await
            .map_err(|e| {
                McpError::internal_error(
                    "plugin_metadata_join",
                    Some(serde_json::json!({ "message": e.to_string() })),
                )
            })?;
        let is_err = out.error.is_some();
        let text = serde_json::to_string_pretty(&out).map_err(|e| {
            McpError::internal_error(
                "json",
                Some(serde_json::json!({ "message": e.to_string() })),
            )
        })?;
        if is_err {
            Ok(CallToolResult::error(vec![Content::text(text)]))
        } else {
            Ok(CallToolResult::success(vec![Content::text(text)]))
        }
    }

    #[tool(
        name = "loot_server_info",
        description = "loot-mcp and libloot versions, optional executable path."
    )]
    fn loot_server_info(&self, _: RequestContext<RoleServer>) -> Result<CallToolResult, McpError> {
        #[derive(Serialize)]
        struct Info<'a> {
            version: &'a str,
            libloot_version: String,
            libloot_revision: String,
            executable: Option<String>,
            note: &'a str,
        }
        let info = Info {
            version: env!("CARGO_PKG_VERSION"),
            libloot_version: libloot::libloot_version().to_string(),
            libloot_revision: libloot::libloot_revision().to_string(),
            executable: std::env::current_exe()
                .ok()
                .map(|p| p.to_string_lossy().into_owned()),
            note: "LOOT metadata from masterlist/userlist + libloot; not Nexus.",
        };
        let text = serde_json::to_string_pretty(&info).map_err(|e| {
            McpError::internal_error(
                "json",
                Some(serde_json::json!({ "message": e.to_string() })),
            )
        })?;
        Ok(CallToolResult::success(vec![Content::text(text)]))
    }
}

#[tool_handler]
impl ServerHandler for LootServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(
            ServerCapabilities::builder()
                .enable_tools()
                .build(),
        )
        .with_server_info(Implementation::new("loot-mcp", env!("CARGO_PKG_VERSION")))
        .with_protocol_version(ProtocolVersion::V_2024_11_05)
        .with_instructions(INSTRUCTIONS.to_string())
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let service = LootServer::new();
    let transport = (stdin(), stdout());
    let server = service.serve(transport).await?;
    server.waiting().await?;
    Ok(())
}
