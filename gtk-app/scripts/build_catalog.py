#!/usr/bin/env python3
"""构建内置模型目录 catalog.json（V4.0.0）。

数据来源（全部真实，禁止编造）：
  1. https://ollama.com/library            —— 官方库全部模型家族（单页全量，无分页）
  2. https://ollama.com/library/{name}     —— 详情页：总 pulls 热度（快照）
  3. https://ollama.com/library/{name}/tags —— 标签清单（按参数档聚合为「尺寸档」）
  4. https://registry.ollama.ai/v2/library/{name}/manifests/{tag} —— 实测体积（layers 求和）

条目单位：家族 × 参数尺寸档（qwen2.5 → 0.5b/1.5b/3b/7b/14b/32b/72b 各一条）。
现有 120 条人工文案（manual_seed.json）原样保留，id 一字不改；
人工条目额外用抓到的真实 pulls 补充热度字段。
撞 id 时 manual 优先、scraped 丢弃。
体积拿不到 registry 实测时按 params×2GB 估算并标 size_source="estimate"，绝不伪称实测。

用法：
  python3 build_catalog.py extract-manual   # 从 catalog.rs 提取 120 条 → manual_seed.json
  python3 build_catalog.py [--limit N] [--no-sizes]
产物：gtk-app/src/ollama/catalog.json
缓存：gtk-app/scripts/.cache/（--resume 断点续抓）
"""
import json
import re
import subprocess
import sys
import time
from datetime import date
from pathlib import Path

HERE = Path(__file__).resolve().parent
APP = HERE.parent                       # gtk-app/
SRC = APP / "src" / "ollama"
CATALOG_RS = SRC / "catalog.rs"
MANUAL_SEED = HERE / "manual_seed.json"
OUT_JSON = SRC / "catalog.json"
CACHE = HERE / ".cache"

UA = "offideskollama-catalog-builder/4.0.0 (desktop app catalog seed; contact: repo issues)"
SLEEP = 0.2                             # 请求间隔（秒），礼貌抓取
TIER_CAP = 12                           # 单家族尺寸档上限（防病态家族刷屏）

# ---------------------------------------------------------------- HTTP（curl 子进程，已验证可出网）
def http_get(url: str, accept: str | None = None, timeout: int = 30) -> str | None:
    cmd = ["curl", "-sS", "-m", str(timeout), "-A", UA]
    if accept:
        cmd += ["-H", f"Accept: {accept}"]
    cmd.append(url)
    try:
        r = subprocess.run(cmd, capture_output=True, text=True, timeout=timeout + 10)
    except subprocess.TimeoutExpired:
        return None
    if r.returncode != 0:
        return None
    return r.stdout

def fetch_json(url: str, accept: str) -> dict | None:
    txt = http_get(url, accept=accept)
    if not txt:
        return None
    try:
        return json.loads(txt)
    except json.JSONDecodeError:
        return None

# ---------------------------------------------------------------- 人工种子提取
S_CALL = re.compile(
    r's\(\s*"([^"]+)"\s*,\s*"([^"]+)"\s*,\s*"([^"]+)"\s*,'
    r'\s*&\[([^\]]*)\]\s*,\s*(true|false)\s*,\s*([0-9.]+)\s*,\s*"([^"]+)"\s*\)'
)
CAP_TOKEN = re.compile(r"[A-Za-z]\w*")   # 源码经 use Capability::* 后是裸标识符（Chat/Tool）

def extract_manual() -> list[dict]:
    src = CATALOG_RS.read_text(encoding="utf-8")
    out = []
    for m in S_CALL.finditer(src):
        name, display, desc, caps_raw, chinese, size, tag = m.groups()
        caps = CAP_TOKEN.findall(caps_raw)
        out.append({
            "id": f"{name}:{tag}",
            "name": name,
            "display": display,
            "desc": desc,
            "caps": [c.lower() for c in caps],
            "chinese": chinese == "true",
            "ref_size_gb": float(size),
            "ref_tag": tag,
            "origin": "manual",
        })
    return out

# ---------------------------------------------------------------- 缓存
def cache_get(key: str):
    p = CACHE / f"{key}.json"
    if p.exists():
        try:
            return json.loads(p.read_text(encoding="utf-8"))
        except json.JSONDecodeError:
            return None
    return None

def cache_put(key: str, val):
    CACHE.mkdir(exist_ok=True)
    (CACHE / f"{key}.json").write_text(
        json.dumps(val, ensure_ascii=False), encoding="utf-8")

# ---------------------------------------------------------------- 抓取
RE_FAMILY = re.compile(r'href="/library/([a-z0-9][a-z0-9._-]*)"')
RE_PULLS = re.compile(r'<span[^>]*>\s*(\d+(?:\.\d+)?)\s*([KMB]?)\s*</span>')
RE_TAG = re.compile(r'href="/library/{name}:([a-zA-Z0-9._-]+)"')
RE_PARAM = re.compile(r'^(\d+(?:\.\d+)?)(b|m)')

def fetch_families() -> list[str]:
    cached = cache_get("families")
    if cached:
        return cached
    html = http_get("https://ollama.com/library") or ""
    fams = sorted(set(RE_FAMILY.findall(html)))
    # 排除明显是标签链接的误捕（family 后跟 : 的不会出现在 href 里，保险起见过滤空）
    fams = [f for f in fams if f]
    cache_put("families", fams)
    return fams

def fetch_detail(fam: str) -> dict:
    """详情页 → {pulls: int|None}。首个 span 数字即总 pulls。"""
    key = f"detail-{fam}"
    cached = cache_get(key)
    if cached is not None:
        return cached
    html = http_get(f"https://ollama.com/library/{fam}")
    pulls = None
    if html:
        m = RE_PULLS.search(html)
        if m:
            num, unit = m.group(1), m.group(2)
            mult = {"": 1, "K": 1e3, "M": 1e6, "B": 1e9}[unit]
            pulls = int(float(num) * mult)
    out = {"pulls": pulls}
    cache_put(key, out)
    time.sleep(SLEEP)
    return out

def fetch_tags(fam: str) -> list[str]:
    key = f"tags-{fam}"
    cached = cache_get(key)
    if cached is not None:
        return cached
    html = http_get(f"https://ollama.com/library/{fam}/tags")
    tags: list[str] = []
    if html:
        pat = re.compile(rf'href="/library/{re.escape(fam)}:([a-zA-Z0-9._-]+)"')
        tags = sorted(set(pat.findall(html)))
    cache_put(key, tags)
    time.sleep(SLEEP)
    return tags

def fetch_size(fam: str, tag: str) -> float | None:
    """registry manifest 实测体积（GB）。拿不到返回 None。"""
    key = f"size-{fam}-{tag}"
    cached = cache_get(key)
    if cached is not None:
        return cached.get("gb")
    man = fetch_json(
        f"https://registry.ollama.ai/v2/library/{fam}/manifests/{tag}",
        accept="application/vnd.docker.distribution.manifest.v2+json")
    gb = None
    if man and isinstance(man.get("layers"), list):
        total = sum(int(l.get("size", 0)) for l in man["layers"])
        if total > 0:
            gb = round(total / 1e9, 2)
    cache_put(key, {"gb": gb})
    time.sleep(SLEEP)
    return gb

# ---------------------------------------------------------------- 尺寸档聚合
def param_of(tag: str):
    m = RE_PARAM.match(tag.lower())
    if not m:
        return None
    v = float(m.group(1))
    if m.group(2) == "m":
        v = v / 1000.0
    return v

def aggregate_tiers(fam: str, tags: list[str]) -> list[dict]:
    """把标签清单聚合为尺寸档。每档选代表 tag：
    优先与参数 token 完全一致的 tag，否则最短 tag。
    latest 仅当家族没有任何参数档时作为唯一档保留。"""
    tiers: dict[str, dict] = {}          # key -> {tags:[...], params: float|None}
    has_param = False
    for t in tags:
        p = param_of(t)
        if p is None:
            continue
        has_param = True
        key = f"{p:g}b"
        tiers.setdefault(key, {"tags": [], "params": p})["tags"].append(t)
    if not has_param:
        # 纯 latest / 命名 tag 家族（嵌入模型等）
        if "latest" in tags:
            tiers["latest"] = {"tags": ["latest"], "params": None}
        else:
            for t in tags[:1]:           # 无 latest 则取第一个命名 tag
                tiers[t] = {"tags": [t], "params": None}
    out = []
    for key, info in tiers.items():
        cands = info["tags"]
        exact = [t for t in cands if t.lower() == key or
                 (info["params"] is None and t == "latest")]
        rep = exact[0] if exact else min(cands, key=len)
        out.append({"ref_tag": rep, "params_billion": info["params"],
                    "n_variants": len(cands)})
    out.sort(key=lambda d: (d["params_billion"] is None,
                            d["params_billion"] or 0))
    return out[:TIER_CAP]

# ---------------------------------------------------------------- 中文命名与能力启发式
FAMILY_ZH = {
    "qwen": "通义千问", "qwen2": "通义千问 2", "qwen2.5": "通义千问 2.5",
    "qwen3": "通义千问 3", "qwen2.5vl": "通义千问视觉 2.5", "qwen3-vl": "通义千问视觉 3",
    "deepseek": "深度求索", "llama": "Meta Llama", "gemma": "谷歌 Gemma",
    "phi": "微软 Phi", "mistral": "Mistral", "glm": "智谱 GLM",
    "yi": "零一万物 Yi", "baichuan": "百川", "internlm": "书生·浦语",
    "minicpm": "面壁 MiniCPM", "falcon": "Falcon", "granite": "IBM Granite",
    "command-r": "Cohere Command-R", "codellama": "Code Llama",
    "codegemma": "谷歌 CodeGemma", "starcoder": "StarCoder",
    "llava": "LLaVA", "moondream": "Moondream", "tinyllama": "TinyLlama",
    "smollm": "SmolLM", "dolphin": "Dolphin", "orca": "Orca",
    "vicuna": "Vicuna", "zephyr": "Zephyr", "openchat": "OpenChat",
    "solar": "Upstage Solar", "exaone": "LG Exaone", "olmo": "AI2 OLMo",
    "tulu": "AI2 Tülu", "hermes": "Hermes", "wizardlm": "WizardLM",
    "nemotron": "NVIDIA Nemotron", "mixtral": "Mixtral", "aya": "Cohere Aya",
    "sqlcoder": "SQLCoder", "magicoder": "Magicoder", "devstral": "Devstral",
    "ministral": "Ministral", "mathstral": "Mathstral", "codestral": "Codestral",
    "bge": "BGE", "nomic": "Nomic", "mxbai": "MXBAi", "minilm": "MiniLM",
    "snowflake": "Snowflake", "paraphrase": "句向量", "all-minilm": "All-MiniLM",
    "qwq": "通义千问 QwQ", "codeqwen": "千问代码 CodeQwen",
    "codegeex": "智谱 CodeGeeX", "marco": "阿里 Marco", "openthinker": "OpenThinker",
    "stable-code": "Stable Code", "granite-code": "IBM Granite Code",
    "deepseek-r1": "深度求索 R1", "phi4": "微软 Phi-4", "phi3": "微软 Phi-3",
    "llama3.1": "Meta Llama 3.1", "llama3.2": "Meta Llama 3.2",
    "llama3.3": "Meta Llama 3.3", "llama4": "Meta Llama 4",
    "gemma2": "谷歌 Gemma 2", "gemma3": "谷歌 Gemma 3",
    "mistral-nemo": "Mistral Nemo", "mistral-small": "Mistral Small",
    "dolphin-llama3": "Dolphin Llama3", "tinydolphin": "Tiny Dolphin",
    "yi-coder": "零一万物 Yi-Coder", "deepseek-coder": "深度求索代码",
    "deepseek-llm": "深度求索 LLM", "granite3": "IBM Granite 3",
    "falcon3": "TII Falcon 3", "smollm2": "SmolLM2", "phi3.5": "微软 Phi-3.5",
}
CHINESE_FAMILIES = ("qwen", "deepseek", "glm", "yi", "baichuan", "internlm",
                    "minicpm", "codegeex", "codeqwen", "qwq", "marco", "bge")

CAP_RULES = [  # (家族名含…, 能力)——按命名特征推断，非官方标签；顺序即优先级
    ("vision", "vision"), ("-vl", "vision"), ("llava", "vision"),
    ("moondream", "vision"), ("minicpm-v", "vision"),
    ("coder", "code"), ("code", "code"), ("starcoder", "code"),
    ("sqlcoder", "code"), ("magicoder", "code"), ("devstral", "code"),
    ("reasoning", "reason"), ("-r1", "reason"), ("qwq", "reason"),
    ("math", "reason"), ("openthinker", "reason"), ("think", "reason"),
    ("embed", "embed"), ("bge", "embed"), ("nomic-embed", "embed"),
    ("arctic-embed", "embed"), ("minilm", "embed"), ("paraphrase", "embed"),
    ("e5", "embed"), ("mxbai", "embed"),
]

def zh_family(name: str) -> str | None:
    if name in FAMILY_ZH:
        return FAMILY_ZH[name]
    # 最长前缀匹配
    best = None
    for k in sorted(FAMILY_ZH, key=len, reverse=True):
        if name.startswith(k):
            best = FAMILY_ZH[k]
            break
    return best

def heur_caps(name: str) -> list[str]:
    n = name.lower()
    caps = []
    for pat, cap in CAP_RULES:
        if pat in n and cap not in caps:
            caps.append(cap)
    if not caps:
        caps = ["chat"]
    elif "vision" in caps or "embed" in caps:
        if "chat" not in caps and "embed" not in caps:
            caps.append("chat")
    return caps

def tier_label(params: float | None, tag: str) -> str:
    if params is None:
        return tag
    if params < 1:
        return f"{params * 1000:g}M"
    return f"{params:g}B"

def build_entry_scraped(fam: str, tier: dict, pulls, size_gb, size_src, gen_date) -> dict:
    tag = tier["ref_tag"]
    params = tier["params_billion"]
    zh = zh_family(fam) or fam
    suffix = tier_label(params, tag)
    display = f"{zh}（{suffix}）" if params is not None else f"{zh}（{tag}）"
    caps = heur_caps(fam)
    cap_zh = {"chat": "对话", "code": "代码", "reason": "推理",
              "vision": "视觉理解", "embed": "向量检索", "tool": "工具调用"}
    cap_txt = "、".join(cap_zh.get(c, c) for c in caps)
    if size_gb is not None:
        desc = f"{zh}，{cap_txt}，{suffix} 档，约 {size_gb:g} GB。"
    else:
        desc = f"{zh}，{cap_txt}，{suffix} 档。"
    return {
        "id": f"{fam}:{tag}",
        "name": fam,
        "display": display,
        "desc": desc,
        "caps": caps,
        "chinese": any(fam.startswith(z) for z in CHINESE_FAMILIES),
        "ref_size_gb": size_gb if size_gb is not None else
                       (round((params or 0) * 2, 2) if params else 0.0),
        "ref_tag": tag,
        "pulls": pulls,
        "params_billion": params,
        "origin": "scraped",
        "size_source": size_src,
        "generated_at": gen_date,
    }

# ---------------------------------------------------------------- 主流程
def main() -> int:
    args = sys.argv[1:]
    if args and args[0] == "extract-manual":
        entries = extract_manual()
        MANUAL_SEED.write_text(json.dumps(entries, ensure_ascii=False, indent=1),
                               encoding="utf-8")
        print(f"manual_seed.json：{len(entries)} 条")
        return 0

    limit = int(args[args.index("--limit") + 1]) if "--limit" in args else None
    no_sizes = "--no-sizes" in args

    if not MANUAL_SEED.exists():
        entries = extract_manual()
        MANUAL_SEED.write_text(json.dumps(entries, ensure_ascii=False, indent=1),
                               encoding="utf-8")
    manual = json.loads(MANUAL_SEED.read_text(encoding="utf-8"))
    print(f"人工种子：{len(manual)} 条")

    gen_date = date.today().isoformat()
    families = fetch_families()
    print(f"官方库家族总数：{len(families)}")
    if limit:
        families = families[:limit]

    # 逐家族：pulls + 标签聚合 + 代表档体积
    scraped: list[dict] = []
    skipped: list[str] = []
    for i, fam in enumerate(families, 1):
        try:
            detail = fetch_detail(fam)
            tags = fetch_tags(fam)
            if not tags:
                skipped.append(fam)
                continue
            tiers = aggregate_tiers(fam, tags)
            for t in tiers:
                if no_sizes:
                    size_gb, size_src = None, "estimate"
                else:
                    size_gb = fetch_size(fam, t["ref_tag"])
                    size_src = "registry" if size_gb is not None else "estimate"
                scraped.append(build_entry_scraped(
                    fam, t, detail.get("pulls"), size_gb, size_src, gen_date))
        except Exception as e:                       # 单家族失败不中断
            skipped.append(f"{fam} ({e})")
        if i % 25 == 0:
            print(f"  … {i}/{len(families)} 家族，已生成 {len(scraped)} 条")
    if skipped:
        (HERE / "skipped.log").write_text("\n".join(skipped), encoding="utf-8")

    # 人工条目补充真实 pulls（同家族）
    fam_pulls = {}
    for e in scraped:
        if e["pulls"] is not None:
            fam_pulls.setdefault(e["name"], e["pulls"])
    for m in manual:
        m.setdefault("pulls", fam_pulls.get(m["name"]))
        m.setdefault("params_billion", param_of(m["ref_tag"]))
        m.setdefault("size_source", "manual")
        m.setdefault("generated_at", gen_date)

    # 合并：manual 优先，scraped 撞 id 丢弃
    seen = {m["id"] for m in manual}
    merged = list(manual)
    dropped = 0
    for e in scraped:
        if e["id"] in seen:
            dropped += 1
            continue
        seen.add(e["id"])
        merged.append(e)

    # 排序：人工条目在前（保持原顺序），scraped 按 pulls 降序（缺失在后）
    def pop_key(e):
        p = e.get("pulls")
        return (0, -(p or 0), e["name"]) if p else (1, 0, e["name"])
    scraped_part = sorted(merged[len(manual):], key=pop_key)
    merged = manual + scraped_part

    OUT_JSON.write_text(json.dumps(merged, ensure_ascii=False, indent=1),
                        encoding="utf-8")
    n_registry = sum(1 for e in merged if e.get("size_source") == "registry")
    n_est = sum(1 for e in merged if e.get("size_source") == "estimate")
    n_pulls = sum(1 for e in merged if e.get("pulls"))
    print(f"完成：共 {len(merged)} 条（人工 {len(manual)} + 抓取 {len(merged)-len(manual)}，"
          f"撞 id 丢弃 {dropped}）")
    print(f"体积：registry 实测 {n_registry} 条 / 估算 {n_est} 条；含热度 {n_pulls} 条")
    print(f"产物：{OUT_JSON}")
    return 0

if __name__ == "__main__":
    sys.exit(main())
