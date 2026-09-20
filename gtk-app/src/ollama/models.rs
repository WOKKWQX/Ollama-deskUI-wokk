use super::{human_size, LoadedModel, ModelDetail, ModelInfo, OllamaClient, OllamaError};

/// GET /api/tags — 列出本地模型
pub async fn list_models(c: &OllamaClient) -> Result<Vec<ModelInfo>, OllamaError> {
    let resp = c.http.get(c.url("/api/tags")).send().await.map_err(|e| {
        OllamaError::connect(format!("无法连接 Ollama（{}），请确认服务已启动", e))
    })?;
    if !resp.status().is_success() {
        return Err(OllamaError::api(format!("Ollama 返回错误: {}", resp.status())));
    }
    let body: serde_json::Value = resp.json().await.map_err(|e| OllamaError::parse(e.to_string()))?;
    let models = body["models"].as_array().cloned().unwrap_or_default();
    let mut out = Vec::new();
    for m in models {
        let size = m["size"].as_u64().unwrap_or(0);
        let details = &m["details"];
        out.push(ModelInfo {
            name: m["name"].as_str().unwrap_or("").to_string(),
            model: m["model"].as_str().unwrap_or("").to_string(),
            size,
            size_human: human_size(size),
            parameter_size: details["parameter_size"].as_str().unwrap_or("").to_string(),
            modified_at: m["modified_at"].as_str().unwrap_or("").to_string(),
            family: details["family"].as_str().unwrap_or("").to_string(),
            format: details["format"].as_str().unwrap_or("").to_string(),
            quant: details["quantization_level"].as_str().unwrap_or("").to_string(),
        });
    }
    // 按名称排序，稳定展示
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// GET /api/ps — 列出已加载到内存的模型
pub async fn loaded_models(c: &OllamaClient) -> Result<Vec<LoadedModel>, OllamaError> {
    let resp = c.http.get(c.url("/api/ps")).send().await.map_err(|e| {
        OllamaError::connect(format!("无法连接 Ollama（{}），请确认服务已启动", e))
    })?;
    if !resp.status().is_success() {
        return Err(OllamaError::api(format!("Ollama 返回错误: {}", resp.status())));
    }
    let body: serde_json::Value = resp.json().await.map_err(|e| OllamaError::parse(e.to_string()))?;
    let models = body["models"].as_array().cloned().unwrap_or_default();
    let mut out = Vec::new();
    for m in models {
        let vram = m["size_vram"].as_u64().unwrap_or(0);
        let total = m["size"].as_u64().unwrap_or(0);
        out.push(LoadedModel {
            name: m["name"].as_str().unwrap_or("").to_string(),
            model: m["model"].as_str().unwrap_or("").to_string(),
            size_vram: vram,
            size_vram_human: human_size(vram),
            total,
            total_human: human_size(total),
            expires_at: m["expires_at"].as_str().unwrap_or("").to_string(),
        });
    }
    Ok(out)
}

/// GET /api/show — 模型详情
pub async fn show_model(c: &OllamaClient, name: &str) -> Result<ModelDetail, OllamaError> {
    let resp = c
        .http
        .post(c.url("/api/show"))
        .json(&serde_json::json!({ "name": name }))
        .send()
        .await
        .map_err(|e| OllamaError::connect(format!("无法连接 Ollama（{}）", e)))?;
    if !resp.status().is_success() {
        return Err(OllamaError::api(format!("Ollama 返回错误: {}", resp.status())));
    }
    let body: serde_json::Value = resp.json().await.map_err(|e| OllamaError::parse(e.to_string()))?;
    let details = &body["details"];
    Ok(ModelDetail {
        license: body["license"].as_str().unwrap_or("").to_string(),
        modelfile: body["modelfile"].as_str().unwrap_or("").to_string(),
        parameters: body["parameters"].as_str().unwrap_or("").to_string(),
        template: body["template"].as_str().unwrap_or("").to_string(),
        system: body["system"].as_str().unwrap_or("").to_string(),
        family: details["family"].as_str().unwrap_or("").to_string(),
        parameter_size: details["parameter_size"].as_str().unwrap_or("").to_string(),
        quantization: details["quantization_level"].as_str().unwrap_or("").to_string(),
    })
}

// ---------- 能力探测（v1.0.10：防嵌入模型混入对话/生成） ----------

use std::collections::HashMap;

fn caps_cache() -> &'static std::sync::Mutex<HashMap<String, Vec<String>>> {
    static CACHE: std::sync::OnceLock<std::sync::Mutex<HashMap<String, Vec<String>>>> =
        std::sync::OnceLock::new();
    CACHE.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
}

/// 能力纯判定：含 embedding 且不含 completion 即视为仅嵌入模型。
/// Ollama capabilities 取值：completion / tools / vision / embedding / thinking。
pub fn caps_embed_only(caps: &[String]) -> bool {
    caps.iter().any(|c| c == "embedding") && !caps.iter().any(|c| c == "completion")
}

/// 探测模型能力（/api/show 的 capabilities 字段）。
/// 失败或旧版 Ollama 无此字段时返回 None——调用方不得据此过滤（诚实边界）。
pub async fn model_capabilities(c: &OllamaClient, name: &str) -> Option<Vec<String>> {
    let resp = c
        .http
        .post(c.url("/api/show"))
        .json(&serde_json::json!({ "name": name }))
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let body: serde_json::Value = resp.json().await.ok()?;
    let caps = body["capabilities"].as_array()?;
    Some(
        caps.iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect(),
    )
}

/// 缓存能力（侧栏刷新时探测写入，整个应用生命周期复用）
pub fn cache_capabilities(name: &str, caps: &[String]) {
    caps_cache()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(name.to_string(), caps.to_vec());
}

/// 查缓存的「是否仅嵌入模型」。None = 未探测过（不得据此拦截）。
pub fn cached_is_embed_only(name: &str) -> Option<bool> {
    caps_cache()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(name)
        .map(|c| caps_embed_only(c))
}

/// 把「模型不支持某能力」的原始报错翻译成可行动提示；其余错误返回 None 原样呈现。
///
/// 覆盖：
/// - `does not support chat` / `does not support generate`：嵌入模型误入对话/生成；
/// - `does not support thinking`：非推理模型误开「思考」开关。
pub fn friendly_capability_error(e: &str) -> Option<String> {
    let e_l = e.to_lowercase();
    // 模型名提取：兼容转义形式（`\"bge-m3:latest\" does not support chat`）
    // 与裸形式（`"bge-m3:latest" does not support chat`），且外层可能带 [API] 前缀与 JSON 包装
    let before = e.split("does not support").next().unwrap_or("");
    let trimmed = before.trim_end_matches(['"', '\\', ' ']);
    let model = trimmed
        .rsplit(|c| c == '"' || c == '\\')
        .next()
        .unwrap_or("该模型");
    let model = if model.is_empty() { "该模型" } else { model };

    if e_l.contains("does not support thinking") {
        return Some(format!(
            "{model} 不是推理模型，不支持思考模式。\n请关闭输入区上方的「思考」开关后再试。"
        ));
    }
    let kind = if e_l.contains("does not support chat") {
        "对话"
    } else if e_l.contains("does not support generate") {
        "文本生成"
    } else {
        return None;
    };
    Some(format!(
        "{model} 是嵌入（embedding）模型，不支持{kind}。\n请在左侧选择对话模型（如 qwen2.5、llama3.2）；嵌入模型请在「嵌入」页使用。"
    ))
}

/// POST /api/cp — 复制模型
pub async fn copy_model(c: &OllamaClient, source: &str, dest: &str) -> Result<(), OllamaError> {
    let resp = c
        .http
        .post(c.url("/api/copy"))
        .json(&serde_json::json!({ "source": source, "destination": dest }))
        .send()
        .await
        .map_err(|e| OllamaError::connect(format!("无法连接 Ollama（{}）", e)))?;
    if !resp.status().is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(OllamaError::api(format!("复制失败: {}", text)));
    }
    Ok(())
}

/// DELETE /api/delete — 删除模型
pub async fn delete_model(c: &OllamaClient, name: &str) -> Result<(), OllamaError> {
    let resp = c
        .http
        .delete(c.url("/api/delete"))
        .json(&serde_json::json!({ "name": name }))
        .send()
        .await
        .map_err(|e| OllamaError::connect(format!("无法连接 Ollama（{}）", e)))?;
    if !resp.status().is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(OllamaError::api(format!("删除失败: {}", text)));
    }
    Ok(())
}

/// POST /api/generate 的 stop — 停止已加载模型（释放内存）
/// 注意：Ollama 该接口要求字段名为 `model`（不是 `name`），字段错配会导致 400 静默失败。
pub async fn stop_model(c: &OllamaClient, name: &str) -> Result<(), OllamaError> {
    let resp = c
        .http
        .post(c.url("/api/generate"))
        .json(&serde_json::json!({ "model": name, "prompt": "", "keep_alive": 0 }))
        .send()
        .await
        .map_err(|e| OllamaError::connect(format!("无法连接 Ollama（{}）", e)))?;
    if !resp.status().is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(OllamaError::api(format!("停止失败: {}", text)));
    }
    Ok(())
}

/// 置顶（常驻）模型：发送一次空生成并把 keep_alive 设为 -1，让模型一直驻留显存。
pub async fn pin_model(c: &OllamaClient, name: &str) -> Result<(), OllamaError> {
    let resp = c
        .http
        .post(c.url("/api/generate"))
        .json(&serde_json::json!({ "model": name, "prompt": "", "keep_alive": -1 }))
        .send()
        .await
        .map_err(|e| OllamaError::connect(format!("无法连接 Ollama（{}）", e)))?;
    if !resp.status().is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(OllamaError::api(format!("常驻失败: {}", text)));
    }
    Ok(())
}

/// 根据 /api/ps 返回的 expires_at（RFC3339）计算模型还将驻留多久。
/// 返回形如 "5m 12s" / "常驻" / "即将卸载" / "—" 的可读字符串。
pub fn keep_alive_remaining(expires_at: &str) -> String {
    if expires_at.is_empty() {
        return "—".to_string();
    }
    let parsed = match chrono::DateTime::parse_from_rfc3339(expires_at) {
        Ok(dt) => dt.with_timezone(&chrono::Utc),
        Err(_) => return expires_at.to_string(),
    };
    let now = chrono::Utc::now();
    let secs = (parsed - now).num_seconds();
    if secs >= 315_360_000 {
        return "常驻".to_string();
    }
    if secs <= 0 {
        return "即将卸载".to_string();
    }
    let m = secs / 60;
    let s = secs % 60;
    if m > 0 {
        format!("{m}m {s}s")
    } else {
        format!("{s}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caps(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn embed_only_detection() {
        assert!(caps_embed_only(&caps(&["embedding"])));
        assert!(caps_embed_only(&caps(&["embedding", "vision"])));
        // 含 completion（普通 LLM）不得判为仅嵌入——宁可漏拦不可误拦
        assert!(!caps_embed_only(&caps(&["completion"])));
        assert!(!caps_embed_only(&caps(&["completion", "tools"])));
        assert!(!caps_embed_only(&caps(&["embedding", "completion"])));
        assert!(!caps_embed_only(&[]));
    }

    #[test]
    fn capability_error_translated() {
        let e = r#"[API] 聊天请求失败: {"error":"\"bge-m3:latest\" does not support chat"}"#;
        let msg = friendly_capability_error(e).unwrap();
        assert!(msg.contains("bge-m3:latest"), "应含模型名: {msg}");
        assert!(msg.contains("嵌入"), "应说明是嵌入模型: {msg}");
        assert!(msg.contains("嵌入」页"), "应指路嵌入页: {msg}");

        let g = friendly_capability_error(r#""x" does not support generate"#).unwrap();
        assert!(g.contains("文本生成"));

        let t = friendly_capability_error(
            r#"[API] 聊天请求失败: {"error":"\"codellama:7b\" does not support thinking"}"#,
        )
        .unwrap();
        assert!(t.contains("codellama:7b"), "应含模型名: {t}");
        assert!(t.contains("思考模式"), "应说明不支持思考模式: {t}");
        assert!(t.contains("关闭"), "应提示关闭思考开关: {t}");

        // 普通错误原样返回 None
        assert!(friendly_capability_error("connection refused").is_none());
    }

    #[test]
    fn capability_cache_roundtrip() {
        cache_capabilities("test-model-a", &caps(&["embedding"]));
        assert_eq!(cached_is_embed_only("test-model-a"), Some(true));
        cache_capabilities("test-model-b", &caps(&["completion", "tools"]));
        assert_eq!(cached_is_embed_only("test-model-b"), Some(false));
        // 未探测过的模型返回 None（不得据此拦截）
        assert_eq!(cached_is_embed_only("never-probed"), None);
    }
}
