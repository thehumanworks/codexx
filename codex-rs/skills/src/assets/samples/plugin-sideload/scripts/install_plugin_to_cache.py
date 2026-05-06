#!/usr/bin/env python3
"""Install a local Codex plugin into the temporary local plugin cache."""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import tempfile
from pathlib import Path
from typing import Any


DEFAULT_MARKETPLACE = "dev"
DEFAULT_VERSION = "local"
PLUGIN_MANIFEST_RELATIVE_PATH = Path(".codex-plugin") / "plugin.json"
PLUGIN_SEGMENT_RE = re.compile(r"^[A-Za-z0-9_-]+$")
PLUGIN_VERSION_RE = re.compile(r"^[A-Za-z0-9_.+-]+$")


def default_codex_home() -> Path:
    codex_home = os.environ.get("CODEX_HOME")
    if codex_home:
        return Path(codex_home).expanduser()
    return Path.home() / ".codex"


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description=(
            "Copy a local plugin into $CODEX_HOME/plugins/cache for development. "
            "The cache is temporary and can be overwritten by Codex at any time."
        )
    )
    parser.add_argument("plugin_path", help="Local plugin source directory")
    parser.add_argument(
        "--codex-home",
        default=str(default_codex_home()),
        help="Codex home directory (defaults to $CODEX_HOME, then ~/.codex)",
    )
    parser.add_argument(
        "--marketplace",
        default=DEFAULT_MARKETPLACE,
        help=f"Marketplace namespace for the cache entry (default: {DEFAULT_MARKETPLACE})",
    )
    parser.add_argument(
        "--version",
        default=DEFAULT_VERSION,
        help=f"Cache version directory to write (default: {DEFAULT_VERSION})",
    )
    parser.add_argument(
        "--no-enable",
        action="store_true",
        help="Only copy into the plugin cache; do not update config.toml",
    )
    return parser.parse_args()


def validate_segment(value: str, kind: str) -> None:
    if not value:
        raise ValueError(f"invalid {kind}: must not be empty")
    if not PLUGIN_SEGMENT_RE.fullmatch(value):
        raise ValueError(
            f"invalid {kind}: only ASCII letters, digits, `_`, and `-` are allowed"
        )


def validate_version(value: str) -> None:
    if not value:
        raise ValueError("invalid plugin version: must not be empty")
    if value in {".", ".."}:
        raise ValueError("invalid plugin version: path traversal is not allowed")
    if not PLUGIN_VERSION_RE.fullmatch(value):
        raise ValueError(
            "invalid plugin version: only ASCII letters, digits, `.`, `+`, `_`, and `-` are allowed"
        )


def load_manifest(plugin_root: Path) -> dict[str, Any]:
    manifest_path = plugin_root / PLUGIN_MANIFEST_RELATIVE_PATH
    if not manifest_path.is_file():
        raise FileNotFoundError(f"missing plugin manifest: {manifest_path}")
    with manifest_path.open(encoding="utf-8") as handle:
        manifest = json.load(handle)
    if not isinstance(manifest, dict):
        raise ValueError(f"{manifest_path} must contain a JSON object")
    return manifest


def plugin_name_from_manifest(manifest: dict[str, Any]) -> str:
    plugin_name = manifest.get("name")
    if not isinstance(plugin_name, str):
        raise ValueError("plugin.json field `name` must be a string")
    plugin_name = plugin_name.strip()
    validate_segment(plugin_name, "plugin name")
    return plugin_name


def copy_plugin_tree(source: Path, target: Path) -> None:
    target.mkdir(parents=True, exist_ok=True)
    for entry in os.scandir(source):
        source_path = Path(entry.path)
        target_path = target / entry.name
        if entry.is_dir(follow_symlinks=False):
            copy_plugin_tree(source_path, target_path)
        elif entry.is_file(follow_symlinks=False):
            target_path.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(source_path, target_path)


def replace_cache_entry(source: Path, plugin_base: Path, version: str) -> Path:
    parent = plugin_base.parent
    parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="plugin-install-", dir=parent) as staging_dir:
        staged_base = Path(staging_dir) / plugin_base.name
        staged_version = staged_base / version
        copy_plugin_tree(source, staged_version)
        backup_base = None
        if plugin_base.exists():
            backup_base = Path(staging_dir) / f"{plugin_base.name}.backup"
            plugin_base.rename(backup_base)
        try:
            staged_base.rename(plugin_base)
        except Exception:
            if backup_base is not None and backup_base.exists() and not plugin_base.exists():
                backup_base.rename(plugin_base)
            raise
    return plugin_base / version


def update_config_enabled(codex_home: Path, plugin_key: str) -> None:
    config_path = codex_home / "config.toml"
    config_path.parent.mkdir(parents=True, exist_ok=True)
    section_header = f'[plugins."{plugin_key}"]'
    enabled_line = "enabled = true"

    if config_path.exists():
        contents = config_path.read_text(encoding="utf-8")
    else:
        contents = ""

    lines = contents.splitlines()
    section_start = next(
        (index for index, line in enumerate(lines) if line.strip() == section_header),
        None,
    )

    if section_start is None:
        if contents and not contents.endswith("\n"):
            contents += "\n"
        if contents:
            contents += "\n"
        contents += f"{section_header}\n{enabled_line}\n"
        config_path.write_text(contents, encoding="utf-8")
        return

    section_end = next(
        (
            index
            for index in range(section_start + 1, len(lines))
            if lines[index].lstrip().startswith("[")
        ),
        len(lines),
    )

    lines = [
        line
        for index, line in enumerate(lines)
        if not (
            section_start < index < section_end
            and re.match(r"^\s*source\s*=", line)
        )
    ]
    section_end = next(
        (
            index
            for index in range(section_start + 1, len(lines))
            if lines[index].lstrip().startswith("[")
        ),
        len(lines),
    )

    for index in range(section_start + 1, section_end):
        if re.match(r"^\s*enabled\s*=", lines[index]):
            lines[index] = enabled_line
            config_path.write_text("\n".join(lines) + "\n", encoding="utf-8")
            return

    lines.insert(section_start + 1, enabled_line)
    config_path.write_text("\n".join(lines) + "\n", encoding="utf-8")


def main() -> None:
    args = parse_args()
    source = Path(args.plugin_path).expanduser().resolve()
    codex_home = Path(args.codex_home).expanduser().resolve()
    marketplace = args.marketplace
    version = args.version

    if not source.is_dir():
        raise ValueError(f"plugin source path is not a directory: {source}")
    validate_segment(marketplace, "marketplace name")
    validate_version(version)

    manifest = load_manifest(source)
    plugin_name = plugin_name_from_manifest(manifest)
    plugin_key = f"{plugin_name}@{marketplace}"
    plugin_base = codex_home / "plugins" / "cache" / marketplace / plugin_name
    installed_path = replace_cache_entry(source, plugin_base, version)

    print(f"Installed plugin cache entry: {installed_path}")
    print(f"Plugin key: {plugin_key}")
    print("Note: this cache is temporary and can be overwritten by Codex at any time.")

    if args.no_enable:
        print("Skipped config.toml update because --no-enable was set.")
    else:
        update_config_enabled(codex_home, plugin_key)
        print(f"Associated and enabled plugin in: {codex_home / 'config.toml'}")

    print("Restart Codex to pick up the refreshed plugin cache.")


if __name__ == "__main__":
    main()
