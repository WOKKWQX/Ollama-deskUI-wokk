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

/// 种子条目构造：id 由 `name:ref_tag` 派生，保证唯一（见 `seed_ids_are_unique` 测试）
#[allow(clippy::too_many_arguments)]
fn s(
    name: &str,
    display: &str,
    desc: &str,
    caps: &[Capability],
    chinese: bool,
    size: f64,
    tag: &str,
) -> CatalogModel {
    CatalogModel {
        id: format!("{name}:{tag}"),
        name: name.to_string(),
        display: display.to_string(),
        desc: desc.to_string(),
        caps: caps.to_vec(),
        chinese,
        ref_size_gb: size,
        ref_tag: tag.to_string(),
    }
}

/// 内置精选种子（编译期）。用户升级应用即自动获得这里新增的条目。
pub fn default_models() -> Vec<CatalogModel> {
    use Capability::*;
    vec![
        // ---------- 中文系 ----------
        s("qwen2.5", "通义千问 2.5", "阿里通义千问，中英文均衡，覆盖 0.5B~72B。", &[Chat, Tool], true, 4.7, "7b"),
        s("qwen2.5", "通义千问 2.5（极速 0.5B）", "千问最小档，老机器也能跑，适合尝鲜。", &[Chat], true, 0.4, "0.5b"),
        s("qwen2.5-coder", "通义千问代码 2.5", "代码专用，补全与改造能力强。", &[Code], true, 4.7, "7b"),
        s("qwen2.5-coder", "通义千问代码 2.5（轻量 1.5B）", "代码专用小档，补全够用、占用低。", &[Code], true, 1.0, "1.5b"),
        s("qwen2.5vl", "通义千问视觉 2.5", "读图多模态，支持图片问答与理解。", &[Vision, Chat], true, 6.0, "7b"),
        s("qwen2.5-math", "通义千问数学 2.5", "数学推理专用，解题步骤清晰。", &[Reason], true, 4.4, "7b"),
        s("qwen3", "通义千问 3", "新一代千问，推理与对话兼顾。", &[Chat, Tool, Reason], true, 5.0, "8b"),
        s("qwen3-coder", "通义千问代码 3（30B）", "第三代代码模型，代理式编程能力强，体积偏大。", &[Code, Tool], true, 19.0, "30b"),
        s("qwen3-vl", "通义千问视觉 3", "第三代多模态，图文理解与文档识别更强。", &[Vision, Chat], true, 5.5, "8b"),
        s("qwq", "通义千问 QwQ（推理）", "千问推理专用，长链思考。", &[Reason, Tool], true, 20.0, "32b"),
        s("deepseek-r1", "深度求索 R1（推理）", "强推理模型，擅长数学与逻辑推导。", &[Reason, Tool], true, 4.7, "7b"),
        s("deepseek-llm", "深度求索 LLM", "DeepSeek 通用基座，中英文均衡。", &[Chat], true, 4.7, "7b"),
        s("deepseek-coder-v2", "深度求索代码 V2", "代码与补全专用，覆盖多种语言。", &[Code], true, 8.9, "16b"),
        s("deepseek-v3", "深度求索 V3（671B 巨量）", "旗舰级通用模型，体积巨大，普通机器跑不动。", &[Chat, Tool], true, 404.0, "671b"),
        s("glm4", "智谱 GLM-4", "智谱通用对话，中文表现优秀。", &[Chat, Tool], true, 5.5, "9b"),
        s("glm-4v", "智谱 GLM-4V（视觉）", "智谱多模态，支持图文理解。", &[Vision, Chat], true, 5.5, "9b"),
        s("yi", "零一万物 Yi", "01.AI 通用模型，中文友好。", &[Chat], true, 3.8, "6b"),
        s("yi-coder", "零一万物 Yi-Coder", "代码专用，长上下文补全。", &[Code], true, 5.2, "9b"),
        s("baichuan2", "百川 2", "百川通用对话，中文优化。", &[Chat], true, 4.2, "7b"),
        s("internlm2", "书生·浦语 2", "上海 AI Lab 通用模型，中文扎实。", &[Chat], true, 4.5, "7b"),
        s("codegeex4", "智谱 CodeGeeX4", "代码生成专用，多语言覆盖。", &[Code], true, 5.5, "9b"),
        s("minicpm-v", "面壁 MiniCPM-V（视觉）", "超轻量多模态，手机级设备可跑。", &[Vision, Chat], true, 4.8, "2.6"),
        s("bge-m3", "BGE-M3（嵌入）", "多语言向量模型，检索与知识库首选。", &[Embed], true, 1.2, "latest"),
        s("bge-large", "BGE-Large（嵌入）", "经典中文检索嵌入，体积小。", &[Embed], true, 0.67, "latest"),
        s("qwen3", "通义千问 3（4B）", "千问 3 轻量档，小显存也能吃推理与工具调用。", &[Chat, Tool, Reason], true, 2.6, "4b"),
        s("qwen3", "通义千问 3（14B）", "千问 3 中档，较 8B 档质量跃升，16GB 级设备甜点。", &[Chat, Tool, Reason], true, 9.3, "14b"),
        s("qwen3", "通义千问 3（30B MoE）", "总参数 30B 仅激活 3B，速度接近小模型、质量更高。", &[Chat, Tool, Reason], true, 19.0, "30b-a3b"),
        s("qwen2.5", "通义千问 2.5（3B）", "千问 2.5 轻量档，笔记本级设备流畅对话。", &[Chat, Tool], true, 1.9, "3b"),
        s("qwen2.5", "通义千问 2.5（14B）", "千问 2.5 中档，质量接近更大参数量模型。", &[Chat, Tool], true, 9.0, "14b"),
        s("qwen2.5", "通义千问 2.5（32B）", "千问 2.5 大档，追求质量的上限选择。", &[Chat, Tool], true, 20.0, "32b"),
        s("qwen2", "通义千问 2（7B）", "上一代千问，老设备与兼容场景备选。", &[Chat], true, 4.4, "7b"),
        s("qwen2.5-coder", "通义千问代码 2.5（3B）", "代码轻量档，日常补全顺手、占用低。", &[Code], true, 1.9, "3b"),
        s("qwen2.5-coder", "通义千问代码 2.5（14B）", "代码中档，整文件改写与仓库级理解更稳。", &[Code], true, 9.0, "14b"),
        s("codeqwen", "千问代码 CodeQwen", "千问代码初代 7B，老牌补全选择。", &[Code], true, 4.2, "7b"),
        s("deepseek-r1", "深度求索 R1（1.5B 蒸馏）", "最小推理档，老机器也能跑思维链。", &[Reason], true, 1.1, "1.5b"),
        s("deepseek-r1", "深度求索 R1（8B 蒸馏）", "Llama3.1 底蒸馏，8GB 显存推理入门首选。", &[Reason, Tool], true, 4.9, "8b"),
        s("deepseek-r1", "深度求索 R1（14B）", "Qwen 底蒸馏，中档机器的推理甜点。", &[Reason, Tool], true, 9.0, "14b"),
        s("deepseek-r1", "深度求索 R1（32B）", "Qwen 底蒸馏，大显存机器的推理主力。", &[Reason, Tool], true, 20.0, "32b"),
        s("deepseek-r1", "深度求索 R1（70B 蒸馏）", "Llama 底蒸馏，顶级推理质量，需大显存或纯 CPU 慢跑。", &[Reason, Tool], true, 40.0, "70b"),
        s("deepseek-coder", "深度求索代码 1.5", "初代代码模型，老机器补全仍可用。", &[Code], true, 3.8, "6.7b"),
        s("deepseek-v2", "深度求索 V2 Lite", "MoE 架构轻量版，激活参数少、吞吐高。", &[Chat, Code], true, 8.9, "16b"),
        s("yi", "零一万物 Yi（9B）", "Yi 中档，中英双语扎实。", &[Chat], true, 5.3, "9b"),
        s("yi", "零一万物 Yi（34B）", "Yi 大档，长文本理解好，需大内存。", &[Chat], true, 20.0, "34b"),
        s("baichuan2", "百川 2（13B）", "百川中档，中文问答稳。", &[Chat], true, 7.4, "13b"),
        s("minicpm-v", "面壁 MiniCPM-V 2.6", "8B 级多模态，读图与 OCR 强、体积小。", &[Vision, Chat], true, 5.5, "8b"),
        // ---------- 国际系 ----------
        s("llama3.1", "Meta Llama 3.1", "Meta 主力通用模型，生态成熟。", &[Chat, Tool], false, 4.7, "8b"),
        s("llama3.2", "Meta Llama 3.2", "轻量通用，1B/3B 适合边缘设备。", &[Chat, Tool], false, 2.0, "3b"),
        s("llama3.2", "Meta Llama 3.2（1B 极速）", "Llama 最小档，极省资源。", &[Chat], false, 1.3, "1b"),
        s("llama3.2-vision", "Meta Llama 3.2（视觉）", "Llama 多模态，图片问答。", &[Vision, Chat], false, 7.0, "11b"),
        s("llama3.3", "Meta Llama 3.3", "Llama 新一代，质量更高（体积大）。", &[Chat, Tool], false, 43.0, "70b"),
        s("llama4", "Meta Llama 4 Scout", "Llama 4 稀疏专家，多模态但体量巨大。", &[Chat, Tool, Vision], false, 67.0, "scout"),
        s("gemma2", "谷歌 Gemma 2", "谷歌开放模型，2B/9B 均衡。", &[Chat], false, 5.4, "9b"),
        s("gemma2", "谷歌 Gemma 2（2B）", "Gemma 2 轻量档，占用低。", &[Chat], false, 1.6, "2b"),
        s("gemma3", "谷歌 Gemma 3（4B）", "Gemma 3 小档，原生多模态。", &[Chat, Vision], false, 3.3, "4b"),
        s("gemma3", "谷歌 Gemma 3（12B）", "Gemma 3 中档，质量与体积平衡。", &[Chat, Vision], false, 8.1, "12b"),
        s("gemma3", "谷歌 Gemma 3（27B）", "Gemma 3 大档，质量更高、需大显存。", &[Chat, Vision], false, 17.0, "27b"),
        s("phi4", "微软 Phi-4", "微软小钢炮，质量超规格。", &[Chat], false, 9.0, "14b"),
        s("phi4-mini", "微软 Phi-4 Mini", "Phi-4 轻量档，小体积强推理。", &[Chat], false, 2.5, "3.8b"),
        s("phi3", "微软 Phi-3", "轻量对话，mini 档极省资源。", &[Chat], false, 2.3, "mini"),
        s("mistral", "Mistral 7B", "经典开放权重通用模型。", &[Chat], false, 4.1, "7b"),
        s("ministral", "Mistral 小钢炮", "Mistral 轻量高效档。", &[Chat], false, 4.8, "8b"),
        s("mistral-nemo", "Mistral Nemo 12B", "长上下文通用模型，多语言好。", &[Chat, Tool], false, 7.1, "12b"),
        s("mistral-small3.1", "Mistral Small 3.1", "中小体积强通用，工具调用佳。", &[Chat, Tool], false, 15.0, "24b"),
        s("devstral", "Mistral Devstral", "面向代理式编程的代码模型。", &[Code, Tool], false, 14.0, "24b"),
        s("mixtral", "Mixtral 8x7B", "稀疏专家模型，质量高但体量大。", &[Chat, Tool], false, 26.0, "8x7b"),
        s("command-r", "Cohere Command-R", "擅长检索增强与工具调用。", &[Chat, Tool], false, 4.7, "7b"),
        s("command-r-plus", "Cohere Command-R+", "Command 大档，检索增强更强。", &[Chat, Tool], false, 59.0, "104b"),
        s("codellama", "Code Llama", "Meta 代码专用模型。", &[Code], false, 3.8, "7b"),
        s("codegemma", "谷歌 CodeGemma", "代码补全与生成专用。", &[Code], false, 5.0, "7b"),
        s("starcoder2", "StarCoder 2", "代码补全与生成专用。", &[Code], false, 4.2, "7b"),
        s("granite3", "IBM Granite 3", "IBM 企业级通用模型。", &[Chat, Tool], false, 4.7, "8b"),
        s("granite3.3", "IBM Granite 3.3", "Granite 新版，推理与工具兼顾。", &[Chat, Tool], false, 4.9, "8b"),
        s("falcon", "Falcon", "TII 开放权重通用模型。", &[Chat], false, 4.5, "7b"),
        s("openchat", "OpenChat", "微调对话模型，体验轻快。", &[Chat], false, 4.1, "7b"),
        s("zephyr", "Zephyr 7B", "DPO 微调对话模型，风格利落。", &[Chat], false, 4.1, "7b"),
        s("vicuna", "Vicuna 7B", "经典对话微调模型。", &[Chat], false, 3.8, "7b"),
        s("orca-mini", "Orca Mini", "小体积对话，适合尝鲜。", &[Chat], false, 2.0, "3b"),
        s("tinyllama", "TinyLlama", "1B 级超小模型，极速。", &[Chat], false, 1.1, "1b"),
        s("smollm2", "SmolLM2（1.7B）", "HuggingFace 小模型，小巧好用。", &[Chat], false, 1.8, "1.7b"),
        s("smollm2", "SmolLM2（360M 超轻）", "超小体积，适合极限省资源。", &[Chat], false, 0.23, "360m"),
        s("dolphin3", "Dolphin 3", "无拘束微调模型，指令跟随好。", &[Chat], false, 4.9, "8b"),
        s("nemotron-mini", "NVIDIA Nemotron Mini", "英伟达小模型，工具调用友好。", &[Chat], false, 2.7, "4b"),
        s("llava", "LLaVA（视觉）", "早期多模态代表，图文问答。", &[Vision, Chat], false, 4.5, "7b"),
        s("llava-llama3", "LLaVA-Llama3（视觉）", "基于 Llama3 的多模态，识图更准。", &[Vision, Chat], false, 5.5, "8b"),
        s("moondream", "Moondream（视觉）", "超轻量视觉模型，边缘设备可跑。", &[Vision, Chat], false, 1.7, "1.8b"),
        s("nomic-embed-text", "Nomic 嵌入", "经典文本向量模型。", &[Embed], false, 0.27, "latest"),
        s("mxbai-embed-large", "MXBAi 嵌入（大）", "高质量英文嵌入。", &[Embed], false, 0.67, "latest"),
        s("snowflake-arctic-embed", "Snowflake 嵌入", "Snowflake 检索嵌入。", &[Embed], false, 0.33, "latest"),
        s("all-minilm", "All-MiniLM（嵌入）", "极小嵌入模型，速度优先。", &[Embed], false, 0.05, "33m"),
        s("llama3.1", "Meta Llama 3.1（70B）", "Llama 3.1 大档，多语言通用质量上限，需大显存。", &[Chat, Tool], false, 40.0, "70b"),
        s("gemma2", "谷歌 Gemma 2（27B）", "Gemma 2 大档，质量较 9B 档有明显跃升。", &[Chat], false, 16.0, "27b"),
        s("gemma3", "谷歌 Gemma 3（1B）", "Gemma 3 最小档，低配设备也能跑。", &[Chat], false, 0.8, "1b"),
        s("phi4-mini-reasoning", "微软 Phi-4 Mini 推理", "小体积数学推理，解题分步清晰。", &[Reason], false, 2.5, "3.8b"),
        s("phi4-reasoning", "微软 Phi-4 推理", "Phi-4 推理版，长链思考更严谨。", &[Reason], false, 9.0, "14b"),
        s("phi3.5", "微软 Phi-3.5", "Phi-3 系小升级，多语言理解更好。", &[Chat], false, 2.2, "3.8b"),
        s("codestral", "Mistral Codestral", "代码主力档，80+ 编程语言补全。", &[Code], false, 13.0, "22b"),
        s("codellama", "Code Llama（13B）", "代码中档，补全与解释均衡。", &[Code], false, 7.4, "13b"),
        s("codellama", "Code Llama（34B）", "代码大档，质量更高、需大显存。", &[Code], false, 19.0, "34b"),
        s("starcoder2", "StarCoder 2（3B）", "小档代码补全，占用极低。", &[Code], false, 1.7, "3b"),
        s("starcoder2", "StarCoder 2（15B）", "代码大档，覆盖 600+ 语言。", &[Code], false, 8.5, "15b"),
        s("granite-code", "IBM Granite Code", "IBM 代码专用，许可宽松适合商用。", &[Code], false, 4.6, "8b"),
        s("aya-expanse", "Cohere Aya Expanse", "多语言对话强项，小语种表现突出。", &[Chat], false, 5.0, "8b"),
        s("hermes3", "Hermes 3", "Nous 微调 Llama3.1，指令跟随更听话。", &[Chat, Tool], false, 4.7, "8b"),
        s("wizardlm2", "WizardLM 2（7B）", "微软系微调，通用对话均衡。", &[Chat], false, 4.1, "7b"),
        s("solar", "Upstage Solar", "韩系 10.7B，效率与质量兼顾。", &[Chat], false, 6.1, "10.7b"),
        s("tinydolphin", "Tiny Dolphin", "1.1B 超小档，树莓派级设备可跑。", &[Chat], false, 0.8, "1.1b"),
        s("dolphin-llama3", "Dolphin Llama3", "无拘束微调 Llama3，角色扮演常用。", &[Chat], false, 4.7, "8b"),
        s("sqlcoder", "Defog SQLCoder", "自然语言转 SQL 专用模型。", &[Code], false, 4.4, "7b"),
        s("magicoder", "Magicoder", "合成数据训练的代码模型，学术出身。", &[Code], false, 3.8, "7b"),
        s("stable-code", "Stability Stable Code", "3B 代码补全，占用极低。", &[Code], false, 1.6, "3b"),
        s("falcon3", "TII Falcon 3", "Falcon 新一代 7B，效率优先。", &[Chat], false, 4.5, "7b"),
        s("olmo2", "AI2 OLMo 2", "全开放数据与权重训练，学术可信度高。", &[Chat], false, 4.5, "7b"),
        s("tulu3", "AI2 Tülu 3", "AI2 微调 Llama，指令跟随严谨。", &[Chat, Tool], false, 4.9, "8b"),
        s("exaone3.5", "LG Exaone 3.5", "LG 出品，长上下文与推理兼顾。", &[Chat, Reason], false, 4.7, "7.8b"),
        s("marco-o1", "阿里 Marco-O1", "开放推理探索版，中英思维链。", &[Reason], true, 4.7, "7b"),
        s("openthinker", "OpenThinker", "开放推理模型，长思考链输出。", &[Reason], false, 20.0, "32b"),
        s("llava-phi3", "LLaVA-Phi3（视觉）", "超轻多模态，2-3GB 即可识图。", &[Vision, Chat], false, 2.9, "latest"),
        s("llama3.2-vision", "Meta Llama 3.2 视觉（90B）", "Llama 视觉大档，识图质量最高档。", &[Vision, Chat], false, 55.0, "90b"),
        s("paraphrase-multilingual", "多语言句向量（嵌入）", "句子级多语言向量，轻量检索可用。", &[Embed], false, 0.7, "latest"),
        s("mathstral", "Mistral Mathstral", "数学与理工专用，STEM 题更强。", &[Reason], false, 4.1, "7b"),
    ]
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
}

impl Catalog {
    /// 仅内置种子（不读用户文件）
    pub fn default_only() -> Self {
        Self { models: default_models() }
    }

    /// 内置种子 + 用户叠加文件
    pub fn load() -> Self {
        let mut models = default_models();
        let ov = Self::load_overlay();
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
        Self { models }
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

    /// 搜索：匹配模型名 / 中文名 / 简介；支持中文别名替换
    pub fn search(&self, query: &str) -> Vec<&CatalogModel> {
        let q = query.trim().to_lowercase();
        if q.is_empty() {
            return self.models.iter().collect();
        }
        let expanded = apply_alias(&q);
        self.models
            .iter()
            .filter(|m| {
                let hay = format!(
                    "{} {} {} {}",
                    m.name,
                    m.display,
                    m.desc,
                    if m.chinese { "中文" } else { "" }
                )
                .to_lowercase();
                hay.contains(&expanded) || m.name.to_lowercase().contains(&expanded)
            })
            .collect()
    }

    /// 按模型名取目录项（取第一个匹配，用于详情/安装默认值）
    pub fn by_name(&self, name: &str) -> Option<&CatalogModel> {
        let base = name.split(':').next().unwrap_or(name);
        self.models.iter().find(|m| m.name == base)
    }
}

/// 在内置种子里按 id 查（叠加匹配的唯一依据）
fn seed_by_id(id: &str) -> Option<&'static CatalogModel> {
    // 用 OnceLock 缓存种子，避免每次比较都重建整份 Vec
    use std::sync::OnceLock;
    static SEED: OnceLock<Vec<CatalogModel>> = OnceLock::new();
    let seed = SEED.get_or_init(default_models);
    seed.iter().find(|m| m.id == id)
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
        assert!(cat().len() >= 60, "种子应已扩充到 60+，实际 {}", cat().len());
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
