#!/usr/bin/env python3
"""Build a self-extracting Codex dev artifact with Makeself."""

from __future__ import annotations

import argparse
import hashlib
import os
import platform
import shutil
import stat
import subprocess
import sys
import tempfile
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent
CODEX_RS = REPO_ROOT / "codex-rs"
DEFAULT_PROFILE = "dev-small"
DEFAULT_CACHE_ROOT = "$HOME/.cache/codex-dev"
COMPLETE_SENTINEL = ".codex-makeself-complete"
RUNNER_NAME = "run-codex"


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--profile",
        default=DEFAULT_PROFILE,
        help=f"Cargo profile to build with. Default: {DEFAULT_PROFILE}.",
    )
    parser.add_argument(
        "--output",
        type=Path,
        default=None,
        help="Path to write the generated Makeself archive. Default: dist/codex-dev/codex-dev.run.",
    )
    parser.add_argument(
        "--cache-root",
        default=DEFAULT_CACHE_ROOT,
        help=(
            "Runtime cache root for extracted builds. Shell variables are preserved "
            f"in the generated artifact. Default: {DEFAULT_CACHE_ROOT}."
        ),
    )
    parser.add_argument(
        "--include-bwrap",
        choices=("auto", "always", "never"),
        default="auto",
        help="Whether to build and bundle bwrap. Default: auto, which includes it on Linux.",
    )
    parser.add_argument(
        "--skip-cargo-build",
        action="store_true",
        help="Use existing Cargo build outputs instead of invoking cargo build.",
    )
    parser.add_argument(
        "--keep-staging-dir",
        action="store_true",
        help="Keep the temporary staged payload directory for inspection.",
    )
    parser.add_argument(
        "--makeself",
        default="makeself",
        help="Path to the makeself executable. Default: makeself from PATH.",
    )
    parser.add_argument(
        "--makeself-header",
        type=Path,
        default=None,
        help="Path to makeself-header.sh. Default: infer from the makeself installation.",
    )
    return parser.parse_args()


def run_command(cmd: list[str], cwd: Path) -> None:
    print("+", " ".join(cmd), flush=True)
    subprocess.run(cmd, cwd=cwd, check=True)


def cargo_profile_output_dir(profile_name: str) -> Path:
    match profile_name:
        case "dev":
            profile_dir = "debug"
        case "release":
            profile_dir = "release"
        case _:
            profile_dir = profile_name
    return CODEX_RS / "target" / profile_dir


def host_executable_name(name: str) -> str:
    if os.name == "nt":
        return f"{name}.exe"
    return name


def should_include_bwrap(mode: str) -> bool:
    match mode:
        case "always":
            return True
        case "never":
            return False
        case "auto":
            return platform.system() == "Linux"
        case _:
            raise ValueError(f"unexpected bwrap mode: {mode}")


def validate_cache_root(cache_root: str) -> None:
    normalized = cache_root.rstrip("/")
    forbidden_roots = {
        "/tmp",
        "/private/tmp",
        "/var/tmp",
        "/var/folders",
        "/private/var/folders",
        "$TMPDIR",
        "${TMPDIR}",
        "${TMPDIR:-/tmp}",
        "${TMPDIR-/tmp}",
    }
    system_temp = Path(tempfile.gettempdir()).resolve()
    forbidden_roots.add(str(system_temp))
    if normalized in forbidden_roots:
        raise RuntimeError(f"Refusing to use temp directory as cache root: {cache_root}")

    forbidden_prefixes = tuple(f"{root}/" for root in sorted(forbidden_roots))
    if normalized.startswith(forbidden_prefixes):
        raise RuntimeError(f"Refusing to use temp directory as cache root: {cache_root}")


def build_binaries(profile_name: str, include_bwrap: bool, skip_cargo_build: bool) -> None:
    if skip_cargo_build:
        return

    cmd = ["cargo", "build", "--profile", profile_name, "--bin", "codex"]
    if include_bwrap:
        cmd.extend(["--bin", "bwrap"])
    run_command(cmd, cwd=CODEX_RS)


def require_file(path: Path, description: str) -> None:
    if not path.is_file():
        raise RuntimeError(f"Missing {description}: {path}")


def stage_payload(build_dir: Path, staging_dir: Path, include_bwrap: bool) -> None:
    codex_name = host_executable_name("codex")
    codex_src = build_dir / codex_name
    require_file(codex_src, "codex binary")
    shutil.copy2(codex_src, staging_dir / codex_name)

    if include_bwrap:
        bwrap_src = build_dir / host_executable_name("bwrap")
        require_file(bwrap_src, "bwrap binary")
        resources_dir = staging_dir / "codex-resources"
        resources_dir.mkdir(parents=True, exist_ok=True)
        shutil.copy2(bwrap_src, resources_dir / host_executable_name("bwrap"))

    runner = staging_dir / RUNNER_NAME
    runner.write_text(
        "\n".join(
            [
                "#!/bin/sh",
                "set -eu",
                f": > {COMPLETE_SENTINEL}",
                f'exec ./{codex_name} "$@"',
                "",
            ]
        ),
        encoding="utf-8",
    )
    runner.chmod(0o755)


def iter_staged_files(staging_dir: Path) -> list[Path]:
    return sorted(path for path in staging_dir.rglob("*") if path.is_file())


def hash_staged_tree(staging_dir: Path) -> str:
    digest = hashlib.sha256()
    for path in iter_staged_files(staging_dir):
        relative_path = path.relative_to(staging_dir).as_posix()
        mode = stat.S_IMODE(path.stat().st_mode)
        digest.update(relative_path.encode("utf-8"))
        digest.update(b"\0")
        digest.update(f"{mode:o}".encode("ascii"))
        digest.update(b"\0")
        with path.open("rb") as file:
            for chunk in iter(lambda: file.read(1024 * 1024), b""):
                digest.update(chunk)
        digest.update(b"\0")
    return digest.hexdigest()


def infer_makeself_header(makeself: str) -> Path:
    makeself_path = shutil.which(makeself)
    if makeself_path is None:
        candidate = Path(makeself)
        if candidate.is_file():
            makeself_path = str(candidate)
        else:
            raise RuntimeError(f"Unable to find makeself executable: {makeself}")

    resolved = Path(makeself_path).resolve()
    candidates = [
        resolved.parent / "makeself-header.sh",
        resolved.parent.parent / "libexec" / "makeself-header.sh",
        resolved.parent.parent / "share" / "makeself" / "makeself-header.sh",
        Path("/usr/libexec/makeself-header.sh"),
        Path("/usr/share/makeself/makeself-header.sh"),
        Path("/usr/lib/makeself/makeself-header.sh"),
    ]
    for candidate in candidates:
        if candidate.is_file():
            return candidate

    raise RuntimeError(
        "Unable to infer makeself-header.sh. Pass --makeself-header with its path."
    )


def write_cached_makeself_header(source_header: Path, output_header: Path) -> None:
    header = source_header.read_text(encoding="utf-8")
    marker = 'if test x"\\$targetdir" = x.; then'
    if marker not in header:
        raise RuntimeError(f"Unable to patch Makeself header; marker not found in {source_header}")
    header = pass_codex_options_through(header, source_header)

    cache_fast_path = f"""
# Codex dev artifacts use content-addressed --target directories. On cache hits,
# run the existing extraction instead of unpacking the payload again.
if test x"\\$keep" = xy -a x"\\$script" != x -a -f "\\$targetdir/{COMPLETE_SENTINEL}"; then
    cd "\\$targetdir" || {{
        echo "Cannot enter cached target directory \\$targetdir" >&2
        exit 1
    }}
    if test x"\\$quiet" = xn; then
        echo "Using cached extraction in \\$targetdir"
    fi
    res=0
    if test x"\\$verbose" = xy; then
        MS_Printf "OK to execute: \\$script \\$scriptargs \\$* ? [Y/n] "
        read yn
        if test x"\\$yn" = x -o x"\\$yn" = xy -o x"\\$yn" = xY; then
            eval "\\"\\$script\\" \\$scriptargs "\\\\\\$@""; res=\\$?
        fi
    else
        eval "\\"\\$script\\" \\$scriptargs "\\\\\\$@""; res=\\$?
    fi
    exit \\$res
fi

"""
    output_header.write_text(header.replace(marker, cache_fast_path + marker), encoding="utf-8")


def pass_codex_options_through(header: str, source_header: Path) -> str:
    header = header.replace("-h | --help)", "--makeself-help)")
    header = header.replace(
        "\\$0 --help   Print this message",
        "\\$0 --makeself-help   Print this message",
    )

    unrecognized_flag_block = """    -*)
\techo Unrecognized flag : "\\$1" >&2
\tMS_Help
\texit 1
\t;;"""
    if unrecognized_flag_block not in header:
        raise RuntimeError(
            f"Unable to patch Makeself option parser; marker not found in {source_header}"
        )

    return header.replace(
        unrecognized_flag_block,
        """    -*)
\tbreak
\t;;""",
    )


def build_archive(
    makeself: str,
    header: Path,
    staging_dir: Path,
    output_path: Path,
    target_dir: str,
    tree_hash: str,
) -> None:
    output_path.parent.mkdir(parents=True, exist_ok=True)
    label = f"Codex dev build {tree_hash[:12]}"
    cmd = [
        makeself,
        "--sha256",
        "--packaging-date",
        f"content-sha256:{tree_hash}",
        "--header",
        str(header),
        "--target",
        target_dir,
        str(staging_dir),
        str(output_path),
        label,
        f"./{RUNNER_NAME}",
    ]
    run_command(cmd, cwd=REPO_ROOT)


def default_output_path() -> Path:
    return REPO_ROOT / "dist" / "codex-dev" / "codex-dev.run"


def main() -> int:
    args = parse_args()
    validate_cache_root(args.cache_root)
    include_bwrap = should_include_bwrap(args.include_bwrap)
    build_binaries(args.profile, include_bwrap, args.skip_cargo_build)

    build_dir = cargo_profile_output_dir(args.profile)
    output_path = args.output or default_output_path()
    makeself_header = args.makeself_header or infer_makeself_header(args.makeself)
    require_file(makeself_header, "makeself header")

    with tempfile.TemporaryDirectory(prefix="codex-makeself-") as temp_root_name:
        temp_root = Path(temp_root_name)
        staging_dir = temp_root / "payload"
        staging_dir.mkdir()
        patched_header = temp_root / "makeself-header-codex-cache.sh"

        stage_payload(build_dir, staging_dir, include_bwrap)
        tree_hash = hash_staged_tree(staging_dir)
        target_dir = f"{args.cache_root.rstrip('/')}/sha256-{tree_hash}"
        write_cached_makeself_header(makeself_header, patched_header)
        build_archive(
            args.makeself,
            patched_header,
            staging_dir,
            output_path,
            target_dir,
            tree_hash,
        )

        if args.keep_staging_dir:
            kept_staging_dir = output_path.parent / f"payload-sha256-{tree_hash[:12]}"
            if kept_staging_dir.exists():
                shutil.rmtree(kept_staging_dir)
            shutil.copytree(staging_dir, kept_staging_dir, copy_function=shutil.copy2)
            print(f"Kept staged payload at {kept_staging_dir}")

    print(f"Wrote {output_path}")
    print(f"Payload sha256: {tree_hash}")
    print(f"Runtime cache target: {target_dir}")
    if not include_bwrap:
        print("bwrap was not bundled; pass --include-bwrap=always to require it.")
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except RuntimeError as exc:
        print(f"error: {exc}", file=sys.stderr)
        sys.exit(1)
