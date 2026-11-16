//! 造物主 Creator additions on top of the open-source Codex CLI (Apache-2.0).
//!
//! Everything Creator-specific lives in this one file so that new upstream Codex
//! releases can be merged with only a handful of small hook edits elsewhere:
//!
//! * data lives in `~/.creator` (or `CREATOR_HOME`), never in Codex's `~/.codex`;
//! * the default model provider is 龍華樹 (`https://b31.si/lh/v1`), signed in with
//!   紫微星通行证 instead of a ChatGPT account, so no ChatGPT quota gate applies;
//! * `B31.Si-1.3` is a selectable model: a commander model that delegates parts of a
//!   task to the leaderboard leader of each domain through Codex's native sub-agents.

use std::collections::BTreeMap;
use std::io::IsTerminal;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Context;
use anyhow::Result;
use anyhow::anyhow;
use anyhow::bail;
use clap::Args;
use codex_models_manager::model_info::model_info_from_slug;
use codex_protocol::openai_models::ApplyPatchToolType;
use codex_protocol::openai_models::InputModality;
use codex_protocol::openai_models::ModelInfo;
use codex_protocol::openai_models::ModelVisibility;
use codex_protocol::openai_models::ModelsResponse;
use codex_protocol::openai_models::ReasoningEffort;
use codex_protocol::openai_models::ReasoningEffortPreset;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use serde_json::json;

/// 紫微星通行证 (Supabase auth behind the main site's `/api/sb` proxy).
const PASSPORT_API: &str = "https://www.zhenbai.one/api/sb";
/// Public Supabase "anon" key, the same one every 紫微星会 web page ships.
const PASSPORT_ANON_KEY: &str = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJpc3MiOiJzdXBhYmFzZSIsInJlZiI6InpzY211dnd6bWF3bmNvdmt6ZmRtIiwicm9sZSI6ImFub24iLCJpYXQiOjE3ODE5NjQ1MzcsImV4cCI6MjA5NzU0MDUzN30.9Jcgs26ikQoTg-LnK7dMYf5Wv5mjWEyYodOTrTvTKEM";
/// 龍華樹 account API (passport bearer token). `boot` returns the model list with
/// 华林园 leaderboard ranks per board (code / text / agent / vision / search).
const LONGHUA_API: &str = "https://b31.si/lh/api";
/// 龍華樹 OpenAI-compatible API (API key bearer token).
const LONGHUA_V1: &str = "https://b31.si/lh/v1";

pub const B31_SLUG: &str = "b31-si-1.3";
const API_KEY_ENV: &str = "CREATOR_API_KEY";
/// JSON object `{catalog_slug: api_model_slug}` read by core when building requests.
const ALIASES_ENV: &str = "CREATOR_MODEL_ALIASES";
const ORIGINATOR_ENV: &str = "CODEX_INTERNAL_ORIGINATOR_OVERRIDE";

const PASSPORT_FILE: &str = "passport.json";
const PLAN_FILE: &str = "b31.json";
const CATALOG_FILE: &str = "models.json";

// ---------------------------------------------------------------- startup

/// Prepare the process environment. Must run first thing in `main`, before any
/// thread is spawned, because it sets environment variables.
pub fn prepare_environment_if_creator() {
    if !invoked_as_creator() {
        return;
    }
    if let Err(err) = prepare_environment() {
        eprintln!("造物主 Creator：初始化失败：{err:#}");
    }
}

fn invoked_as_creator() -> bool {
    let Some(arg0) = std::env::args_os().next() else {
        return true;
    };
    let stem = Path::new(&arg0)
        .file_stem()
        .map(|s| s.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    stem == "creator" || stem.starts_with("creator-") || stem == "造物主"
}

fn prepare_environment() -> Result<()> {
    let home = creator_home()?;
    set_env("CODEX_HOME", home.as_os_str());
    if std::env::var_os(ORIGINATOR_ENV).is_none() {
        set_env(ORIGINATOR_ENV, "creator".as_ref());
    }
    seed_defaults(&home)?;
    if std::env::var_os(API_KEY_ENV).is_none()
        && let Some(key) = read_passport(&home).api_key.filter(|k| !k.is_empty())
    {
        set_env(API_KEY_ENV, key.as_ref());
    }
    if std::env::var_os(ALIASES_ENV).is_none() {
        let plan = read_plan(&home).unwrap_or_else(default_plan);
        let aliases = json!({ B31_SLUG: plan.commander });
        set_env(ALIASES_ENV, aliases.to_string().as_ref());
    }
    Ok(())
}

fn set_env(key: &str, value: &std::ffi::OsStr) {
    // SAFETY: only called from `prepare_environment_if_creator`, which runs at the very
    // start of `main` while the process is still single-threaded.
    unsafe { std::env::set_var(key, value) };
}

/// `CREATOR_HOME`, or `~/.creator`. Created if missing.
pub fn creator_home() -> Result<PathBuf> {
    let dir = match std::env::var_os("CREATOR_HOME").filter(|v| !v.is_empty()) {
        Some(v) => PathBuf::from(v),
        None => user_home()?.join(".creator"),
    };
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("无法创建数据文件夹 {}", dir.display()))?;
    Ok(dir)
}

fn user_home() -> Result<PathBuf> {
    let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var_os(var)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| anyhow!("找不到用户主目录（{var} 未设置）"))
}

// ---------------------------------------------------------------- B31.Si plan

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RolePick {
    model: String,
    board: String,
    rank: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct B31Plan {
    commander: String,
    commander_board: String,
    commander_rank: Option<u64>,
    roles: BTreeMap<String, RolePick>,
    source: String,
    synced_at: String,
}

struct RoleSpec {
    id: &'static str,
    title: &'static str,
    boards: &'static [&'static str],
    fallback: &'static str,
    brief: &'static str,
}

const ROLES: &[RoleSpec] = &[
    RoleSpec {
        id: "b31_coder",
        title: "编程",
        boards: &["code", "agent"],
        fallback: "claude-opus-5.5",
        brief: "写代码、改代码、调试、跑测试、做网站和 APP。",
    },
    RoleSpec {
        id: "b31_operator",
        title: "电脑与浏览器操控",
        boards: &["agent", "code"],
        fallback: "claude-opus-5.5",
        brief: "多步骤操作：运行命令、调用插件和 MCP 工具、操作浏览器与文件。",
    },
    RoleSpec {
        id: "b31_researcher",
        title: "世界知识与检索",
        boards: &["search", "text"],
        fallback: "claude-fable-5.1",
        brief: "查资料、核对事实、整理长文档和数据。",
    },
    RoleSpec {
        id: "b31_writer",
        title: "文案与创意写作",
        boards: &["text"],
        fallback: "claude-fable-5.1",
        brief: "写文章、文案、脚本、翻译和润色。",
    },
    RoleSpec {
        id: "b31_vision",
        title: "看图与视觉设计",
        boards: &["vision"],
        fallback: "claude-fable-5.1",
        brief: "读图、截图分析、界面与视觉设计建议。",
    },
    RoleSpec {
        id: "b31_reviewer",
        title: "交叉审校",
        boards: &["text", "code"],
        fallback: "claude-fable-5.1",
        brief: "独立复查其他专家的成果，找出错误和遗漏。",
    },
];

/// The commander drives an agent harness (tools, files, terminal), so the agent board
/// decides first; ties fall back to the overall text board.
const COMMANDER_BOARDS: &[&str] = &["agent", "text", "code"];
const COMMANDER_FALLBACK: &str = "claude-opus-5.5";

fn default_plan() -> B31Plan {
    B31Plan {
        commander: COMMANDER_FALLBACK.to_string(),
        commander_board: "text".to_string(),
        commander_rank: None,
        roles: ROLES
            .iter()
            .map(|r| {
                (
                    r.id.to_string(),
                    RolePick {
                        model: r.fallback.to_string(),
                        board: r.boards[0].to_string(),
                        rank: None,
                    },
                )
            })
            .collect(),
        source: "内置默认（尚未同步排行榜）".to_string(),
        synced_at: String::new(),
    }
}

/// One model entry from `/lh/api/boot` (passport) — only the fields we need.
#[derive(Debug, Clone)]
struct RankedModel {
    id: String,
    ranks: BTreeMap<String, u64>,
    /// Zero-priced lines (free key rotations) rank after paid official lines on ties,
    /// because they are rate-limited and not meant to carry paid work.
    free: bool,
    sort: i64,
}

fn parse_ranked_models(v: &Value) -> Vec<RankedModel> {
    let list = v
        .as_array()
        .or_else(|| v.get("models").and_then(Value::as_array))
        .or_else(|| v.get("data").and_then(Value::as_array));
    let Some(list) = list else {
        return Vec::new();
    };
    list.iter()
        .filter(|m| m.get("available").and_then(Value::as_bool) != Some(false))
        .filter(|m| {
            m.get("kind")
                .and_then(Value::as_str)
                .is_none_or(|k| k == "chat")
        })
        .filter_map(|m| {
            let id = m.get("id").and_then(Value::as_str)?.to_string();
            let ranks = m
                .get("ranks")
                .and_then(Value::as_object)
                .map(|o| {
                    o.iter()
                        .filter_map(|(k, v)| {
                            v.as_u64()
                                .or_else(|| v.as_f64().map(|f| f as u64))
                                .filter(|r| *r > 0)
                                .map(|r| (k.clone(), r))
                        })
                        .collect()
                })
                .unwrap_or_default();
            let free = m.get("p_in").and_then(Value::as_f64) == Some(0.0)
                && m.get("p_out").and_then(Value::as_f64) == Some(0.0);
            let sort = m.get("sort").and_then(Value::as_i64).unwrap_or(0);
            Some(RankedModel { id, ranks, free, sort })
        })
        .collect()
}

/// Best available model for the first board (in order) that has any ranked model.
fn pick_leader<'a>(
    ranked: &'a [RankedModel],
    boards: &[&str],
    usable: &dyn Fn(&str) -> bool,
    exclude: Option<&str>,
) -> Option<(&'a RankedModel, String, u64)> {
    for board in boards {
        let best = ranked
            .iter()
            .filter(|m| usable(&m.id) && Some(m.id.as_str()) != exclude)
            .filter_map(|m| m.ranks.get(*board).map(|r| (m, *r)))
            .min_by_key(|(m, r)| (*r, m.free, m.sort));
        if let Some((m, r)) = best {
            return Some((m, (*board).to_string(), r));
        }
    }
    None
}

fn build_plan(ranked: &[RankedModel], usable: &dyn Fn(&str) -> bool) -> B31Plan {
    let mut plan = default_plan();
    if ranked.is_empty() {
        return plan;
    }
    if let Some((m, board, rank)) = pick_leader(ranked, COMMANDER_BOARDS, usable, None) {
        plan.commander = m.id.clone();
        plan.commander_board = board;
        plan.commander_rank = Some(rank);
    }
    let coder = pick_leader(ranked, ROLES[0].boards, usable, None).map(|(m, _, _)| m.id.clone());
    for spec in ROLES {
        // The reviewer must be a different model from the coder so the check is independent.
        let exclude = (spec.id == "b31_reviewer").then_some(coder.as_deref()).flatten();
        if let Some((m, board, rank)) = pick_leader(ranked, spec.boards, usable, exclude) {
            plan.roles.insert(
                spec.id.to_string(),
                RolePick {
                    model: m.id.clone(),
                    board,
                    rank: Some(rank),
                },
            );
        }
    }
    plan.source = "华林园全球 AI 排行榜（龍華樹实时数据）".to_string();
    plan
}

/// Replace any model the endpoint does not serve: role fallback first, then the first
/// served model. Keeps the reviewer independent of the coder when possible.
fn sanitize_plan(plan: &mut B31Plan, usable: &dyn Fn(&str) -> bool, any: Option<&str>) {
    let pick = |want: &str, fallback: &str| -> Option<String> {
        [want, fallback]
            .into_iter()
            .chain(any)
            .find(|m| usable(m))
            .map(str::to_string)
    };
    if let Some(m) = pick(&plan.commander.clone(), COMMANDER_FALLBACK) {
        if m != plan.commander {
            plan.commander_rank = None;
        }
        plan.commander = m;
    }
    for spec in ROLES {
        let entry = plan.roles.entry(spec.id.to_string()).or_insert_with(|| RolePick {
            model: spec.fallback.to_string(),
            board: spec.boards[0].to_string(),
            rank: None,
        });
        if let Some(m) = pick(&entry.model.clone(), spec.fallback)
            && m != entry.model
        {
            entry.model = m;
            entry.rank = None;
        }
    }
    let coder = plan.roles.get("b31_coder").map(|p| p.model.clone());
    if let (Some(coder), Some(reviewer)) = (coder, plan.roles.get_mut("b31_reviewer"))
        && reviewer.model == coder
    {
        let alt = ["claude-fable-5.1", "claude-opus-5", "claude-sonnet-5.5", "claude-opus-5.5"]
            .into_iter()
            .find(|m| *m != coder && usable(m));
        if let Some(alt) = alt {
            reviewer.model = alt.to_string();
            reviewer.rank = None;
        }
    }
}

fn read_plan(home: &Path) -> Option<B31Plan> {
    let text = std::fs::read_to_string(home.join(PLAN_FILE)).ok()?;
    serde_json::from_str(&text).ok()
}

// ---------------------------------------------------------------- catalog

/// One entry of the public `/lh/v1/models` list.
#[derive(Debug, Clone, Deserialize)]
struct GatewayModel {
    id: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    owned_by: Option<String>,
    #[serde(default)]
    vision: Option<bool>,
    #[serde(default)]
    reasoning_efforts: Vec<String>,
}

/// Models known to exist on 龍華樹 when no network is available yet.
fn builtin_gateway_models() -> Vec<GatewayModel> {
    let all = ["low", "medium", "high", "xhigh", "max"];
    let three = ["low", "medium", "high"];
    // Only models the 龍華樹 Responses endpoint serves today (Codex speaks Responses only).
    let _ = three;
    let rows: &[(&str, &str, &str, bool, &[&str])] = &[
        ("claude-opus-5.5", "Claude Opus 5.5", "Anthropic", true, &all),
        ("claude-fable-5.1", "Claude Fable 5.1", "Anthropic", true, &all),
        ("claude-sonnet-5.5", "Claude Sonnet 5.5", "Anthropic", true, &all),
        ("claude-opus-5", "Claude Opus 5", "Anthropic", true, &all),
        ("claude-haiku-5.5", "Claude Haiku 5.5", "Anthropic", true, &all),
    ];
    rows.iter()
        .map(|(id, name, owner, vision, efforts)| GatewayModel {
            id: (*id).to_string(),
            name: Some((*name).to_string()),
            owned_by: Some((*owner).to_string()),
            vision: Some(*vision),
            reasoning_efforts: efforts.iter().map(|e| (*e).to_string()).collect(),
        })
        .collect()
}

fn effort_description(effort: &str) -> &'static str {
    match effort {
        "low" => "快速回答",
        "medium" => "速度与深度平衡",
        "high" => "深入思考",
        "xhigh" => "极高：复杂任务",
        "max" => "最强：最难的任务",
        _ => "",
    }
}

fn reasoning_presets(efforts: &[String]) -> Vec<ReasoningEffortPreset> {
    efforts
        .iter()
        .filter_map(|e| {
            let effort: ReasoningEffort = e.parse().ok()?;
            Some(ReasoningEffortPreset {
                effort,
                description: effort_description(e).to_string(),
            })
        })
        .collect()
}

fn creator_instructions(base: &str) -> String {
    const UPSTREAM: &str = "You are a coding agent running in the Codex CLI, a terminal-based coding assistant. Codex CLI is an open source project led by OpenAI.";
    const CREATOR: &str = "You are 造物主 Creator, the coding and computer-operating agent of 紫微星会 and 龍華樹 ASI. Creator's engine is built on the open-source Codex CLI (Apache-2.0). Reply in the user's language (usually Simplified Chinese).";
    if base.contains(UPSTREAM) {
        base.replacen(UPSTREAM, CREATOR, 1)
    } else {
        format!("{CREATOR}\n\n{base}")
    }
}

fn model_entry(m: &GatewayModel, priority: i32) -> ModelInfo {
    let mut info = model_info_from_slug(&m.id);
    info.display_name = m.name.clone().unwrap_or_else(|| m.id.clone());
    info.description = m
        .owned_by
        .as_ref()
        .map(|o| format!("{o} · 由龍華樹提供，按龍華樹 API 余额计费"));
    info.supported_reasoning_levels = reasoning_presets(&m.reasoning_efforts);
    info.default_reasoning_level = ["high", "medium", "low"]
        .iter()
        .find(|e| m.reasoning_efforts.iter().any(|x| x == *e))
        .and_then(|e| e.parse().ok());
    info.supports_reasoning_summary_parameter = !m.reasoning_efforts.is_empty();
    info.visibility = ModelVisibility::List;
    info.priority = priority;
    // 龍華樹 converts the freeform apply_patch tool for every upstream provider.
    info.apply_patch_tool_type = Some(ApplyPatchToolType::Freeform);
    info.input_modalities = if m.vision.unwrap_or(false) {
        vec![InputModality::Text, InputModality::Image]
    } else {
        vec![InputModality::Text]
    };
    if let Some(messages) = info.model_messages.as_mut()
        && let Some(template) = messages.instructions_template.as_mut()
    {
        *template = creator_instructions(template);
    }
    info
}

fn commander_prompt(plan: &B31Plan) -> String {
    let mut table = String::new();
    for spec in ROLES {
        let pick = plan.roles.get(spec.id);
        let model = pick.map_or(spec.fallback, |p| p.model.as_str());
        let rank = pick
            .and_then(|p| p.rank.map(|r| format!("（{}榜 #{r}）", p.board)))
            .unwrap_or_default();
        table.push_str(&format!(
            "- `{}` · {} · 模型 {model}{rank}：{}\n",
            spec.id, spec.title, spec.brief
        ));
    }
    format!(
        "# B31.Si-1.3 指挥官模式（万能许愿机）

You are the commander of the B31.Si-1.3 super-intelligence. You run on the strongest available model ({commander}). You lead a team of domain experts; each one is the current #1 model of its leaderboard domain:

{table}
How you work:
1. Simple questions or small edits: just do them yourself, no delegation.
2. Larger tasks: break the request into parts by domain. For each part call `spawn_agent` with `agent_type` set to the matching expert above and a complete, self-contained brief (goal, files, constraints, expected output). Independent parts can run in parallel.
3. Wait for the experts' results, check them yourself, and send important results to `b31_reviewer` for an independent check before you rely on them.
4. Merge the best parts into one finished result and present it to the user. Say briefly which experts did which part.
5. Be honest: if something could not be done or verified, say so plainly instead of claiming success. Never invent results.

Expert table synced from: {source}.",
        commander = plan.commander,
        source = plan.source,
    )
}

fn b31_entry(plan: &B31Plan, commander: Option<&GatewayModel>) -> ModelInfo {
    let mut info = model_info_from_slug(B31_SLUG);
    info.display_name = "B31.Si-1.3 · 万能许愿机".to_string();
    info.description = Some(format!(
        "指挥官 {} 按排行榜调度各领域第一的模型协作完成任务",
        plan.commander
    ));
    let efforts: Vec<String> = commander
        .map(|m| m.reasoning_efforts.clone())
        .filter(|e| !e.is_empty())
        .unwrap_or_else(|| ["low", "medium", "high", "xhigh", "max"].map(String::from).to_vec());
    info.supported_reasoning_levels = reasoning_presets(&efforts);
    info.default_reasoning_level = ["xhigh", "high", "medium"]
        .iter()
        .find(|e| efforts.iter().any(|x| x == *e))
        .and_then(|e| e.parse().ok());
    info.visibility = ModelVisibility::List;
    info.priority = 0;
    info.apply_patch_tool_type = Some(ApplyPatchToolType::Freeform);
    info.input_modalities = if commander.and_then(|m| m.vision).unwrap_or(true) {
        vec![InputModality::Text, InputModality::Image]
    } else {
        vec![InputModality::Text]
    };
    if let Some(messages) = info.model_messages.as_mut()
        && let Some(template) = messages.instructions_template.as_mut()
    {
        *template = format!("{}\n\n{}", creator_instructions(template), commander_prompt(plan));
    }
    info
}

fn build_catalog(models: &[GatewayModel], plan: &B31Plan) -> ModelsResponse {
    let commander = models.iter().find(|m| m.id == plan.commander);
    let mut out = vec![b31_entry(plan, commander)];
    for (i, m) in models.iter().enumerate() {
        out.push(model_entry(m, i as i32 + 1));
    }
    ModelsResponse { models: out }
}

// ---------------------------------------------------------------- seeding

fn toml_literal(path: &Path) -> String {
    // TOML literal strings keep Windows backslashes as-is.
    format!("'{}'", path.display())
}

fn default_config(home: &Path) -> String {
    let catalog = toml_literal(&home.join(CATALOG_FILE));
    let mut roles = String::new();
    for spec in ROLES {
        roles.push_str(&format!(
            "\n[agents.{id}]\ndescription = \"B31.Si {title}专家：{brief}\"\nconfig_file = {file}\n",
            id = spec.id,
            title = spec.title,
            brief = spec.brief,
            file = toml_literal(&home.join("agents").join(format!("{}.toml", spec.id))),
        ));
    }
    format!(
        r#"# 造物主 Creator 配置（首次运行自动生成，可以修改）
# 内核基于开源 Codex（Apache-2.0）。所有数据只放在这个文件夹，不读取 Codex 的 ~/.codex。
model_provider = "longhua"
model = "{B31_SLUG}"
model_reasoning_effort = "xhigh"
model_catalog_json = {catalog}
check_for_update_on_startup = false
web_search = "disabled"

[analytics]
enabled = false

[feedback]
enabled = false

[model_providers.longhua]
name = "龍華樹 · 紫微星通行证"
base_url = "{LONGHUA_V1}"
env_key = "{API_KEY_ENV}"
env_key_instructions = "请先在终端运行 `creator login`，用紫微星通行证登录。"
wire_api = "responses"
requires_openai_auth = false
request_max_retries = 4
stream_max_retries = 5
stream_idle_timeout_ms = 300000

[agents]
max_concurrent_threads_per_session = 6
{roles}"#
    )
}

fn role_file(spec: &RoleSpec, pick: &RolePick) -> String {
    format!(
        "# B31.Si-1.3 · {title}专家（由 `creator sync` 按排行榜生成）\nmodel = \"{model}\"\nmodel_reasoning_effort = \"high\"\ndeveloper_instructions = \"\"\"\n你是 B31.Si-1.3 团队里的{title}专家，由指挥官分派任务。职责：{brief}\n只完成指挥官交给你的部分，交回清楚、可直接使用的结果；做不到或没验证的地方要如实说明。\n\"\"\"\n",
        title = spec.title,
        brief = spec.brief,
        model = pick.model,
    )
}

fn write_atomic(path: &Path, contents: &[u8]) -> Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, contents).with_context(|| format!("写入 {} 失败", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("写入 {} 失败", path.display()))?;
    Ok(())
}

fn write_plan_files(home: &Path, plan: &B31Plan, models: &[GatewayModel]) -> Result<()> {
    let agents = home.join("agents");
    std::fs::create_dir_all(&agents)?;
    for spec in ROLES {
        if let Some(pick) = plan.roles.get(spec.id) {
            write_atomic(
                &agents.join(format!("{}.toml", spec.id)),
                role_file(spec, pick).as_bytes(),
            )?;
        }
    }
    write_atomic(&home.join(PLAN_FILE), serde_json::to_string_pretty(plan)?.as_bytes())?;
    let catalog = build_catalog(models, plan);
    write_atomic(
        &home.join(CATALOG_FILE),
        serde_json::to_string_pretty(&catalog)?.as_bytes(),
    )?;
    Ok(())
}

fn seed_defaults(home: &Path) -> Result<()> {
    let config = home.join("config.toml");
    if !home.join(CATALOG_FILE).exists() || !home.join(PLAN_FILE).exists() {
        let plan = read_plan(home).unwrap_or_else(default_plan);
        write_plan_files(home, &plan, &builtin_gateway_models())?;
    }
    if !config.exists() {
        write_atomic(&config, default_config(home).as_bytes())?;
    }
    Ok(())
}

// ---------------------------------------------------------------- passport

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct PassportFile {
    #[serde(default)]
    user_id: Option<String>,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    api_key: Option<String>,
    #[serde(default)]
    key_id: Option<String>,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    updated_at: Option<String>,
}

fn read_passport(home: &Path) -> PassportFile {
    std::fs::read_to_string(home.join(PASSPORT_FILE))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

fn write_passport(home: &Path, p: &PassportFile) -> Result<()> {
    let path = home.join(PASSPORT_FILE);
    write_atomic(&path, serde_json::to_string_pretty(p)?.as_bytes())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

struct Session {
    access_token: String,
    refresh_token: Option<String>,
}

fn http() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .user_agent(concat!("creator/", env!("CARGO_PKG_VERSION")))
        .build()
        .context("无法创建网络客户端")
}

async fn json_or_error(resp: reqwest::Response, what: &str) -> Result<Value> {
    let status = resp.status();
    let body: Value = resp.json().await.unwrap_or(Value::Null);
    if !status.is_success() {
        let msg = ["msg", "error_description", "message", "error"]
            .iter()
            .find_map(|k| body.get(*k).and_then(Value::as_str))
            .map(str::to_string)
            .unwrap_or_else(|| body.to_string());
        bail!("{what}失败（HTTP {status}）：{msg}");
    }
    Ok(body)
}

fn session_from(body: &Value) -> Result<Session> {
    let access_token = body
        .get("access_token")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("通行证没有返回登录凭证"))?
        .to_string();
    let refresh_token = body
        .get("refresh_token")
        .and_then(Value::as_str)
        .map(str::to_string);
    Ok(Session {
        access_token,
        refresh_token,
    })
}

async fn passport_post(client: &reqwest::Client, path: &str, body: Value, what: &str) -> Result<Value> {
    let resp = client
        .post(format!("{PASSPORT_API}/{path}"))
        .header("apikey", PASSPORT_ANON_KEY)
        .json(&body)
        .send()
        .await
        .with_context(|| format!("{what}：无法连接紫微星通行证"))?;
    json_or_error(resp, what).await
}

async fn refresh_session(client: &reqwest::Client, refresh_token: &str) -> Result<Session> {
    let body = passport_post(
        client,
        "auth/v1/token?grant_type=refresh_token",
        json!({ "refresh_token": refresh_token }),
        "刷新通行证登录",
    )
    .await?;
    session_from(&body)
}

async fn passport_user(client: &reqwest::Client, s: &Session) -> Result<(Option<String>, Option<String>)> {
    let resp = client
        .get(format!("{PASSPORT_API}/auth/v1/user"))
        .header("apikey", PASSPORT_ANON_KEY)
        .bearer_auth(&s.access_token)
        .send()
        .await
        .context("读取通行证账号：网络错误")?;
    let v = json_or_error(resp, "读取通行证账号").await?;
    Ok((
        v.get("id").and_then(Value::as_str).map(str::to_string),
        v.get("email").and_then(Value::as_str).map(str::to_string),
    ))
}

async fn longhua(client: &reqwest::Client, s: &Session, path: &str, body: Option<Value>) -> Result<Value> {
    let url = format!("{LONGHUA_API}/{path}");
    let req = match body {
        Some(b) => client.post(url).json(&b),
        None => client.get(url),
    };
    let resp = req
        .bearer_auth(&s.access_token)
        .send()
        .await
        .with_context(|| format!("龍華樹 {path}：网络错误"))?;
    json_or_error(resp, &format!("龍華樹 {path} ")).await
}

fn prompt_line(label: &str) -> Result<String> {
    eprint!("{label}");
    std::io::stderr().flush().ok();
    let mut s = String::new();
    std::io::stdin().read_line(&mut s)?;
    Ok(s.trim().to_string())
}

/// Read a secret without echoing it when attached to a terminal.
fn prompt_secret(label: &str) -> Result<String> {
    use crossterm::event::Event;
    use crossterm::event::KeyCode;
    use crossterm::event::KeyEventKind;
    use crossterm::event::KeyModifiers;
    if !std::io::stdin().is_terminal() {
        return prompt_line(label);
    }
    eprint!("{label}");
    std::io::stderr().flush().ok();
    crossterm::terminal::enable_raw_mode()?;
    let mut out = String::new();
    let result = loop {
        match crossterm::event::read() {
            Ok(Event::Key(k)) if k.kind != KeyEventKind::Release => match k.code {
                KeyCode::Enter => break Ok(()),
                KeyCode::Char('c') if k.modifiers.contains(KeyModifiers::CONTROL) => {
                    break Err(anyhow!("已取消"));
                }
                KeyCode::Backspace => {
                    out.pop();
                }
                KeyCode::Char(c) => out.push(c),
                _ => {}
            },
            Ok(_) => {}
            Err(e) => break Err(e.into()),
        }
    };
    crossterm::terminal::disable_raw_mode().ok();
    eprintln!();
    result.map(|()| out)
}

fn read_stdin_trimmed() -> Result<String> {
    let mut s = String::new();
    std::io::Read::read_to_string(&mut std::io::stdin(), &mut s)?;
    Ok(s.trim().to_string())
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

#[derive(Debug, Args)]
pub struct LoginArgs {
    /// 紫微星通行证邮箱。
    #[arg(long)]
    pub email: Option<String>,

    /// 用邮箱验证码登录（适合用 Google / GitHub 注册、没有设置密码的账号）。
    #[arg(long)]
    pub code: bool,

    /// 从标准输入读取密码（供造物主桌面版和脚本使用）。
    #[arg(long)]
    pub password_stdin: bool,

    /// 从标准输入读取已登录的通行证 refresh token（造物主桌面版已登录时直接复用）。
    #[arg(long)]
    pub refresh_token_stdin: bool,

    /// 直接使用在 https://www.zhenbai.one/AI/api 创建的龍華樹 API Key。
    #[arg(long, value_name = "KEY")]
    pub api_key: Option<String>,
}

pub async fn run_login(args: LoginArgs) -> Result<()> {
    let home = creator_home()?;
    let client = http()?;
    if let Some(key) = args.api_key.filter(|k| !k.trim().is_empty()) {
        let mut p = read_passport(&home);
        p.api_key = Some(key.trim().to_string());
        p.key_id = None;
        p.updated_at = Some(now());
        write_passport(&home, &p)?;
        println!("已保存龍華樹 API Key。消耗从这个 Key 所属账号的 API 余额里扣。");
        sync_public(&home, &client, None).await?;
        return Ok(());
    }

    let session = if args.refresh_token_stdin {
        refresh_session(&client, &read_stdin_trimmed()?).await?
    } else {
        let email = match args.email {
            Some(e) => e,
            None => prompt_line("紫微星通行证邮箱：")?,
        };
        if email.is_empty() {
            bail!("没有输入邮箱");
        }
        if args.code {
            passport_post(
                &client,
                "auth/v1/otp",
                json!({ "email": email, "create_user": false }),
                "发送验证码",
            )
            .await?;
            let code = prompt_line("验证码已发到邮箱，请输入：")?;
            let body = passport_post(
                &client,
                "auth/v1/verify",
                json!({ "type": "email", "email": email, "token": code }),
                "验证码登录",
            )
            .await?;
            session_from(&body)?
        } else {
            let password = if args.password_stdin {
                read_stdin_trimmed()?
            } else {
                prompt_secret("密码（输入时不显示）：")?
            };
            let body = passport_post(
                &client,
                "auth/v1/token?grant_type=password",
                json!({ "email": email, "password": password }),
                "通行证登录",
            )
            .await
            .map_err(|e| anyhow!("{e:#}\n提示：用 Google/GitHub 注册的账号请改用 `creator login --code`。"))?;
            session_from(&body)?
        }
    };

    let (user_id, email) = passport_user(&client, &session).await?;
    let mut p = read_passport(&home);
    let same_user = p.user_id.is_some() && p.user_id == user_id;
    if !(same_user && p.api_key.as_deref().is_some_and(|k| !k.is_empty())) {
        let host = std::env::var("COMPUTERNAME")
            .or_else(|_| std::env::var("HOSTNAME"))
            .unwrap_or_else(|_| "电脑".to_string());
        let created = longhua(
            &client,
            &session,
            "keys",
            Some(json!({ "name": format!("造物主 Creator · {host}") })),
        )
        .await
        .map_err(|e| {
            anyhow!("{e:#}\n如果提示已达 10 个 Key 上限，请到 https://www.zhenbai.one/AI/api 撤销不用的 Key 后重试。")
        })?;
        let key = created
            .get("key")
            .and_then(|k| k.as_str().map(str::to_string).or_else(|| k.get("key").and_then(Value::as_str).map(str::to_string)))
            .ok_or_else(|| anyhow!("龍華樹没有返回 API Key：{created}"))?;
        p.key_id = created
            .get("id")
            .or_else(|| created.get("key").and_then(|k| k.get("id")))
            .and_then(|v| v.as_str().map(str::to_string).or_else(|| v.as_u64().map(|n| n.to_string())));
        p.api_key = Some(key);
    }
    p.user_id = user_id;
    p.email = email.clone();
    p.refresh_token = session.refresh_token.clone();
    p.updated_at = Some(now());
    write_passport(&home, &p)?;
    println!(
        "已用紫微星通行证登录：{}",
        email.as_deref().unwrap_or("(未知邮箱)")
    );
    sync_public(&home, &client, Some(&session)).await?;
    print_balance(&client, &session).await;
    Ok(())
}

async fn session_from_file(client: &reqwest::Client, home: &Path) -> Option<Session> {
    let mut p = read_passport(home);
    let rt = p.refresh_token.clone()?;
    match refresh_session(client, &rt).await {
        Ok(s) => {
            p.refresh_token = s.refresh_token.clone().or(Some(rt));
            p.updated_at = Some(now());
            let _ = write_passport(home, &p);
            Some(s)
        }
        Err(e) => {
            eprintln!("通行证登录已过期（{e:#}），请重新运行 `creator login`。");
            None
        }
    }
}

async fn print_balance(client: &reqwest::Client, s: &Session) {
    match longhua(client, s, "keys", None).await {
        Ok(v) => {
            let balance = if v.get("owner").and_then(Value::as_bool) == Some(true) {
                "∞（站长）".to_string()
            } else {
                format!("✦{:.2}", v.get("api_balance").and_then(Value::as_f64).unwrap_or(0.0))
            };
            let keys = v.get("keys").and_then(Value::as_array).map_or(0, Vec::len);
            println!("龍華樹 API 余额：{balance} · 有效 Key：{keys} 个");
        }
        Err(e) => eprintln!("读取余额失败：{e:#}"),
    }
}

pub async fn run_logout() -> Result<()> {
    let home = creator_home()?;
    let client = http()?;
    let p = read_passport(&home);
    if let (Some(key_id), Some(s)) = (p.key_id.clone(), session_from_file(&client, &home).await) {
        match longhua(&client, &s, "keys", Some(json!({ "revoke": key_id }))).await {
            Ok(_) => println!("已撤销本机的龍華樹 API Key。"),
            Err(e) => eprintln!("撤销 Key 失败（{e:#}），可到 https://www.zhenbai.one/AI/api 手动撤销。"),
        }
    }
    let path = home.join(PASSPORT_FILE);
    if path.exists() {
        std::fs::remove_file(&path)?;
    }
    println!("已退出紫微星通行证。");
    Ok(())
}

pub async fn run_status() -> Result<()> {
    let home = creator_home()?;
    let client = http()?;
    let p = read_passport(&home);
    println!("造物主 Creator 数据文件夹：{}", home.display());
    match (&p.email, &p.api_key) {
        (_, None) => println!("未登录：请运行 `creator login`。"),
        (Some(email), Some(_)) => println!("已登录紫微星通行证：{email}"),
        (None, Some(_)) => println!("正在使用手动填写的龍華樹 API Key。"),
    }
    if let Some(s) = session_from_file(&client, &home).await {
        print_balance(&client, &s).await;
    }
    print_plan(&read_plan(&home).unwrap_or_else(default_plan));
    Ok(())
}

fn print_plan(plan: &B31Plan) {
    let rank = |r: Option<u64>, b: &str| r.map(|r| format!("（{b}榜 #{r}）")).unwrap_or_default();
    println!("B31.Si-1.3 分工（来源：{}）", plan.source);
    println!(
        "  指挥官：{}{}",
        plan.commander,
        rank(plan.commander_rank, &plan.commander_board)
    );
    for spec in ROLES {
        if let Some(pick) = plan.roles.get(spec.id) {
            println!("  {}：{}{}", spec.title, pick.model, rank(pick.rank, &pick.board));
        }
    }
}

pub async fn run_sync() -> Result<()> {
    let home = creator_home()?;
    let client = http()?;
    let session = session_from_file(&client, &home).await;
    sync_public(&home, &client, session.as_ref()).await
}

/// Models the Responses endpoint actually serves. `/lh/v1/models` lists every model on
/// 龍華樹, but Codex speaks only the Responses API, which may serve a subset. An unknown
/// model name makes the endpoint answer "Unknown model X. Available: a, b, c" (no charge).
async fn served_models(client: &reqwest::Client, api_key: &str) -> Option<std::collections::HashSet<String>> {
    let resp = client
        .post(format!("{LONGHUA_V1}/responses"))
        .bearer_auth(api_key)
        .json(&json!({ "model": "creator-probe-unknown-model", "input": "x" }))
        .send()
        .await
        .ok()?;
    let v: Value = resp.json().await.ok()?;
    let msg = v.pointer("/error/message")?.as_str()?;
    let list = msg.split("Available:").nth(1)?;
    let set: std::collections::HashSet<String> = list
        .split(',')
        .map(|s| s.trim().trim_end_matches('.').to_string())
        .filter(|s| !s.is_empty() && !s.contains(' '))
        .collect();
    (!set.is_empty()).then_some(set)
}

/// Refresh the model catalog from `/lh/v1/models` and, when signed in, the B31.Si
/// expert table from the leaderboard ranks in `/lh/api/models`.
async fn sync_public(home: &Path, client: &reqwest::Client, session: Option<&Session>) -> Result<()> {
    let models: Vec<GatewayModel> = match client.get(format!("{LONGHUA_V1}/models")).send().await {
        Ok(resp) => match json_or_error(resp, "读取模型目录").await {
            Ok(v) => v
                .get("data")
                .cloned()
                .and_then(|d| serde_json::from_value(d).ok())
                .unwrap_or_default(),
            Err(e) => {
                eprintln!("{e:#}");
                Vec::new()
            }
        },
        Err(e) => {
            eprintln!("读取模型目录：网络错误：{e}");
            Vec::new()
        }
    };
    let mut models = if models.is_empty() {
        eprintln!("暂时用内置模型目录。");
        builtin_gateway_models()
    } else {
        models
    };
    let api_key = std::env::var(API_KEY_ENV)
        .ok()
        .filter(|k| !k.is_empty())
        .or_else(|| read_passport(home).api_key.filter(|k| !k.is_empty()));
    if let Some(key) = api_key
        && let Some(served) = served_models(client, &key).await
    {
        models.retain(|m| served.contains(&m.id));
        for id in &served {
            if !models.iter().any(|m| &m.id == id) {
                models.push(GatewayModel {
                    id: id.clone(),
                    name: None,
                    owned_by: None,
                    vision: None,
                    reasoning_efforts: Vec::new(),
                });
            }
        }
    }
    let ids: std::collections::HashSet<String> = models.iter().map(|m| m.id.clone()).collect();
    let usable = |id: &str| ids.contains(id);
    let mut plan = match session {
        Some(s) => match longhua(client, s, "boot", None).await {
            Ok(v) => build_plan(&parse_ranked_models(&v), &usable),
            Err(e) => {
                eprintln!("读取排行榜失败（{e:#}），B31.Si 暂用默认分工。");
                read_plan(home).unwrap_or_else(default_plan)
            }
        },
        None => read_plan(home).unwrap_or_else(default_plan),
    };
    sanitize_plan(&mut plan, &usable, models.first().map(|m| m.id.as_str()));
    plan.synced_at = now();
    write_plan_files(home, &plan, &models)?;
    println!("模型目录已更新：{} 个模型 + B31.Si-1.3。", models.len());
    print_plan(&plan);
    println!("新的分工在下一次启动 creator 时生效。");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_uses_leaderboard_and_independent_reviewer() {
        let ranked = parse_ranked_models(&json!({"models": [
            {"id": "a", "kind": "chat", "available": true, "ranks": {"code": 1, "text": 2, "agent": 2}},
            {"id": "b", "kind": "chat", "available": true, "ranks": {"code": 2, "text": 1, "vision": 1, "agent": 1}},
            {"id": "free", "kind": "chat", "available": true, "p_in": 0.0, "p_out": 0.0, "ranks": {"code": 1, "agent": 1}},
            {"id": "c", "kind": "image", "available": true, "ranks": {"code": 0}},
            {"id": "d", "kind": "chat", "available": false, "ranks": {"text": 1}}
        ]}));
        assert_eq!(ranked.len(), 3);
        let plan = build_plan(&ranked, &|_| true);
        assert_eq!(plan.commander, "b");
        assert_eq!(plan.roles["b31_coder"].model, "a");
        assert_eq!(plan.roles["b31_vision"].model, "b");
        assert_ne!(plan.roles["b31_reviewer"].model, plan.roles["b31_coder"].model);
    }

    #[test]
    fn catalog_round_trips_and_has_b31_first() {
        let plan = default_plan();
        let catalog = build_catalog(&builtin_gateway_models(), &plan);
        let text = serde_json::to_string(&catalog).expect("serialize");
        let back: ModelsResponse = serde_json::from_str(&text).expect("parse");
        assert_eq!(back.models[0].slug, B31_SLUG);
        let template = back.models[0]
            .model_messages
            .as_ref()
            .and_then(|m| m.instructions_template.clone())
            .unwrap_or_default();
        assert!(template.contains("B31.Si-1.3"));
        assert!(template.contains("造物主 Creator"));
    }

    #[test]
    fn default_config_parses() {
        let home = PathBuf::from(if cfg!(windows) { r"C:\Users\x\.creator" } else { "/home/x/.creator" });
        let text = default_config(&home);
        let v: toml::Value = toml::from_str(&text).expect("valid toml");
        assert_eq!(v["model_provider"].as_str(), Some("longhua"));
        assert!(v["agents"]["b31_coder"]["config_file"].as_str().is_some());
    }
}
