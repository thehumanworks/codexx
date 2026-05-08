#!/usr/bin/env bash

load_bazel_ci_config() {
  local config_path="${CODEX_BAZEL_CI_CONFIG:-}"
  if [[ -z "$config_path" ]]; then
    return
  fi

  if [[ ! -f "$config_path" ]]; then
    echo "Bazel CI config file not found: $config_path" >&2
    exit 1
  fi

  local name
  local value
  while IFS=$'\t' read -r name value; do
    [[ -n "$name" ]] || continue
    value="${value%$'\r'}"
    case "$name" in
      BAZEL_REPOSITORY_CACHE | \
      BAZEL_OUTPUT_USER_ROOT | \
      BAZEL_REPO_CONTENTS_CACHE | \
      CODEX_BAZEL_EXECUTION_LOG_COMPACT_DIR | \
      CODEX_BAZEL_WINDOWS_PATH | \
      INCLUDE | \
      LIB | \
      LIBPATH | \
      UCRTVersion | \
      UniversalCRTSdkDir | \
      VCINSTALLDIR | \
      VCToolsInstallDir | \
      WindowsLibPath | \
      WindowsSdkBinPath | \
      WindowsSdkDir | \
      WindowsSDKLibVersion | \
      WindowsSDKVersion)
        if [[ -z "${!name:-}" ]]; then
          printf -v "$name" '%s' "$value"
          export "$name"
        fi
        ;;
      *)
        echo "Unknown Bazel CI config key: $name" >&2
        exit 1
        ;;
    esac
  done < "$config_path"
}
