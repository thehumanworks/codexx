#!/usr/bin/env python3

"""Verify codex-tui stays behind the app-server/core boundary."""

from __future__ import annotations

import re
import sys
import tomllib
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
TUI_ROOT = ROOT / "codex-rs" / "tui"
TUI_MANIFEST = TUI_ROOT / "Cargo.toml"
FORBIDDEN_PACKAGE = "codex-core"
CODEX_PROTOCOL_PACKAGE = "codex-protocol"
CODEX_PROTOCOL_MESSAGE = "references `codex_protocol::protocol`"
IDENTIFIER = r"[A-Za-z_][A-Za-z0-9_]*"
FORBIDDEN_SOURCE_RULES = (
    (
        "imports `codex_core`",
        (
            re.compile(r"\bcodex_core::"),
            re.compile(r"\buse\s+codex_core\b"),
            re.compile(r"\bextern\s+crate\s+codex_core\b"),
        ),
    ),
)
CRATE_ALIAS_PATTERNS = (
    re.compile(rf"\buse\s+({IDENTIFIER})\s+as\s+({IDENTIFIER})\s*;"),
    re.compile(rf"\bextern\s+crate\s+({IDENTIFIER})\s+as\s+({IDENTIFIER})\s*;"),
)
GROUPED_USE_PATTERN = re.compile(
    rf"\buse\s+({IDENTIFIER})\s*::\s*\{{([^;]*)\}}\s*;"
)
GROUPED_SELF_ALIAS_PATTERN = re.compile(rf"\bself\s+as\s+({IDENTIFIER})\b")


def main() -> int:
    failures = []
    failures.extend(manifest_failures())
    failures.extend(source_failures())

    if not failures:
        return 0

    print("codex-tui must stay behind the app-server/core boundary.")
    print(
        "Use app-server protocol types at the TUI boundary; temporary embedded "
        "startup gaps belong behind codex_app_server_client::legacy_core, and "
        "core protocol references should remain outside codex-tui."
    )
    print()
    for failure in failures:
        print(f"- {failure}")

    return 1


def manifest_failures() -> list[str]:
    manifest = tomllib.loads(TUI_MANIFEST.read_text())
    failures = []
    for section_name, dependencies in dependency_sections(manifest):
        if FORBIDDEN_PACKAGE in dependencies:
            failures.append(
                f"{relative_path(TUI_MANIFEST)} declares `{FORBIDDEN_PACKAGE}` "
                f"in `[{section_name}]`"
            )
    return failures


def dependency_sections(manifest: dict) -> list[tuple[str, dict]]:
    sections: list[tuple[str, dict]] = []
    for section_name in ("dependencies", "dev-dependencies", "build-dependencies"):
        dependencies = manifest.get(section_name)
        if isinstance(dependencies, dict):
            sections.append((section_name, dependencies))

    for target_name, target in manifest.get("target", {}).items():
        if not isinstance(target, dict):
            continue
        for section_name in ("dependencies", "dev-dependencies", "build-dependencies"):
            dependencies = target.get(section_name)
            if isinstance(dependencies, dict):
                sections.append((f'target.{target_name}.{section_name}', dependencies))

    return sections


def source_failures() -> list[str]:
    failures = []
    manifest = tomllib.loads(TUI_MANIFEST.read_text())
    codex_protocol_names = protocol_dependency_names(manifest)
    for path in sorted(TUI_ROOT.glob("**/*.rs")):
        text = path.read_text()
        seen_locations = set()
        for message, patterns in FORBIDDEN_SOURCE_RULES:
            for pattern in patterns:
                for match in pattern.finditer(text):
                    failures.append(source_failure(path, text, match.start(), message))
                    seen_locations.add((match.start(), message))

        for match in protocol_reference_matches(text, codex_protocol_names):
            key = (match.start(), CODEX_PROTOCOL_MESSAGE)
            if key in seen_locations:
                continue
            failures.append(source_failure(path, text, match.start(), CODEX_PROTOCOL_MESSAGE))
            seen_locations.add(key)
    return failures


def protocol_dependency_names(manifest: dict) -> set[str]:
    names = {"codex_protocol"}
    for _section_name, dependencies in dependency_sections(manifest):
        for dependency_name, dependency_value in dependencies.items():
            package_name = dependency_name
            if isinstance(dependency_value, dict):
                package_name = dependency_value.get("package", dependency_name)
            if package_name == CODEX_PROTOCOL_PACKAGE:
                names.add(rust_crate_name(dependency_name))
    return names


def rust_crate_name(package_or_dependency_name: str) -> str:
    return package_or_dependency_name.replace("-", "_")


def protocol_reference_matches(
    text: str, codex_protocol_names: set[str]
) -> list[re.Match[str]]:
    matches = []
    for crate_name in expanded_crate_aliases(text, codex_protocol_names):
        escaped_name = re.escape(crate_name)
        patterns = (
            re.compile(rf"\b{escaped_name}\s*::\s*protocol\b"),
            re.compile(rf"\b{escaped_name}\s*::\s*\{{[^;]*\bprotocol\b"),
        )
        for pattern in patterns:
            matches.extend(pattern.finditer(text))
    return matches


def expanded_crate_aliases(text: str, crate_names: set[str]) -> set[str]:
    aliases = set(crate_names)
    while True:
        previous_count = len(aliases)
        for source, alias in crate_alias_pairs(text):
            if source in aliases:
                aliases.add(alias)
        if len(aliases) == previous_count:
            return aliases


def crate_alias_pairs(text: str) -> list[tuple[str, str]]:
    pairs = []
    for pattern in CRATE_ALIAS_PATTERNS:
        for match in pattern.finditer(text):
            pairs.append((match.group(1), match.group(2)))
    for match in GROUPED_USE_PATTERN.finditer(text):
        source = match.group(1)
        body = match.group(2)
        for alias_match in GROUPED_SELF_ALIAS_PATTERN.finditer(body):
            pairs.append((source, alias_match.group(1)))
    return pairs


def source_failure(path: Path, text: str, offset: int, message: str) -> str:
    line_number = text.count("\n", 0, offset) + 1
    return f"{relative_path(path)}:{line_number} {message}"


def relative_path(path: Path) -> str:
    return str(path.relative_to(ROOT))


if __name__ == "__main__":
    sys.exit(main())
