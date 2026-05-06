---
name: sideload-plugin
description: Install a local Codex plugin into the temporary local Codex plugin cache for development and testing. Use after plugin-creator or when a user asks to try, reinstall, refresh, or test a local plugin in Codex without publishing it.
---

# Sideload Plugin

Use this skill to copy a local plugin into Codex's local plugin cache so it can be tested in the current Codex installation.

This is a development workflow. The plugin cache is temporary runtime state under `$CODEX_HOME/plugins/cache` and can be overwritten, refreshed, deleted, or replaced by Codex at any time. Do not treat it as source storage, a publication mechanism, or a durable install location.

## Quick Start

Install a local plugin source directory into the dev marketplace cache and enable it:

```bash
python3 .agents/skills/sideload-plugin/scripts/install_plugin_to_cache.py <plugin-path>
```

On Windows, run the same script with the Python launcher or `python`:

```powershell
py -3 .agents\skills\sideload-plugin\scripts\install_plugin_to_cache.py <plugin-path>
```

The script:

- reads `<plugin-path>/.codex-plugin/plugin.json`
- copies the plugin to `$CODEX_HOME/plugins/cache/dev/<plugin-name>/local`
- associates and enables the cached plugin by writing or updating this valid `$CODEX_HOME/config.toml` entry:

```toml
[plugins."<plugin-name>@dev"]
enabled = true
```

Restart Codex after installation so the refreshed plugin cache and enabled plugin entry are picked up.

## Common Workflows

Reinstall after editing plugin files:

```bash
python3 .agents/skills/sideload-plugin/scripts/install_plugin_to_cache.py <plugin-path>
```

Use a custom marketplace namespace:

```bash
python3 .agents/skills/sideload-plugin/scripts/install_plugin_to_cache.py <plugin-path> --marketplace debug
```

Install to a specific cache version:

```bash
python3 .agents/skills/sideload-plugin/scripts/install_plugin_to_cache.py <plugin-path> --version local
```

Install without modifying `$CODEX_HOME/config.toml`:

```bash
python3 .agents/skills/sideload-plugin/scripts/install_plugin_to_cache.py <plugin-path> --no-enable
```

Use a non-default Codex home:

```bash
python3 .agents/skills/sideload-plugin/scripts/install_plugin_to_cache.py <plugin-path> --codex-home <path>
```

## Behavior

- Defaults `--codex-home` to `$CODEX_HOME`, then `~/.codex`.
- Defaults `--marketplace` to `dev`.
- Defaults `--version` to `local`.
- Requires plugin and marketplace names to use only ASCII letters, digits, `_`, and `-`.
- Requires cache version names to use only ASCII letters, digits, `.`, `+`, `_`, and `-`.
- Replaces the existing cache entry for the same `<marketplace>/<plugin>` with the newly copied plugin.
- Associates the plugin in `config.toml` by plugin key (`<plugin>@<marketplace>`), not by writing a `source` field; plugin source files stay in the cache path.
- Skips symlinks while copying, matching Codex's plugin cache behavior.
- Works on macOS, Linux, and Windows with Python 3 and only the standard library.

## When To Use Marketplace Workflows Instead

Use `plugin-creator` or a real marketplace workflow when the goal is to publish, share, or persist plugin source metadata. This skill is only for local development installs into a disposable cache.

If the user wants to evaluate, benchmark, or improve the sideloaded plugin after testing it, point
them to the `plugin-eval` plugin from the `openai-curated` marketplace.

## Validation

After changing the script, run a smoke test with a temporary Codex home:

```bash
python3 codex-rs/skills/src/assets/samples/sideload-plugin/scripts/install_plugin_to_cache.py \
  <plugin-path> \
  --codex-home <temporary-codex-home>
```

Confirm the output path exists and the config contains the enabled plugin entry.
