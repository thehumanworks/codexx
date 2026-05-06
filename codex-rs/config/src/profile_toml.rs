use codex_utils_absolute_path::AbsolutePathBuf;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;

use crate::config_toml::ToolsToml;
use crate::types::AnalyticsConfigToml;
use crate::types::ApprovalsReviewer;
use crate::types::Personality;
use crate::types::WindowsToml;
use codex_features::FeaturesToml;
use codex_protocol::config_types::ReasoningSummary;
use codex_protocol::config_types::SandboxMode;
use codex_protocol::config_types::ServiceTier;
use codex_protocol::config_types::Verbosity;
use codex_protocol::config_types::WebSearchMode;
use codex_protocol::openai_models::ReasoningEffort;
use codex_protocol::protocol::AskForApproval;

macro_rules! define_config_profile_struct {
    (
        $name:ident => $lenient_name:ident {
            $(
                $kind:ident $(($lenient_ty:ident))? {
                    $(#[$meta:meta])*
                    pub $field:ident: $ty:ty,
                }
            )*
        }
    ) => {
        /// Collection of common configuration options that a user can define as a unit
        /// in `config.toml`.
        #[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
        #[schemars(deny_unknown_fields)]
        pub struct $name {
            $(
                $(#[$meta])*
                pub $field: $ty,
            )*
        }
    };
}

// Single source of truth for profile fields. The lenient loader reuses entries
// marked as `enum`, `section`, or `map` to produce startup warnings.
macro_rules! config_profile_fields {
    ($callback:ident) => {
        $callback! {
            ConfigProfile => LenientConfigProfile {
            direct {
                pub model: Option<String>,
            }
            enum {
                /// Optional explicit service tier preference for new turns (`fast` or `flex`).
                pub service_tier: Option<ServiceTier>,
            }
            direct {
                /// The key in the `model_providers` map identifying the
                /// [`ModelProviderInfo`] to use.
                pub model_provider: Option<String>,
            }
            enum {
                pub approval_policy: Option<AskForApproval>,
            }
            enum {
                pub approvals_reviewer: Option<ApprovalsReviewer>,
            }
            enum {
                pub sandbox_mode: Option<SandboxMode>,
            }
            enum {
                pub model_reasoning_effort: Option<ReasoningEffort>,
            }
            enum {
                pub plan_mode_reasoning_effort: Option<ReasoningEffort>,
            }
            enum {
                pub model_reasoning_summary: Option<ReasoningSummary>,
            }
            enum {
                pub model_verbosity: Option<Verbosity>,
            }
            direct {
                /// Optional path to a JSON model catalog (applied on startup only).
                pub model_catalog_json: Option<AbsolutePathBuf>,
            }
            enum {
                pub personality: Option<Personality>,
            }
            direct {
                pub chatgpt_base_url: Option<String>,
            }
            direct {
                /// Optional path to a file containing model instructions.
                pub model_instructions_file: Option<AbsolutePathBuf>,
            }
            direct {
                /// Deprecated: ignored.
                #[schemars(skip)]
                pub js_repl_node_path: Option<AbsolutePathBuf>,
            }
            direct {
                /// Deprecated: ignored.
                #[schemars(skip)]
                pub js_repl_node_module_dirs: Option<Vec<AbsolutePathBuf>>,
            }
            direct {
                /// Optional absolute path to patched zsh used by zsh-exec-bridge-backed shell execution.
                pub zsh_path: Option<AbsolutePathBuf>,
            }
            direct {
                /// Deprecated: ignored. Use `model_instructions_file`.
                #[schemars(skip)]
                pub experimental_instructions_file: Option<AbsolutePathBuf>,
            }
            direct {
                pub experimental_compact_prompt_file: Option<AbsolutePathBuf>,
            }
            direct {
                pub include_apply_patch_tool: Option<bool>,
            }
            direct {
                pub include_permissions_instructions: Option<bool>,
            }
            direct {
                pub include_apps_instructions: Option<bool>,
            }
            direct {
                pub include_environment_context: Option<bool>,
            }
            direct {
                pub experimental_use_unified_exec_tool: Option<bool>,
            }
            direct {
                pub experimental_use_freeform_apply_patch: Option<bool>,
            }
            direct {
                pub tools_view_image: Option<bool>,
            }
            section(LenientToolsToml) {
                pub tools: Option<ToolsToml>,
            }
            enum {
                pub web_search: Option<WebSearchMode>,
            }
            direct {
                pub analytics: Option<AnalyticsConfigToml>,
            }
            section(LenientWindowsToml) {
                #[serde(default)]
                pub windows: Option<WindowsToml>,
            }
            direct {
                /// Optional feature toggles scoped to this profile.
                #[serde(default)]
                // Injects known feature keys into the schema and forbids unknown keys.
                #[schemars(schema_with = "crate::schema::features_schema")]
                pub features: Option<FeaturesToml>,
            }
            direct {
                pub oss_provider: Option<String>,
            }
            }
        }
    };
}

pub(crate) use config_profile_fields;

config_profile_fields!(define_config_profile_struct);

impl From<ConfigProfile> for codex_app_server_protocol::Profile {
    fn from(config_profile: ConfigProfile) -> Self {
        Self {
            model: config_profile.model,
            model_provider: config_profile.model_provider,
            approval_policy: config_profile.approval_policy,
            model_reasoning_effort: config_profile.model_reasoning_effort,
            model_reasoning_summary: config_profile.model_reasoning_summary,
            model_verbosity: config_profile.model_verbosity,
            chatgpt_base_url: config_profile.chatgpt_base_url,
        }
    }
}
