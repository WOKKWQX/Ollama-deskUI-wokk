use super::{OllamaClient, OllamaError};
use futures_util::StreamExt;

/// POST /api/create — 从 Modelfile 文本创建模型
/// modelfile 内容由前端提供（用户在界面中编辑），on_progress 回调整体进度
pub async fn create_model<F>(
    c: &OllamaClient,
    name: &str,
    modelfile: &str,
    mut on_progress: F,
) -> Result<(), OllamaError>
where
    F: FnMut(f64, &str) + Send,
{
    let c = c.long_timeout_clone();
    let resp = c
        .http
        .post(c.url("/api/create"))
        .json(&serde_json::json!({ "name": name, "modelfile": modelfile, "stream": true }))
        .send()
        .await
        .map_err(|e| OllamaError::connect(format!("无法连接 Ollama（{}）", e)))?;
    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(OllamaError::api_from_body(status, &text));
    }

    let mut stream = resp.bytes_stream();
    let mut buf: Vec<u8> = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| OllamaError::network(e.to_string()))?;
        buf.extend_from_slice(&chunk);
        while let Some(pos) = buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = buf.drain(..=pos).collect();
            let line = String::from_utf8_lossy(&line);
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let v: serde_json::Value = serde_json::from_str(line)
                .map_err(|e| OllamaError::parse(format!("解析创建进度失败: {}", e)))?;
            if let Some(err) = v["error"].as_str() {
                return Err(OllamaError::api_from_stream_error(err));
            }
            let status = v["status"].as_str().unwrap_or("");
            if v["done"].as_bool() == Some(true) || status == "success" {
                on_progress(100.0, status);
            } else {
                on_progress(-1.0, status);
            }
        }
    }
    Ok(())
}
