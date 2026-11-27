use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionFileSpec {
    pub path: String,
    /// tauri_json | cargo_toml | package_json | pyproject_toml | csproj | pubspec_yaml | regex
    pub format: String,
    #[serde(default)]
    pub pattern: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionSpec {
    pub id: String,
    pub label: String,
    /// test | build | package | publish | maintenance | custom
    pub category: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub cwd: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub dangerous: bool,
    #[serde(default)]
    pub supports_bump: bool,
    #[serde(default)]
    pub supports_channel: bool,
    #[serde(default)]
    pub supports_skip_tests: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoBinding {
    /// github | gitee | gitlab | gitea
    #[serde(default = "default_provider_github")]
    pub provider: String,
    /// Empty for default cloud host (github.com / gitee.com / gitlab.com), or custom base URL e.g. https://git.corp.local
    #[serde(default)]
    pub base_url: String,
    /// owner/repo (or group/subgroup/project for GitLab)
    #[serde(default)]
    pub repo_slug: String,
    /// Credential ID in encrypted vault (optional; falls back to provider default or gh CLI)
    #[serde(default)]
    pub credential_id: String,
}

impl Default for RepoBinding {
    fn default() -> Self {
        Self {
            provider: default_provider_github(),
            base_url: String::new(),
            repo_slug: String::new(),
            credential_id: String::new(),
        }
    }
}

fn default_provider_github() -> String {
    "github".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageProfile {
    pub id: String,
    pub name: String,
    /// zip | targz
    #[serde(default = "default_zip_format")]
    pub format: String,
    #[serde(default)]
    pub name_template: String,
    #[serde(default)]
    pub sources: Vec<String>,
    #[serde(default)]
    pub excludes: Vec<String>,
    #[serde(default)]
    pub split_size_mb: Option<u64>,
    #[serde(default)]
    pub include_readme: bool,
    #[serde(default)]
    pub readme_content: String,
}

fn default_zip_format() -> String {
    "zip".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GuardrailSettings {
    /// Minimum required free disk buffer in MB before build/pack/download (default 2048 MB)
    #[serde(default = "default_min_disk_mb")]
    pub min_free_disk_mb: u64,
    /// Safety multiplier on estimated output size (default 3x)
    #[serde(default = "default_disk_multiplier")]
    pub disk_headroom_multiplier: u64,
    /// Max worker threads for hashing/packing/scanning (0 = auto cores-1 capped at 8)
    #[serde(default)]
    pub max_concurrency: usize,
    /// Block packing sensitive files (.env, *.pem, *.key, etc.) unless explicitly allowed
    #[serde(default = "default_true")]
    pub block_sensitive_files: bool,
    /// Auto-rollback on cancel or error during mutating pipeline
    #[serde(default = "default_true")]
    pub auto_rollback_on_abort: bool,
    /// Default split threshold warning in MB (e.g. 95 for Gitee, 1900 for GitHub)
    #[serde(default = "default_split_warn_mb")]
    pub split_warning_mb: u64,
}

fn default_min_disk_mb() -> u64 {
    2048
}
fn default_disk_multiplier() -> u64 {
    3
}
fn default_split_warn_mb() -> u64 {
    95
}

impl Default for GuardrailSettings {
    fn default() -> Self {
        Self {
            min_free_disk_mb: default_min_disk_mb(),
            disk_headroom_multiplier: default_disk_multiplier(),
            max_concurrency: 0,
            block_sensitive_files: true,
            auto_rollback_on_abort: true,
            split_warning_mb: default_split_warn_mb(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ProjectConfig {
    pub id: String,
    pub name: String,
    pub root: String,
    /// tauri | rust | electron | node | python | go | dotnet | flutter | cpp | java | wails | nsis | custom
    pub kind: String,
    pub adapter_label: String,
    #[serde(default)]
    pub primary_language: String,
    #[serde(default)]
    pub detected_languages: Vec<String>,
    #[serde(default)]
    pub shell_type: String,
    #[serde(default)]
    pub product_name: String,
    #[serde(default)]
    pub artifact_prefix: String,
    #[serde(default)]
    pub release_dir: String,
    #[serde(default)]
    pub changelog_path: String,
    #[serde(default)]
    pub github_repo: String,
    #[serde(default)]
    pub repo_binding: RepoBinding,
    #[serde(default = "default_keep_releases")]
    pub keep_releases: usize,
    #[serde(default)]
    pub launch_exe: String,
    #[serde(default)]
    pub lock_file: String,
    #[serde(default)]
    pub journal_file: String,
    #[serde(default)]
    pub has_build_ps1: bool,
    #[serde(default)]
    pub has_publish_ps1: bool,
    #[serde(default)]
    pub engine_src_dir: String,
    #[serde(default)]
    pub engine_exe_path: String,
    #[serde(default)]
    pub version_files: Vec<VersionFileSpec>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default)]
    pub actions: Vec<ActionSpec>,
    #[serde(default)]
    pub package_profiles: Vec<PackageProfile>,
}

fn default_keep_releases() -> usize {
    2
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdaterConfig {
    #[serde(default = "default_updater_channel")]
    pub channel: String,
    #[serde(default = "default_true")]
    pub auto_check: bool,
    #[serde(default)]
    pub ignored_version: String,
    #[serde(default)]
    pub last_check_at: String,
}

fn default_updater_channel() -> String {
    "stable".to_string()
}

impl Default for UpdaterConfig {
    fn default() -> Self {
        Self {
            channel: default_updater_channel(),
            auto_check: true,
            ignored_version: String::new(),
            last_check_at: String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AppConfig {
    #[serde(default)]
    pub config_version: u32,
    pub active_project_id: Option<String>,
    pub projects: Vec<ProjectConfig>,
    #[serde(default)]
    pub default_port: u16,
    #[serde(default = "default_true")]
    pub auto_watch: bool,
    #[serde(default)]
    pub scan_roots: Vec<String>,
    #[serde(default)]
    pub onboarding_completed: bool,
    #[serde(default)]
    pub guardrails: GuardrailSettings,
    #[serde(default)]
    pub updater: UpdaterConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveredProject {
    pub path: String,
    pub name: String,
    pub kind: String,
    pub adapter_label: String,
    pub version: String,
    pub github_repo: String,
    pub already_imported: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageInfo {
    pub data_root: String,
    pub config_dir: String,
    pub tools_dir: String,
    pub cache_dir: String,
    pub is_ascii: bool,
    pub is_c_drive: bool,
    pub total_bytes: u64,
    pub managed_tool_count: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionBucket {
    pub version: String,
    #[serde(default)]
    pub base_git_sha: String,
    #[serde(default)]
    pub last_built_git_sha: String,
    #[serde(default)]
    pub last_built_fingerprint: String,
    #[serde(default)]
    pub last_built_at: String,
    #[serde(default)]
    pub build_count: u32,
    #[serde(default)]
    pub notes_draft: String,
    #[serde(default)]
    pub changed_files: Vec<String>,
    #[serde(default)]
    pub artifact_hashes: HashMap<String, String>,
    #[serde(default)]
    pub sealed: bool,
    #[serde(default)]
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClassifiedChangeItem {
    pub path: String,
    pub status: String,
    /// source | engine | config | docs | artifact | other
    pub category: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeployReadiness {
    /// "ready_to_deploy" | "needs_build" | "published" | "partial_artifacts"
    pub status: String,
    pub version: String,
    pub headline: String,
    pub detail: String,
    pub artifacts_ready: bool,
    pub artifact_files: Vec<String>,
    pub artifact_total_bytes: u64,
    pub newest_artifact_unix: u64,
    pub manifest_exists: bool,
    pub manifest_aligned: bool,
    pub checksums_exists: bool,
    pub checksums_aligned: bool,
    pub source_newer_than_artifacts: bool,
    pub stale_reason: String,
    pub git_is_repo: bool,
    pub git_dirty_count: usize,
    pub git_ahead_count: usize,
    pub git_has_release_tag: bool,
    pub git_needs_push: bool,
    pub repo_bound: bool,
    pub already_published: bool,
    pub pending_actions: Vec<String>,
    pub blockers: Vec<String>,
}

fn default_deploy_true() -> bool {
    true
}

fn default_deploy_channel() -> String {
    "stable".into()
}

fn default_deploy_git_mode() -> String {
    "tag_push".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeployPushRequest {
    pub project_id: String,
    pub version: String,
    #[serde(default = "default_deploy_channel")]
    pub channel: String,
    #[serde(default = "default_deploy_true")]
    pub ensure_manifest: bool,
    #[serde(default = "default_deploy_true")]
    pub verify_checksums: bool,
    /// "none" | "tag_push" | "commit_tag_push"
    #[serde(default = "default_deploy_git_mode")]
    pub git_mode: String,
    #[serde(default)]
    pub git_commit_message: String,
    #[serde(default = "default_deploy_true")]
    pub upload_release: bool,
    #[serde(default)]
    pub draft: bool,
    #[serde(default = "default_deploy_true")]
    pub overwrite_assets: bool,
    #[serde(default)]
    pub release_notes: String,
    #[serde(default)]
    pub artifact_files: Vec<String>,
    #[serde(default)]
    pub force_stale_source: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangeReport {
    pub project_id: String,
    #[serde(default)]
    pub project_name: String,
    /// git | filesystem
    #[serde(default)]
    pub scan_mode: String,
    pub current_version: String,
    pub head_sha: String,
    pub dirty_count: usize,
    pub dirty_files: Vec<String>,
    #[serde(default)]
    pub classified_changes: Vec<ClassifiedChangeItem>,
    #[serde(default)]
    pub category_counts: HashMap<String, usize>,
    #[serde(default)]
    pub change_summary: String,
    #[serde(default)]
    pub artifact_changed: bool,
    #[serde(default)]
    pub source_fingerprint: String,
    #[serde(default)]
    pub artifact_fingerprint: String,
    pub engine_stale: bool,
    pub artifact_count: usize,
    pub fingerprint: String,
    pub version_changed: bool,
    pub has_external_change: bool,
    pub changed_since_last_build: bool,
    pub consecutive_warning: String,
    pub bucket: VersionBucket,
    #[serde(default)]
    pub deploy_readiness: Option<DeployReadiness>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionReading {
    pub path: String,
    pub format: String,
    pub version: String,
    pub exists: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionState {
    pub current: String,
    pub synced: bool,
    pub readings: Vec<VersionReading>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangelogEntry {
    pub version: String,
    pub heading: String,
    pub body: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangelogData {
    pub path: String,
    pub exists: bool,
    pub entries: Vec<ChangelogEntry>,
    pub raw_head: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedReleaseNotes {
    pub version: String,
    pub notes: String,
    /// "release_doc" | "changelog" | "changelog_range" | "readme" | "manifest" | "draft" | "unreleased" | "git_commits" | "workspace_diff" | "fallback"
    pub source: String,
    pub source_label: String,
    pub is_fallback: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_changelog_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_changelog_notes: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitFileChange {
    pub status: String,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitCommit {
    pub sha: String,
    pub date: String,
    pub author: String,
    pub subject: String,
    pub refs: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitStatus {
    pub is_repo: bool,
    pub branch: String,
    pub head_sha: String,
    pub remote_url: String,
    pub dirty_count: usize,
    #[serde(default)]
    pub ahead_count: usize,
    #[serde(default)]
    pub behind_count: usize,
    #[serde(default)]
    pub has_upstream: bool,
    pub changes: Vec<GitFileChange>,
    pub recent_commits: Vec<GitCommit>,
    pub tags: Vec<String>,
    pub last_tag: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildLockInfo {
    pub path: String,
    pub exists: bool,
    pub alive: bool,
    pub pid: u32,
    pub session: String,
    pub version: String,
    pub mode: String,
    pub started_at: String,
    pub host: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineStaleInfo {
    pub stale: bool,
    pub newer_count: usize,
    pub newest_file: String,
    pub engine_exe_exists: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactInfo {
    pub name: String,
    pub path: String,
    pub size: u64,
    pub modified: String,
    /// installer | portable | manifest | checksum | archive | other
    pub kind: String,
    pub version: Option<String>,
    pub is_archived: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildJournalEntry {
    pub ts: String,
    pub version: String,
    pub mode: String,
    pub result: String,
    pub duration: String,
    pub git: String,
    pub note: String,
    pub source: String,
    pub project_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectOverview {
    pub project: ProjectConfig,
    pub root_exists: bool,
    pub versions: VersionState,
    pub git: GitStatus,
    pub lock: Option<BuildLockInfo>,
    pub engine_stale: Option<EngineStaleInfo>,
    pub artifacts: Vec<ArtifactInfo>,
    pub recent_builds: Vec<BuildJournalEntry>,
    pub launch_exe_exists: bool,
    pub launch_exe_size: u64,
    pub health_issues: Vec<String>,
    pub change_report: ChangeReport,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolchainItem {
    pub id: String,
    pub name: String,
    pub category: String,
    pub available: bool,
    /// ok | corrupted | missing
    pub status: String,
    pub version: String,
    pub path: String,
    pub managed: bool,
    pub essential: bool,
    pub repairable: bool,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskLogEvent {
    pub task_id: String,
    #[serde(default)]
    pub project_id: String,
    pub stream: String,
    pub line: String,
    pub ts: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskProgressEvent {
    pub task_id: String,
    #[serde(default)]
    pub project_id: String,
    pub label: String,
    pub stage: String,
    pub detail: String,
    /// 0..=100
    pub percent: u8,
    pub indeterminate: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskFinishedEvent {
    pub task_id: String,
    pub project_id: String,
    pub action_id: String,
    pub label: String,
    pub success: bool,
    pub exit_code: i32,
    pub duration_ms: u64,
    pub cancelled: bool,
    pub is_pipeline_substep: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PipelineStepEvent {
    pub pipeline_id: String,
    #[serde(default)]
    pub project_id: String,
    pub index: usize,
    pub total: usize,
    pub name: String,
    /// pending | running | ok | failed | skipped
    pub status: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PipelineFinishedEvent {
    pub pipeline_id: String,
    pub project_id: String,
    pub success: bool,
    pub cancelled: bool,
    pub duration_ms: u64,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageRequest {
    pub project_id: String,
    /// zip | targz
    pub format: String,
    pub output_name: String,
    pub sources: Vec<String>,
    pub excludes: Vec<String>,
    pub readme_name: Option<String>,
    pub readme_content: Option<String>,
    #[serde(default)]
    pub split_size_mb: Option<u64>,
    #[serde(default)]
    pub allow_sensitive: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageResult {
    pub output_path: String,
    pub file_name: String,
    pub file_count: usize,
    pub raw_bytes: u64,
    pub compressed_bytes: u64,
    pub sha256: String,
    pub duration_ms: u64,
    #[serde(default)]
    pub split_parts: Vec<String>,
    #[serde(default)]
    pub split_manifest_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestRequest {
    pub project_id: String,
    pub version: String,
    pub channel: String,
    /// github | custom | local
    pub url_mode: String,
    pub github_repo: String,
    pub base_url: String,
    pub local_port: u16,
    pub installer_file: Option<String>,
    pub portable_file: Option<String>,
    pub extra_files: Vec<String>,
    pub min_version: String,
    pub notes: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestResult {
    pub latest_path: String,
    pub sums_path: String,
    pub latest_json: String,
    pub sums_text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChecksumVerifyItem {
    pub file: String,
    pub expected: String,
    pub actual: String,
    /// ok | mismatch | missing | unlisted
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerLogEntry {
    pub ts: String,
    pub method: String,
    pub path: String,
    pub status: u16,
    pub bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalServerStatus {
    pub running: bool,
    #[serde(default)]
    pub project_id: String,
    #[serde(default)]
    pub project_name: String,
    pub port: u16,
    pub base_url: String,
    pub feed_url: String,
    pub dir: String,
    pub rewrite_manifest: bool,
    pub hit_count: u64,
    pub logs: Vec<ServerLogEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GhStatus {
    pub installed: bool,
    pub authenticated: bool,
    pub account: String,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseAssetItem {
    #[serde(default)]
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub download_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GhReleaseItem {
    #[serde(default)]
    pub id: String,
    pub tag_name: String,
    pub name: String,
    #[serde(default)]
    pub body: String,
    pub is_prerelease: bool,
    pub is_draft: bool,
    pub published_at: String,
    pub url: String,
    #[serde(default)]
    pub assets: Vec<ReleaseAssetItem>,
    #[serde(default)]
    pub manifest_synced: bool,
    #[serde(default)]
    pub manifest_sync_message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GhPublishRequest {
    pub project_id: String,
    pub repo: String,
    pub tag: String,
    pub title: String,
    pub notes: String,
    pub files: Vec<String>,
    pub prerelease: bool,
    pub draft: bool,
    pub overwrite_assets: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PipelineRequest {
    pub project_id: String,
    /// none | patch | minor | major | beta | custom
    pub bump_kind: String,
    pub custom_version: Option<String>,
    pub channel: String,
    pub run_tests: bool,
    pub run_build: bool,
    pub build_action_id: String,
    pub generate_manifest: bool,
    pub upload_github: bool,
    pub github_repo: String,
    pub release_notes: String,
    pub draft: bool,
    /// none | commit_tag | commit_tag_push
    #[serde(default)]
    pub git_mode: String,
    #[serde(default)]
    pub git_commit_message: String,
    #[serde(default)]
    pub repo_provider: String,
    #[serde(default)]
    pub repo_base_url: String,
    #[serde(default)]
    pub credential_id: String,
}

// ============================================================
// ShipForge v2.0 Extended Models
// ============================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DetectedLanguage {
    pub id: String,
    pub name: String,
    pub confidence: u8,
    pub evidence: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DetectedShell {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub confidence: u8,
    pub output_formats: Vec<String>,
    pub evidence: Vec<String>,
    pub recommended: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectSubmodule {
    pub name: String,
    pub rel_path: String,
    pub language: String,
    pub shell: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectProfileMatrix {
    pub project_id: String,
    pub primary_language: String,
    pub primary_shell: String,
    pub languages: Vec<DetectedLanguage>,
    pub shells: Vec<DetectedShell>,
    pub submodules: Vec<ProjectSubmodule>,
    pub supported_languages: Vec<DetectedLanguage>,
    pub supported_shells: Vec<DetectedShell>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrereqItem {
    pub id: String,
    /// toolchain | project | publish
    pub tier: String,
    pub name: String,
    /// ok | warn | missing | corrupted
    pub status: String,
    pub required: bool,
    pub reason: String,
    pub detail: String,
    pub repairable: bool,
    /// portable | project_dep | system_sdk | config_fix | manual
    pub repair_strategy: String,
    pub target_path: String,
    pub estimated_mb: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrereqReport {
    pub project_id: String,
    pub language: String,
    pub shell: String,
    pub ready: bool,
    pub missing_required: usize,
    pub warnings: usize,
    pub items: Vec<PrereqItem>,
    pub headroom: HeadroomSnapshot,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepairPlan {
    pub project_id: String,
    pub items: Vec<PrereqItem>,
    pub total_estimated_mb: u64,
    pub disk_free_mb: u64,
    pub disk_ok: bool,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepairResult {
    pub succeeded: Vec<String>,
    pub failed: Vec<String>,
    pub report: PrereqReport,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileCategoryStat {
    pub category: String,
    pub label: String,
    pub file_count: usize,
    pub total_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LargeFileEntry {
    pub rel_path: String,
    pub size_bytes: u64,
    pub category: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SensitiveWarning {
    pub rel_path: String,
    pub reason: String,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectFileAnalysis {
    pub project_id: String,
    pub total_files: usize,
    pub total_bytes: u64,
    pub skipped_dirs: Vec<String>,
    pub categories: Vec<FileCategoryStat>,
    pub largest_files: Vec<LargeFileEntry>,
    pub sensitive_warnings: Vec<SensitiveWarning>,
    #[serde(default)]
    pub truncated: bool,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClassifiedArtifact {
    pub artifact: ArtifactInfo,
    /// pending_verify | verified | ready | published | archived
    pub stage: String,
    pub stage_label: String,
    pub sha256_short: String,
    pub published_url: String,
    pub published_provider: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoCredentialMeta {
    pub id: String,
    /// github | gitee | gitlab | gitea
    pub provider: String,
    pub base_url: String,
    pub label: String,
    pub username: String,
    pub masked_token: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteRepoItem {
    pub full_name: String,
    pub html_url: String,
    pub clone_url: String,
    pub private: bool,
    pub default_branch: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MultiPublishRequest {
    pub project_id: String,
    /// github | gitee | gitlab | gitea
    pub provider: String,
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub credential_id: String,
    pub repo: String,
    pub tag: String,
    pub title: String,
    pub notes: String,
    pub files: Vec<String>,
    pub prerelease: bool,
    pub draft: bool,
    pub overwrite_assets: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateReleaseRequest {
    #[serde(default)]
    pub project_id: String,
    /// github | gitee | gitlab | gitea
    pub provider: String,
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub credential_id: String,
    pub repo: String,
    #[serde(default)]
    pub release_id: String,
    pub tag_name: String,
    pub title: String,
    pub notes: String,
    pub prerelease: bool,
    pub draft: bool,
    #[serde(default)]
    pub sync_latest_json: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncChangelogOnlineRequest {
    pub project_id: String,
    pub version_str: String,
    #[serde(default)]
    pub summary: String,
    pub body: String,
    #[serde(default = "default_true_sync")]
    pub write_changelog: bool,
    #[serde(default = "default_true_sync")]
    pub sync_latest_json: bool,
}

fn default_true_sync() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncChangelogOnlineResult {
    pub tag_name: String,
    pub url: String,
    pub changelog_written: bool,
    pub manifest_synced: bool,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteReleaseRequest {
    pub project_id: String,
    /// github | gitee | gitlab | gitea
    pub provider: String,
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub credential_id: String,
    pub repo: String,
    #[serde(default)]
    pub release_id: String,
    pub tag_name: String,
    #[serde(default)]
    pub delete_remote_tag: bool,
    #[serde(default)]
    pub delete_local_tag: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteReleaseResult {
    pub release_deleted: bool,
    pub remote_tag_deleted: bool,
    pub local_tag_deleted: bool,
    pub summary: String,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteReleaseAssetRequest {
    /// github | gitee | gitlab | gitea
    pub provider: String,
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub credential_id: String,
    pub repo: String,
    #[serde(default)]
    pub release_id: String,
    pub tag_name: String,
    pub asset_id: String,
    #[serde(default)]
    pub asset_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RollbackCheckItem {
    pub item: String,
    pub restored: bool,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RollbackVerification {
    pub tx_id: String,
    pub triggered_by: String,
    pub all_restored: bool,
    pub checks: Vec<RollbackCheckItem>,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CrashRecoveryResult {
    pub recovered_count: usize,
    pub verifications: Vec<RollbackVerification>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HeadroomSnapshot {
    pub target_drive: String,
    pub disk_free_bytes: u64,
    pub disk_total_bytes: u64,
    pub disk_required_bytes: u64,
    pub disk_ok: bool,
    pub mem_avail_bytes: u64,
    pub mem_total_bytes: u64,
    pub mem_ok: bool,
    pub cpu_cores: usize,
    pub safe_concurrency: usize,
    pub cache_on_c_drive: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PerformanceAdvisory {
    /// ok | info | warn
    pub level: String,
    pub title: String,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthCheckItem {
    /// ok | warn | err
    pub status: String,
    pub label: String,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NextStepAction {
    pub id: String,
    pub label: String,
    pub target_page: String,
    pub hint: String,
}

/// Ephemeral one-time integrity & health analysis report.
/// NEVER persisted to disk, logs, or browser storage.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthReport {
    pub report_id: String,
    pub project_id: String,
    pub project_name: String,
    /// pipeline | build | pack | publish | rollback | crash_recovery
    pub operation: String,
    /// ok | rolled_back | warning | error
    pub outcome: String,
    pub version: String,
    pub duration_ms: u64,
    pub summary: String,
    pub rollback_proof: Option<RollbackVerification>,
    pub integrity_checks: Vec<HealthCheckItem>,
    pub consistency_checks: Vec<HealthCheckItem>,
    pub headroom: HeadroomSnapshot,
    pub advisories: Vec<PerformanceAdvisory>,
    pub next_steps: Vec<NextStepAction>,
}
