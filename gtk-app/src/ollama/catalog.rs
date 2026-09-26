//! 模型目录：**内置精选清单（编译期种子）+ 用户叠加层（运行期文件）**。
//!
//! 架构（v3.3.0 起）：
//! - 内置清单仍是编译期种子，随应用升级自动获得新增模型（不会被用户改坏）。
//! - 用户在「设置 → 云库索引」里的改动，只落到一份**叠加文件**
//!   `~/.config/offideskollama/catalog_user.json`：
//!     · `added`     —— 用户新增的条目（可删可改）
//!     · `overrides` —— 被改过的内置条目（按稳定 id 匹配，只能改不能删，可单项恢复）
//! - 运行时把「种子 + 叠加」合并成一份完整目录供界面使用。
//!
//! 诚实边界（对齐「不造假」原则）：
//! - 不实时抓取 ollama.com 主列表（HTML 脆弱、对大陆网络不稳），改用精选清单，
//!   保证离线可用、界面稳定。
//! - 不显示任何伪造的「下载量 / 热度」——热度用「编辑推荐」呈现。
//! - `ref_size_gb` 是**参考体积**（公开已知的标准体积），仅用于适配估算与排序，
//!   一律在 UI 标注「参考」；详情页联网时再用 registry 真实体积覆盖。

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use super::hardware::{Fit, HardwareProfile, fit_for};

/// 模型能力标签
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    Chat,
    Code,
    Reason,
    Vision,
    Embed,
    Tool,
}

impl Capability {
    /// 全部能力（编辑器勾选项按此顺序）
    pub const ALL: [Capability; 6] = [
        Capability::Chat,
        Capability::Code,
        Capability::Reason,
        Capability::Vision,
        Capability::Embed,
        Capability::Tool,
    ];

    pub fn label(&self) -> &'static str {
        match self {
            Capability::Chat => "对话",
            Capability::Code => "代码",
            Capability::Reason => "推理",
            Capability::Vision => "视觉",
            Capability::Embed => "嵌入",
            Capability::Tool => "工具",
        }
    }
}

/// 左侧分类导航（纯 UI 维度，不入库）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    All,
    Chat,
    Code,
    Reason,
    Vision,
    Embed,
    Chinese,
    Small,
    Installed,
    Curated,
}

impl Category {
    pub fn label(&self) -> &'static str {
        match self {
            Category::All => "全部",
            Category::Chat => "对话",
            Category::Code => "代码",
            Category::Reason => "推理",
            Category::Vision => "视觉",
            Category::Embed => "嵌入",
            Category::Chinese => "中文优化",
            Category::Small => "小模型",
            Category::Installed => "已安装",
            Category::Curated => "编辑推荐",
        }
    }
}

/// 目录中的单个模型。
///
/// `id` 是**稳定唯一键**：内置条目由 `name:ref_tag` 派生，用户条目自动生成；
/// 叠加层按 id 匹配内置条目，因此 id 一旦生成就不再变更（编辑对话框中不可改）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CatalogModel {
    pub id: String,
    /// ollama 模型名，如 "qwen2.5"
    pub name: String,
    /// 中文展示名
    pub display: String,
    /// 一句话简介
    pub desc: String,
    /// 能力标签
    pub caps: Vec<Capability>,
    /// 是否中文优化（由模型族判断，非官方标签）
    pub chinese: bool,
    /// 参考体积（GB）：默认/推荐标签的体积，仅作适配估算，非实测
    pub ref_size_gb: f64,
    /// 默认推荐标签（用于「为你选」与 fit 估算）
    pub ref_tag: String,
    /// 官方累计拉取量快照（采集日见 `generated_at`）；无数据为 None（诚实缺省，不补造）
    #[serde(default)]
    pub pulls: Option<u64>,
    /// 参数量（十亿）。从标签名解析（如 7b→7.0），命名档（scout/mini）为 None
    #[serde(default)]
    pub params_billion: Option<f64>,
    /// 条目来源："manual"（人工文案）| "scraped"（事实模板生成）
    #[serde(default)]
    pub origin: String,
    /// 体积来源："registry"（实测）| "estimate"（估算）| "manual"（人工录入参考值）
    #[serde(default)]
    pub size_source: String,
    /// 数据采集日期（ISO 快照标记，热度与体积均以此为时效口径）
    #[serde(default)]
    pub generated_at: String,
}

impl CatalogModel {
    /// 内容是否与另一条一致（用于判断内置条目是否被用户改过）
    fn same_content(&self, o: &Self) -> bool {
        self.name == o.name
            && self.display == o.display
            && self.desc == o.desc
            && self.caps == o.caps
            && self.chinese == o.chinese
            && (self.ref_size_gb - o.ref_size_gb).abs() < 1e-9
            && self.ref_tag == o.ref_tag
    }

    /// 能力摘要（"对话 · 代码"）
    pub fn caps_summary(&self) -> String {
        self.caps.iter().map(|c| c.label()).collect::<Vec<_>>().join(" · ")
    }
}

/// 内置种子：编译期嵌入 `catalog.json`（由 `scripts/build_catalog.py` 从 Ollama
/// 官方源生成：家族×尺寸档真实条目 + 人工精修文案）。单一事实来源在数据文件。
const SEED_JSON: &str = include_str!("catalog.json");

fn seed_vec() -> &'static Vec<CatalogModel> {
    static SEED: OnceLock<Vec<CatalogModel>> = OnceLock::new();
    SEED.get_or_init(|| serde_json::from_str(SEED_JSON).expect("内置 catalog.json 解析失败"))
}

/// 内置精选种子。用户升级应用即自动获得新增条目。
pub fn default_models() -> Vec<CatalogModel> {
    seed_vec().clone()
}

/// 中文别名 → 检索关键词（帮助中文用户用中文找模型）
const ALIASES: &[(&str, &str)] = &[
    ("千问", "qwen"),
    ("通义", "qwen"),
    ("阿里", "qwen"),
    ("羊驼", "llama"),
    ("骆驼", "llama"),
    ("梅塔", "llama"),
    ("meta", "llama"),
    ("智谱", "glm"),
    ("百川", "baichuan"),
    ("深度求索", "deepseek"),
    ("零一万物", "yi"),
    ("面壁", "minicpm"),
    ("书生", "internlm"),
    ("浦语", "internlm"),
    ("微软", "phi"),
    ("谷歌", "gemma"),
    ("嵌入", "embed"),
    ("向量", "embed"),
    ("推理", "reason"),
    ("视觉", "vision"),
    ("读图", "vision"),
    ("识图", "vision"),
    ("代码", "code"),
    ("数学", "math"),
    ("猎鹰", "falcon"),
    ("ibm", "granite"),
    ("lg", "exaone"),
    ("nous", "hermes"),
];

fn apply_alias(q: &str) -> String {
    let mut s = q.to_string();
    for (from, to) in ALIASES {
        s = s.replace(from, to);
    }
    s
}

// ============================================================
// 用户叠加层持久化
// ============================================================

/// 用户叠加层：只存「用户新增」与「被改过的内置」，不复制整份目录。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UserOverlay {
    #[serde(default)]
    pub added: Vec<CatalogModel>,
    #[serde(default)]
    pub overrides: Vec<CatalogModel>,
}

/// 导入时兼容两种格式：叠加层对象，或裸模型数组（视为「新增」）。
#[derive(Deserialize)]
#[serde(untagged)]
enum Imported {
    Overlay(UserOverlay),
    List(Vec<CatalogModel>),
}

/// 运行时目录：内置种子为底 + 用户叠加。
pub struct Catalog {
    models: Vec<CatalogModel>,
    /// 与 models 同序的搜索干草堆（构建时预拼小写，千条规模搜索零临时串）
    hay: Vec<String>,
}

fn build_hay(models: &[CatalogModel]) -> Vec<String> {
    models
        .iter()
        .map(|m| {
            format!(
                "{} {} {} {}",
                m.name,
                m.display,
                m.desc,
                if m.chinese { "中文" } else { "" }
            )
            .to_lowercase()
        })
        .collect()
}

impl Catalog {
    fn from_vec(models: Vec<CatalogModel>) -> Self {
        let hay = build_hay(&models);
        Self { models, hay }
    }

    /// 仅内置种子（不读用户文件）
    pub fn default_only() -> Self {
        Self::from_vec(default_models())
    }

    /// 内置种子 + 用户叠加文件
    pub fn load() -> Self {
        Self::load_with_overlay(&Self::load_overlay())
    }

    /// 指定叠加层的加载（测试可注入，不触碰真实 HOME）
    pub fn load_with_overlay(ov: &UserOverlay) -> Self {
        let mut models = default_models();
        for o in &ov.overrides {
            if let Some(slot) = models.iter_mut().find(|m| m.id == o.id) {
                *slot = o.clone();
            }
        }
        for a in &ov.added {
            if !models.iter().any(|m| m.id == a.id) {
                models.push(a.clone());
            }
        }
        Self::from_vec(models)
    }

    fn user_path() -> std::path::PathBuf {
        let dir = std::env::var("HOME").unwrap_or_else(|_| ".".into());
        std::path::PathBuf::from(dir)
            .join(".config")
            .join("offideskollama")
            .join("catalog_user.json")
    }

    fn load_overlay() -> UserOverlay {
        std::fs::read_to_string(Self::user_path())
            .ok()
            .and_then(|s| serde_json::from_str::<UserOverlay>(&s).ok())
            .unwrap_or_default()
    }

    /// 是否已存在用户叠加文件
    pub fn has_user_file() -> bool {
        Self::user_path().exists()
    }

    /// 当前目录里用户的痕迹数量：(新增条数, 被改过的内置条数)
    pub fn user_summary(&self) -> (usize, usize) {
        let mut added = 0usize;
        let mut modified = 0usize;
        for m in &self.models {
            match seed_by_id(&m.id) {
                None => added += 1,
                Some(seed) => {
                    if !m.same_content(seed) {
                        modified += 1
                    }
                }
            }
        }
        (added, modified)
    }

    /// 把当前目录里「与内置种子不同」的部分写回叠加文件。
    /// 若已无任何自定义，则删除叠加文件，保持配置目录干净。
    pub fn save(&self) -> bool {
        let mut ov = UserOverlay::default();
        for m in &self.models {
            match seed_by_id(&m.id) {
                None => ov.added.push(m.clone()),
                Some(seed) => {
                    if !m.same_content(seed) {
                        ov.overrides.push(m.clone());
                    }
                }
            }
        }
        let path = Self::user_path();
        if ov.added.is_empty() && ov.overrides.is_empty() {
            let _ = std::fs::remove_file(&path);
            return true;
        }
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        match serde_json::to_string_pretty(&ov) {
            Ok(s) => std::fs::write(&path, s).is_ok(),
            Err(_) => false,
        }
    }

    /// 丢弃全部用户自定义（删除叠加文件）
    pub fn reset_user() -> bool {
        let path = Self::user_path();
        if !path.exists() {
            return true;
        }
        std::fs::remove_file(&path).is_ok()
    }

    // ---------- 编辑操作（编辑器使用）----------

    /// 某 id 是否来自内置种子（内置不可删除）
    pub fn is_builtin(id: &str) -> bool {
        seed_by_id(id).is_some()
    }

    /// 某内置条目是否已被用户改动（非内置恒为 false）
    pub fn is_modified(&self, id: &str) -> bool {
        match seed_by_id(id) {
            Some(seed) => self
                .models
                .iter()
                .find(|m| m.id == id)
                .map(|m| !m.same_content(seed))
                .unwrap_or(false),
            None => false,
        }
    }

    /// 一次遍历返回全部「被改过的内置条目」id 集合。
    /// 千条规模下供编辑器列表重建使用，避免逐行 O(n) 查询退化成 O(n²)。
    pub fn modified_ids(&self) -> HashSet<String> {
        self.models
            .iter()
            .filter_map(|m| match seed_by_id(&m.id) {
                Some(seed) if !m.same_content(seed) => Some(m.id.clone()),
                _ => None,
            })
            .collect()
    }

    /// 为一个新条目生成不冲突的 id
    pub fn next_user_id(&self, name: &str) -> String {
        let slug: String = name
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect();
        let slug = slug.trim_matches('-').to_lowercase();
        let slug = if slug.is_empty() { "model".to_string() } else { slug };
        let mut n = 1u32;
        loop {
            let id = if n == 1 { format!("user-{slug}") } else { format!("user-{slug}-{n}") };
            if !self.models.iter().any(|m| m.id == id) {
                return id;
            }
            n += 1;
        }
    }

    /// 新增一条（id 为空则自动生成）
    pub fn add(&mut self, mut m: CatalogModel) -> String {
        if m.id.trim().is_empty() {
            m.id = self.next_user_id(&m.name);
        }
        let id = m.id.clone();
        self.models.push(m);
        id
    }

    /// 按 id 覆盖一条（不改 id）
    pub fn update(&mut self, id: &str, mut m: CatalogModel) -> bool {
        m.id = id.to_string();
        if let Some(slot) = self.models.iter_mut().find(|x| x.id == id) {
            *slot = m;
            true
        } else {
            false
        }
    }

    /// 删除用户条目（内置条目拒绝删除）
    pub fn remove(&mut self, id: &str) -> bool {
        if Self::is_builtin(id) {
            return false;
        }
        let before = self.models.len();
        self.models.retain(|m| m.id != id);
        self.models.len() != before
    }

    /// 把内置条目恢复为种子值（用户条目则等价于删除）
    pub fn reset_entry(&mut self, id: &str) -> bool {
        match seed_by_id(id) {
            Some(seed) => {
                if let Some(slot) = self.models.iter_mut().find(|m| m.id == id) {
                    *slot = seed.clone();
                }
                true
            }
            None => self.remove(id),
        }
    }

    /// 导出叠加层 JSON（用户新增 + 改过的内置）
    pub fn to_json(&self) -> Result<String, String> {
        let mut ov = UserOverlay::default();
        for m in &self.models {
            match seed_by_id(&m.id) {
                None => ov.added.push(m.clone()),
                Some(seed) => {
                    if !m.same_content(seed) {
                        ov.overrides.push(m.clone());
                    }
                }
            }
        }
        serde_json::to_string_pretty(&ov).map_err(|e| e.to_string())
    }

    /// 合并导入一段 JSON（叠加层或裸数组）。返回生效条数。
    pub fn import_json(&mut self, json: &str) -> Result<usize, String> {
        let parsed: Imported =
            serde_json::from_str(json).map_err(|e| format!("JSON 解析失败：{e}"))?;
        let (added, overrides) = match parsed {
            Imported::Overlay(o) => (o.added, o.overrides),
            Imported::List(v) => (v, Vec::new()),
        };
        let mut n = 0usize;
        for o in overrides {
            if let Some(slot) = self.models.iter_mut().find(|m| m.id == o.id) {
                *slot = o;
                n += 1;
            }
        }
        for a in added {
            if let Some(slot) = self.models.iter_mut().find(|m| m.id == a.id) {
                *slot = a;
            } else {
                self.models.push(a);
            }
            n += 1;
        }
        Ok(n)
    }

    // ---------- 查询 ----------

    pub fn all(&self) -> &[CatalogModel] {
        &self.models
    }

    pub fn len(&self) -> usize {
        self.models.len()
    }

    pub fn is_empty(&self) -> bool {
        self.models.is_empty()
    }

    /// 按分类过滤。installed 为已装模型名（用于「已安装」分类匹配）
    pub fn by_category(&self, cat: Category, installed: &[String]) -> Vec<&CatalogModel> {
        match cat {
            Category::All => self.models.iter().collect(),
            Category::Chat => self.models.iter().filter(|m| m.caps.contains(&Capability::Chat)).collect(),
            Category::Code => self.models.iter().filter(|m| m.caps.contains(&Capability::Code)).collect(),
            Category::Reason => self.models.iter().filter(|m| m.caps.contains(&Capability::Reason)).collect(),
            Category::Vision => self.models.iter().filter(|m| m.caps.contains(&Capability::Vision)).collect(),
            Category::Embed => self.models.iter().filter(|m| m.caps.contains(&Capability::Embed)).collect(),
            Category::Chinese => self.models.iter().filter(|m| m.chinese).collect(),
            Category::Small => self.models.iter().filter(|m| m.ref_size_gb <= 3.0).collect(),
            Category::Curated => self.curated_picks(),
            // 精确到标签：某条目的**推荐标签**确实在本机才算。
            // （旧实现按基名 `starts_with("name:")` 匹配，装了 0.5b 会连 7b 一起标为已安装。
            //  注意：云库「已安装」视图已改为直接展示本机真实条目，不再走这里。）
            Category::Installed => self
                .models
                .iter()
                .filter(|m| {
                    let full = format!("{}:{}", m.name, m.ref_tag);
                    installed.iter().any(|i| i == &full || i == &m.name)
                })
                .collect(),
        }
    }

    /// 编辑推荐：覆盖主要用途的稳妥选择（按稳定 id 选取）
    pub fn curated_picks(&self) -> Vec<&CatalogModel> {
        const CURATED_IDS: &[&str] = &[
            "qwen2.5:7b",
            "deepseek-r1:7b",
            "llama3.2:3b",
            "qwen2.5-coder:7b",
            "qwen2.5-vl:7b",
            "nomic-embed-text:latest",
            "gemma2:9b",
        ];
        CURATED_IDS
            .iter()
            .filter_map(|id| self.models.iter().find(|m| &m.id == id))
            .collect()
    }

    /// 搜索：匹配模型名 / 中文名 / 简介；支持中文别名替换。
    /// 干草堆在目录构建时预拼，千条规模下每次搜索零临时串拼接。
    pub fn search(&self, query: &str) -> Vec<&CatalogModel> {
        let q = query.trim().to_lowercase();
        if q.is_empty() {
            return self.models.iter().collect();
        }
        let expanded = apply_alias(&q);
        self.models
            .iter()
            .enumerate()
            .filter(|(i, m)| {
                self.hay[*i].contains(&expanded) || m.name.to_lowercase().contains(&expanded)
            })
            .map(|(_, m)| m)
            .collect()
    }

    /// 目录中最大的官方 pulls 值（热度归一化分母；无数据为 0）
    pub fn max_pulls(&self) -> u64 {
        self.models.iter().filter_map(|m| m.pulls).max().unwrap_or(0)
    }

    /// 按模型名取目录项（取第一个匹配，用于详情/安装默认值）
    pub fn by_name(&self, name: &str) -> Option<&CatalogModel> {
        let base = name.split(':').next().unwrap_or(name);
        self.models.iter().find(|m| m.name == base)
    }
}

/// 在内置种子里按 id 查（叠加匹配的唯一依据）。
/// 种子 Vec 用 OnceLock 缓存、id→下标用 HashMap 索引，千条规模 O(1) 命中。
fn seed_by_id(id: &str) -> Option<&'static CatalogModel> {
    static INDEX: OnceLock<HashMap<String, usize>> = OnceLock::new();
    let seed = seed_vec();
    let idx = INDEX.get_or_init(|| {
        seed.iter().enumerate().map(|(i, m)| (m.id.clone(), i)).collect()
    });
    idx.get(id).map(|&i| &seed[i])
}

// ============================================================
// 白盒评分推荐（V4.0.0）
// ============================================================

/// 评分明细：每一维分数与对应的中文理由，全部可解释、可复现。
#[derive(Debug, Clone)]
pub struct ScoreBreakdown {
    pub total: f64,
    pub task: f64,
    pub fit: f64,
    pub pop: f64,
    pub reasons: Vec<String>,
}

/// 三维白盒评分：
/// ① 任务匹配（0 / 40）：能力标签包含指定任务得满分；
/// ② 硬件适配（30 / 22 / 8 / −40）：由既有 `fit_for` 判定派生；
/// ③ 官方热度（0..30）：pulls 对数归一化，快照数据、缺失按 0 分诚实呈现。
pub fn score_model(
    m: &CatalogModel,
    task: Option<Capability>,
    prof: &HardwareProfile,
    max_pulls: u64,
) -> ScoreBreakdown {
    let (task_score, mut reasons) = match task {
        Some(t) => {
            if m.caps.contains(&t) {
                (
                    40.0,
                    vec![format!("能力匹配：支持{}（{}）", t.label(), m.caps_summary())],
                )
            } else {
                (
                    0.0,
                    vec![format!("能力不含「{}」，任务匹配弱", t.label())],
                )
            }
        }
        None => (0.0, Vec::new()),
    };

    let fit = fit_for((m.ref_size_gb * 1e9) as u64, prof);
    let fit_score = match fit {
        Fit::Smooth => 30.0,
        Fit::Tight => 22.0,
        Fit::CpuOnly => 8.0,
        Fit::NoFit => -40.0,
    };
    reasons.push(fit.explain().to_string());

    let pop_score = match m.pulls {
        Some(p) if max_pulls > 0 && p > 0 => {
            let v = 30.0 * (1.0 + p as f64).log10() / (1.0 + max_pulls as f64).log10();
            reasons.push(format!(
                "官方累计拉取约 {}（热度指数 {:.0}/100，采集于 {}）",
                human_pulls(p),
                v / 30.0 * 100.0,
                if m.generated_at.is_empty() { "未知日期" } else { &m.generated_at }
            ));
            v
        }
        _ => {
            reasons.push("暂无官方热度数据".to_string());
            0.0
        }
    };

    ScoreBreakdown {
        total: task_score + fit_score + pop_score,
        task: task_score,
        fit: fit_score,
        pop: pop_score,
        reasons,
    }
}

fn human_pulls(p: u64) -> String {
    if p >= 1_000_000_000 {
        format!("{:.1}B", p as f64 / 1e9)
    } else if p >= 1_000_000 {
        format!("{:.1}M", p as f64 / 1e6)
    } else if p >= 1_000 {
        format!("{:.1}K", p as f64 / 1e3)
    } else {
        p.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cat() -> Catalog {
        Catalog::default_only()
    }

    #[test]
    fn seed_ids_are_unique() {
        let mut ids: Vec<String> = default_models().into_iter().map(|m| m.id).collect();
        let total = ids.len();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), total, "内置种子存在重复 id");
    }

    #[test]
    fn catalog_is_expanded() {
        let n = cat().len();
        // 实抓 458 条（官方库 243 家族 × 尺寸档），软下限 400 防官方库规模波动误报
        assert!(n >= 400, "V4.0.0 种子应扩到 400+，实际 {n}");
    }

    #[test]
    fn manual_entries_preserved() {
        // 人工文案一字不改：抽 3 条代表性条目核对 display 与 desc
        let c = cat();
        let get = |id: &str| c.all().iter().find(|m| m.id == id).unwrap().clone();
        assert_eq!(get("qwen2.5:7b").display, "通义千问 2.5");
        assert_eq!(get("deepseek-r1:7b").desc, "强推理模型，擅长数学与逻辑推导。");
        assert_eq!(get("bge-m3:latest").chinese, true);
        assert_eq!(get("llama3.1:8b").chinese, false);
    }

    #[test]
    fn seed_has_heat_and_origin_metadata() {
        let c = cat();
        assert!(c.max_pulls() > 0, "应有条目带官方 pulls 热度");
        assert!(c
            .all()
            .iter()
            .any(|m| m.origin == "manual" && m.size_source == "manual"));
        assert!(c.all().iter().any(|m| m.origin == "scraped"));
    }

    #[test]
    fn overlay_old_file_compat() {
        // 旧版叠加层 JSON 无 V4 新字段，serde(default) 必须兼容
        let old = r#"{"added":[{"id":"user-x","name":"x","display":"X","desc":"d","caps":["chat"],"chinese":false,"ref_size_gb":1.0,"ref_tag":"latest"}],"overrides":[]}"#;
        let ov: UserOverlay = serde_json::from_str(old).expect("旧叠加层应可解析");
        assert_eq!(ov.added.len(), 1);
        // 目录加载后仍能识别这条 added（id 不在种子里）
        let c = Catalog::load_with_overlay(&ov);
        assert_eq!(c.user_summary(), (1, 0));
    }

    #[test]
    fn score_model_ranking() {
        let prof = HardwareProfile::default();
        let c = cat();
        let with_heat = c
            .all()
            .iter()
            .find(|m| m.pulls.is_some() && m.ref_size_gb > 0.0 && m.ref_size_gb < 5.0)
            .expect("应存在带热度的小模型")
            .clone();
        let mut cold = with_heat.clone();
        cold.id = "cold-test:1b".into();
        cold.pulls = None;
        let maxp = c.max_pulls();
        let hot = score_model(&with_heat, None, &prof, maxp);
        let none = score_model(&cold, None, &prof, maxp);
        assert!(hot.pop > 0.0 && none.pop == 0.0);
        assert!(hot.total > none.total);
        assert!(!hot.reasons.is_empty() && !none.reasons.is_empty());
    }

    #[test]
    fn score_model_task_match_dominates() {
        let prof = HardwareProfile::default();
        let c = cat();
        let coder = c
            .all()
            .iter()
            .find(|m| m.caps.contains(&Capability::Code))
            .unwrap()
            .clone();
        let chat = c
            .all()
            .iter()
            .find(|m| !m.caps.contains(&Capability::Code) && m.caps.contains(&Capability::Chat))
            .unwrap()
            .clone();
        let s_code = score_model(&coder, Some(Capability::Code), &prof, c.max_pulls());
        let s_chat = score_model(&chat, Some(Capability::Code), &prof, c.max_pulls());
        assert!(s_code.task == 40.0 && s_chat.task == 0.0);
        assert!(s_code.total > s_chat.total);
    }

    #[test]
    fn score_reasons_nonempty_for_all() {
        let prof = HardwareProfile::default();
        let c = cat();
        let maxp = c.max_pulls();
        for m in c.all() {
            let s = score_model(m, None, &prof, maxp);
            assert!(!s.reasons.is_empty(), "{} 理由为空", m.id);
        }
    }

    #[test]
    fn search_uses_haystack() {
        let c = cat();
        assert!(c.search("千问").iter().any(|m| m.name.starts_with("qwen")));
        assert!(c.search("嵌入").iter().any(|m| m.caps.contains(&Capability::Embed)));
    }

    #[test]
    fn all_returns_full_catalog() {
        let c = cat();
        assert_eq!(c.all().len(), default_models().len());
    }

    #[test]
    fn by_category_filters_capabilities() {
        let c = cat();
        let code = c.by_category(Category::Code, &[]);
        assert!(!code.is_empty());
        assert!(code.iter().all(|m| m.caps.contains(&Capability::Code)));
        let vision = c.by_category(Category::Vision, &[]);
        assert!(vision.iter().all(|m| m.caps.contains(&Capability::Vision)));
    }

    #[test]
    fn small_category_uses_size_threshold() {
        let c = cat();
        let small = c.by_category(Category::Small, &[]);
        assert!(small.iter().all(|m| m.ref_size_gb <= 3.0));
        assert!(small.iter().any(|m| m.name == "tinyllama"));
    }

    #[test]
    fn chinese_category_only_chinese() {
        let c = cat();
        let zh = c.by_category(Category::Chinese, &[]);
        assert!(zh.iter().all(|m| m.chinese));
        assert!(!zh.iter().any(|m| m.name == "llama3.1"));
    }

    #[test]
    fn search_by_chinese_alias() {
        let c = cat();
        let r = c.search("千问");
        assert!(!r.is_empty());
        assert!(r.iter().any(|m| m.name.starts_with("qwen")));
        // 新增别名
        assert!(c.search("书生").iter().any(|m| m.name == "internlm2"));
    }

    #[test]
    fn search_by_english_name() {
        let c = cat();
        let r = c.search("llama");
        assert!(!r.is_empty());
        assert!(r.iter().any(|m| m.name.starts_with("llama")));
    }

    #[test]
    fn search_empty_returns_all() {
        let c = cat();
        assert_eq!(c.search("").len(), c.len());
    }

    #[test]
    fn by_name_handles_tag_suffix() {
        let c = cat();
        assert!(c.by_name("qwen2.5:7b").is_some());
        assert!(c.by_name("unknown-model").is_none());
    }

    #[test]
    fn curated_picks_are_subset() {
        let c = cat();
        let cur = c.curated_picks();
        assert!(!cur.is_empty());
        assert!(cur.len() <= c.len());
    }

    #[test]
    fn installed_category_matches_base_name() {
        let c = cat();
        let inst = c.by_category(Category::Installed, &["qwen2.5:7b".to_string()]);
        assert!(inst.iter().any(|m| m.name == "qwen2.5"));
    }

    // ---------- 叠加层语义 ----------

    fn user_model(name: &str) -> CatalogModel {
        CatalogModel {
            id: String::new(),
            name: name.to_string(),
            display: format!("{name} 自建"),
            desc: "用户自建条目".into(),
            caps: vec![Capability::Chat],
            chinese: false,
            ref_size_gb: 1.5,
            ref_tag: "latest".into(),
            pulls: None,
            params_billion: None,
            origin: String::new(),
            size_source: String::new(),
            generated_at: String::new(),
        }
    }

    #[test]
    fn builtin_is_not_removable_but_user_is() {
        let mut c = cat();
        assert!(!c.remove("qwen2.5:7b"), "内置条目不可删除");
        let id = c.add(user_model("my-model"));
        assert!(c.remove(&id), "用户条目应可删除");
    }

    #[test]
    fn next_user_id_avoids_collision() {
        let mut c = cat();
        let a = c.add(user_model("mine"));
        let b = c.add(user_model("mine"));
        assert_ne!(a, b);
    }

    #[test]
    fn reset_entry_restores_seed() {
        let mut c = cat();
        let mut edited = c.all().iter().find(|m| m.id == "qwen2.5:7b").unwrap().clone();
        edited.display = "被改过的名字".into();
        assert!(c.update("qwen2.5:7b", edited));
        assert_eq!(c.all().iter().find(|m| m.id == "qwen2.5:7b").unwrap().display, "被改过的名字");
        assert!(c.reset_entry("qwen2.5:7b"));
        assert_ne!(c.all().iter().find(|m| m.id == "qwen2.5:7b").unwrap().display, "被改过的名字");
    }

    #[test]
    fn user_summary_counts_adds_and_modifies() {
        let mut c = cat();
        assert_eq!(c.user_summary(), (0, 0));
        c.add(user_model("mine"));
        let mut edited = c.all().iter().find(|m| m.id == "llama3.1:8b").unwrap().clone();
        edited.ref_size_gb = 9.9;
        c.update("llama3.1:8b", edited);
        let (added, modified) = c.user_summary();
        assert_eq!(added, 1);
        assert_eq!(modified, 1);
    }

    #[test]
    fn import_accepts_overlay_and_bare_list() {
        let mut c = cat();
        let bare = r#"[{"id":"user-x","name":"x","display":"X","desc":"d","caps":["chat"],"chinese":false,"ref_size_gb":1.0,"ref_tag":"latest"}]"#;
        assert_eq!(c.import_json(bare).unwrap(), 1);
        assert!(c.all().iter().any(|m| m.id == "user-x"));

        let overlay = r#"{"added":[],"overrides":[{"id":"llama3.1:8b","name":"llama3.1","display":"覆盖名","desc":"d","caps":["chat"],"chinese":false,"ref_size_gb":4.7,"ref_tag":"8b"}]}"#;
        assert_eq!(c.import_json(overlay).unwrap(), 1);
        assert_eq!(c.all().iter().find(|m| m.id == "llama3.1:8b").unwrap().display, "覆盖名");
    }

    #[test]
    fn export_round_trips_added() {
        let mut c = cat();
        c.add(user_model("mine"));
        let json = c.to_json().unwrap();
        let mut c2 = cat();
        c2.import_json(&json).unwrap();
        assert!(c2.all().iter().any(|m| m.name == "mine"));
    }

    #[test]
    fn is_modified_tracks_overrides() {
        let mut c = cat();
        assert!(!c.is_modified("qwen2.5:7b"));
        let mut edited = c.all().iter().find(|m| m.id == "qwen2.5:7b").unwrap().clone();
        edited.ref_tag = "14b".into();
        c.update("qwen2.5:7b", edited);
        assert!(c.is_modified("qwen2.5:7b"));
        // 用户条目不算「被改的内置」
        let id = c.add(user_model("mine"));
        assert!(!c.is_modified(&id));
    }

    #[test]
    fn import_rejects_bad_json() {
        let mut c = cat();
        assert!(c.import_json("{ not json").is_err());
    }
}
