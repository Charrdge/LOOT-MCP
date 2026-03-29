# loot-mcp

Single **Rust** binary: a read-only [MCP](https://modelcontextprotocol.io/) server using **[rmcp](https://github.com/modelcontextprotocol/rust-sdk)** and **[libloot](https://github.com/loot/libloot)**. It exposes LOOT-style output (masterlist-driven load order suggestion, plugin metadata YAML, general messages, Bash tags from plugin headers) — **not** Nexus API data.

**Does not** replace **mutagen-mcp** (records, FormIDs, compare) or **mod-organizer-mcp** (MO2 layout, `plugins.txt`). Use those for lists and deep plugin analysis; use `loot_evaluate` with paths from MO2 (especially `game_local_path` = active profile folder).

## Build

```bash
cargo build --release
# binary: target/release/loot-mcp
```

Dependencies: stable Rust, same platform requirements as libloot (see upstream).

## Run (stdio)

```bash
./target/release/loot-mcp
```

Optional env: `LOOT_DATA_PATH` (default LOOT data root when tool args omit `loot_data_path`).

## Cursor MCP config

```json
{
  "mcpServers": {
    "loot": {
      "command": "/absolute/path/to/loot-mcp"
    }
  }
}
```

## Container

```bash
docker build -t loot-mcp .
docker run -i --rm \
  -v "/path/to/Game:/game:ro" \
  -v "/path/to/MO2/Profile:/profile:ro" \
  -v "/path/to/LOOT:/loot:ro" \
  -e LOOT_DATA_PATH=/loot \
  loot-mcp
```

Use **in-container paths** in `loot_evaluate` (e.g. `game_path: "/game"`, `game_local_path: "/profile"`). One image, one `ENTRYPOINT` — no secondary helper binary.

**Cursor via Docker:** `command: "docker"`, `args: ["run", "-i", "--rm", "-v", "...", "loot-mcp"]` — same as any stdio MCP in a container.

## Tools

- **`loot_evaluate`** — Required: `game_type`, `game_path`. Recommended for MO2: `game_local_path` pointing at the profile directory with `plugins.txt` / `loadorder.txt`. Masterlist defaults to `<loot_data>/<game folder>/masterlist.yaml` (same layout as the LOOT app). On failure, returns JSON with an `error` field and MCP tool result marked as error.
- **`loot_server_info`** — Crate version, libloot version/revision, optional executable path.

## WSL / Linux

Game and profile paths must be readable inside the process (e.g. `/mnt/c/...`). libloot behavior matches upstream LOOT for supported games.
