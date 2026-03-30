# loot-mcp

Single **Rust** binary: a read-only [MCP](https://modelcontextprotocol.io/) server using **[rmcp](https://github.com/modelcontextprotocol/rust-sdk)** and **[libloot](https://github.com/loot/libloot)**. It exposes LOOT-style output (masterlist-driven load order suggestion, plugin metadata YAML, general messages, Bash tags from plugin headers) — **not** Nexus API data.

**Does not** replace **mutagen-mcp** (records, FormIDs, compare) or **mod-organizer-mcp** (MO2 layout, `plugins.txt`). Use those for lists and deep plugin analysis; use `loot_evaluate` with paths from MO2 (especially `game_local_path` = active profile folder).

Version: see `Cargo.toml` (`loot_server_info` also returns the crate version at runtime).

## Build (recommended: Docker)

This repo is meant to be built and run in a container (same toolchain as CI/production):

```bash
docker build -t loot-mcp .
# binary ends up in the image as /usr/local/bin/loot-mcp
```

Local `cargo build --release` works if you have a suitable Rust + libloot environment; the binary path is `target/release/loot-mcp`.

## Run (stdio)

```bash
docker run -i --rm loot-mcp
```

The process speaks MCP over stdio (typical for Cursor and other MCP clients).

## Environment variables

| Variable | Purpose |
|----------|---------|
| `LOOT_DATA_PATH` | LOOT application data root (masterlists, prelude). Used when tool args omit `loot_data_path`. |
| `LOOT_MCP_GAME_TYPE` | Default game id (e.g. `SkyrimSE`) when the client omits `game_type`. |
| `LOOT_MCP_GAME_PATH` | Default game install root when the client omits `game_path`. |
| `LOOT_MCP_GAME_LOCAL_PATH` | Default profile folder (`plugins.txt` / `loadorder.txt`). |
| `LOOT_MCP_MO2_MODS_PATH` | MO2 `mods` directory; requires profile path and `modlist.txt` for priority order. |
| `LOOT_MCP_CACHE` | In-process prep cache for libloot game + plugin headers. Default **`1` in the published Docker image**; set `0` to disable. |
| `LOOT_MCP_CACHE_TTL_SEC` | Optional TTL (seconds) for cache entries when set to a positive value. |
| `LOOT_MCP_TIMING` | `1` / `true`: print phase timings to **stderr** (`loot-mcp timing: …`). |
| `LOOT_MCP_TIMINGS_JSON` | `1` / `true`: add a `timings` object to tool JSON responses (`total_ms`, `prep_cache`, per-phase ms). |
| `LOOT_MCP_DIAGNOSTICS_LOG` | Absolute path to an **NDJSON** file; appends one JSON object per begin/end (and errors). In Docker, bind-mount a host directory and point this path inside the container. |
| `LOOT_MCP_PARALLEL_METADATA` | Set to `0` to disable parallel per-plugin metadata evaluation (default is parallel). |

## Cursor MCP config

### Docker (typical)

Use **paths as seen inside the container** in tool arguments (e.g. `game_path: "/game"`, `game_local_path: "/profile"`).

```json
{
  "mcpServers": {
    "loot-mcp": {
      "command": "docker",
      "args": [
        "run", "--rm", "-i",
        "-v", "/path/to/Skyrim Special Edition:/game:ro",
        "-v", "/path/to/MO2/profiles/MyProfile:/profile:ro",
        "-v", "/path/to/MO2/mods:/mods:ro",
        "-v", "/path/to/LOOT AppData:/loot:ro",
        "-e", "LOOT_DATA_PATH=/loot",
        "-e", "LOOT_MCP_GAME_TYPE=SkyrimSE",
        "-e", "LOOT_MCP_GAME_PATH=/game",
        "-e", "LOOT_MCP_GAME_LOCAL_PATH=/profile",
        "-e", "LOOT_MCP_MO2_MODS_PATH=/mods",
        "-e", "LOOT_MCP_CACHE=1",
        "-e", "LOOT_MCP_TIMINGS_JSON=1",
        "-e", "LOOT_MCP_DIAGNOSTICS_LOG=/logs/loot-mcp-diagnostics.ndjson",
        "-v", "/path/on/host/mcp-logs/loot:/logs",
        "loot-mcp"
      ]
    }
  }
}
```

Adjust host paths and image tag (`loot-mcp`, `loot-mcp:local`, etc.). Omit `LOOT_MCP_TIMINGS_JSON` / `LOOT_MCP_DIAGNOSTICS_LOG` / the logs volume if you do not need diagnostics.

### Local binary

```json
{
  "mcpServers": {
    "loot": {
      "command": "/absolute/path/to/loot-mcp"
    }
  }
}
```

## Container one-liner (manual test)

```bash
docker build -t loot-mcp .
docker run -i --rm \
  -v "/path/to/Game:/game:ro" \
  -v "/path/to/MO2/Profile:/profile:ro" \
  -v "/path/to/MO2/mods:/mods:ro" \
  -v "/path/to/LOOT:/loot:ro" \
  -e LOOT_DATA_PATH=/loot \
  -e LOOT_MCP_GAME_TYPE=SkyrimSE \
  -e LOOT_MCP_GAME_PATH=/game \
  -e LOOT_MCP_GAME_LOCAL_PATH=/profile \
  -e LOOT_MCP_MO2_MODS_PATH=/mods \
  loot-mcp
```

The image sets `ENV LOOT_MCP_CACHE=1` so repeated tools in one container reuse prep when signatures match.

## Tools

- **`loot_evaluate`** — Main evaluation: `load_order_current`, optional `load_order_suggested`, `general_messages`, optional per-plugin metadata. Required MCP args: `game_type`, `game_path` (defaults can come from env above). Useful flags: `include_load_order_suggested: false` skips sorting (faster); `include_plugin_metadata: true` with `plugin_metadata_content: "problems"` returns only plugins with LOOT issues (warn/error messages, incompatibilities, optional requirements/load_after); `general_messages_min_severity`: `say` \| `warn` \| `error`; pagination via `plugin_metadata_offset` / `plugin_metadata_limit`; `include_master_header_issues`. On failure, JSON includes `error` and the MCP result is an error.
- **`loot_load_order`** — Reads `loadorder.txt` or `plugins.txt` from `game_local_path` (fast). Optional `load_order_use_libloadorder: true` forces libloadorder (slower with many MO2 paths). Same path/env defaults as `loot_evaluate`.
- **`loot_plugin_metadata`** — Evaluated metadata for an explicit list of plugin names; same base arguments as `loot_evaluate` (flattened), plus `plugin_names`.
- **`loot_server_info`** — Crate version, libloot version/revision, executable path.

## Notes

### `general_messages` vs per-plugin messages

`general_messages` are **global** LOOT messages (prelude / masterlist “general” section) — usually a small count. Most masterlist text is **per plugin**; use `include_plugin_metadata` and `plugin_metadata_content: "problems"` or `"full"` to see it.

### WSL / Linux

Game and profile paths must be readable inside the process (e.g. `/mnt/c/...` or distro mounts). libloot behavior matches upstream LOOT for supported games.
