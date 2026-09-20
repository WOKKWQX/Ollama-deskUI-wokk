use super::{extract_stats, ChatMessage, GenParams, GenStats, OllamaClient, OllamaError};
use futures_util::StreamExt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// POST /api/chat — 流式对话。on_token 收到 content 片段，on_think 收到思考过程片段。
///
/// `cancel` 置为 true 时中断生成：此处跳出读取循环并释放响应对象，
/// 底层连接随之关闭，Ollama 便停止推理（等价于 `ollama run` 里按 Ctrl+C）。
///
/// 返回本次生成的统计信息（token 数、速度、耗时），供界面如实展示。
pub async fn stream_chat<F, G>(
    c: &OllamaClient,
    model: &str,
    messages: &[ChatMessage],
    params: &GenParams,
    images: &[String],
    cancel: Arc<AtomicBool>,
    mut on_token: F,
    mut on_think: G,
) -> Result<GenStats, OllamaError>
where
    F: FnMut(&str) + Send,
    G: FnMut(&str) + Send,
{
    let mut msgs = Vec::new();
    for (i, m) in messages.iter().enumerate() {
        let mut obj = serde_json::json!({ "role": m.role, "content": m.content });
        // 多模态：图片附在最后一条用户消息上（Ollama 约定 images 放在 message 内）
        if i + 1 == messages.len() && m.role == "user" && !images.is_empty() {
            obj["images"] =
                serde_json::Value::Array(images.iter().map(|s| s.clone().into()).collect());
        }
        msgs.push(obj);
    }

    let mut body = serde_json::Map::new();
    body.insert("model".into(), model.into());
    body.insert("messages".into(), serde_json::Value::Array(msgs));
    body.insert("stream".into(), true.into());
    if let Some(s) = &params.system {
        body.insert("system".into(), s.clone().into());
    }
    for (k, v) in params.to_option().as_object().unwrap_or(&serde_json::Map::new()) {
        body.insert(k.clone(), v.clone());
    }
    if let Some(opts) = params.options_value() {
        body.insert("options".into(), opts);
    }

    let resp = c
        .http
        .post(c.url("/api/chat"))
        .json(&serde_json::Value::Object(body))
        .send()
        .await
        .map_err(|e| OllamaError::connect(format!("无法连接 Ollama（{}）", e)))?;
    if !resp.status().is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(OllamaError::api(format!("聊天请求失败: {}", text)));
    }

    let mut stream = resp.bytes_stream();
    let mut buf: Vec<u8> = Vec::new();
    while let Some(chunk) = stream.next().await {
        // 用户点了「停止」：丢弃响应、断开连接，让 Ollama 停止推理
        if cancel.load(Ordering::SeqCst) {
            break;
        }
        let chunk = chunk.map_err(|e| OllamaError::parse(e.to_string()))?;
        buf.extend_from_slice(&chunk);
        while let Some(pos) = buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = buf.drain(..=pos).collect();
            let line = String::from_utf8_lossy(&line);
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let v: serde_json::Value = serde_json::from_str(line)
                .map_err(|e| OllamaError::parse(format!("解析聊天响应失败: {}", e)))?;
            if let Some(err) = v["error"].as_str() {
                return Err(OllamaError::api(err.to_string()));
            }
            // 思考过程（推理模型）单独回调，界面可折叠展示
            if let Some(th) = v["message"]["thinking"].as_str() {
                if !th.is_empty() {
                    on_think(th);
                }
            }
            if let Some(content) = v["message"]["content"].as_str() {
                on_token(content);
            }
            if v["done"].as_bool() == Some(true) {
                // 最后一帧携带真实统计（token 数 / 耗时），如实呈现给界面
                return Ok(extract_stats(&v));
            }
        }
    }
    Ok(GenStats::default())
}

/// POST /api/generate — 单次补全（非对话）。on_token 收到 content 片段，on_think 收到思考过程。
/// `cancel` 置为 true 时中断生成；返回本次生成的统计信息。
pub async fn stream_generate<F, G>(
    c: &OllamaClient,
    model: &str,
    prompt: &str,
    params: &GenParams,
    images: &[String],
    cancel: Arc<AtomicBool>,
    mut on_token: F,
    mut on_think: G,
) -> Result<GenStats, OllamaError>
where
    F: FnMut(&str) + Send,
    G: FnMut(&str) + Send,
{
    let mut body = serde_json::Map::new();
    body.insert("model".into(), model.into());
    body.insert("prompt".into(), prompt.into());
    body.insert("stream".into(), true.into());
    if !images.is_empty() {
        body.insert(
            "images".into(),
            serde_json::Value::Array(images.iter().map(|s| s.clone().into()).collect()),
        );
    }
    for (k, v) in params.to_option().as_object().unwrap_or(&serde_json::Map::new()) {
        body.insert(k.clone(), v.clone());
    }
    if let Some(opts) = params.options_value() {
        body.insert("options".into(), opts);
    }

    let resp = c
        .http
        .post(c.url("/api/generate"))
        .json(&serde_json::Value::Object(body))
        .send()
        .await
        .map_err(|e| OllamaError::connect(format!("无法连接 Ollama（{}）", e)))?;
    if !resp.status().is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(OllamaError::api(format!("生成请求失败: {}", text)));
    }

    let mut stream = resp.bytes_stream();
    let mut buf: Vec<u8> = Vec::new();
    while let Some(chunk) = stream.next().await {
        if cancel.load(Ordering::SeqCst) {
            break;
        }
        let chunk = chunk.map_err(|e| OllamaError::parse(e.to_string()))?;
        buf.extend_from_slice(&chunk);
        while let Some(pos) = buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = buf.drain(..=pos).collect();
            let line = String::from_utf8_lossy(&line);
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let v: serde_json::Value = serde_json::from_str(line)
                .map_err(|e| OllamaError::parse(format!("解析生成响应失败: {}", e)))?;
            if let Some(err) = v["error"].as_str() {
                return Err(OllamaError::api(err.to_string()));
            }
            if let Some(th) = v["thinking"].as_str() {
                if !th.is_empty() {
                    on_think(th);
                }
            }
            if let Some(content) = v["response"].as_str() {
                on_token(content);
            }
            if v["done"].as_bool() == Some(true) {
                return Ok(extract_stats(&v));
            }
        }
    }
    Ok(GenStats::default())
}

/// POST /api/embed — 文本嵌入（支持一次多条文本）。
/// `inputs` 为文本数组，对应 Ollama `/api/embed` 的 `input` 数组字段；
/// 返回与输入一一对应的向量列表（每条文本一个向量）。
pub async fn embed(c: &OllamaClient, model: &str, inputs: &[String]) -> Result<Vec<Vec<f64>>, OllamaError> {
    let resp = c
        .http
        .post(c.url("/api/embed"))
        .json(&serde_json::json!({ "model": model, "input": inputs }))
        .send()
        .await
        .map_err(|e| OllamaError::connect(format!("无法连接 Ollama（{}）", e)))?;
    if !resp.status().is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(OllamaError::api(format!("嵌入请求失败: {}", text)));
    }
    let body: serde_json::Value = resp.json().await.map_err(|e| OllamaError::parse(e.to_string()))?;
    Ok(parse_embeddings(&body))
}

/// 从 /api/embed 响应解析嵌入向量（与输入顺序一致）。解析与网络解耦，便于单测。
pub(crate) fn parse_embeddings(body: &serde_json::Value) -> Vec<Vec<f64>> {
    let emb = body["embeddings"].as_array().cloned().unwrap_or_default();
    emb.iter()
        .map(|arr| {
            arr.as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(|x| x.as_f64().unwrap_or(0.0))
                .collect::<Vec<f64>>()
        })
        .collect()
}

/// GET /api/version — Ollama 版本
pub async fn get_version(c: &OllamaClient) -> Result<String, OllamaError> {
    let resp = c
        .http
        .get(c.url("/api/version"))
        .send()
        .await
        .map_err(|e| OllamaError::connect(format!("无法连接 Ollama（{}）", e)))?;
    if !resp.status().is_success() {
        return Err(OllamaError::api(format!("Ollama 返回错误: {}", resp.status())));
    }
    let body: serde_json::Value = resp.json().await.map_err(|e| OllamaError::parse(e.to_string()))?;
    Ok(body["version"].as_str().unwrap_or("unknown").to_string())
}

/// 检测 Ollama 是否在线（轻量，仅 GET /api/version）
pub async fn is_online(c: &OllamaClient) -> bool {
    c.http
        .get(c.url("/api/version"))
        .timeout(std::time::Duration::from_secs(2))
        .send()
        .await
        .map(|r| r.status().is_success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_embeddings_handles_multiple_inputs() {
        // /api/embed 返回 embeddings 为「数组的数组」，与输入顺序一致
        let body = serde_json::json!({
            "model": "nomic-embed-text",
            "embeddings": [
                [0.1, 0.2, 0.3],
                [0.4, 0.5, 0.6],
                [0.7, 0.8, 0.9]
            ]
        });
        let v = parse_embeddings(&body);
        assert_eq!(v.len(), 3, "应返回与输入数量一致的向量");
        assert_eq!(v[0], vec![0.1, 0.2, 0.3]);
        assert_eq!(v[2].len(), 3);
    }

    #[test]
    fn parse_embeddings_tolerates_missing_or_malformed() {
        // 缺字段回落为空向量列表；内部非数字回落 0.0
        let empty = serde_json::json!({});
        assert!(parse_embeddings(&empty).is_empty());

        let bad = serde_json::json!({ "embeddings": [ [1.0, "x", 3.0], "not_array" ] });
        let v = parse_embeddings(&bad);
        assert_eq!(v.len(), 2);
        assert_eq!(v[0], vec![1.0, 0.0, 3.0], "非数字元素应回落 0.0");
        assert_eq!(v[1], Vec::<f64>::new(), "非数组元素应回落空向量");
    }
}
