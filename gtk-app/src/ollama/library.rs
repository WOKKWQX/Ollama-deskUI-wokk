use super::{human_size, OllamaClient, OllamaError};
use reqwest::header;

const UA: &str = "offideskOllama/0.15 (+https://github.com/offideskollama/offideskollama)";

/// 模型库中的一个可用标签（如 llama3.2 的 latest / 1b / 3b …）
#[derive(Debug, Clone)]
pub struct LibraryTag {
    pub tag: String,
    pub size: u64,
    pub size_human: String,
}

/// 从 ollama.com/library/{model}/tags 的 HTML 中抽取标签名。
///
/// Ollama 官方未暴露 registry 的 `/v2/library/{model}/tags/list` 端点（返回 404），
/// 因此解析公开库页面的标签链接，URL 模式为 `/library/{model}:{tag}`，
/// 比 DOM 结构更稳定。
pub(crate) fn extract_tags_from_html(html: &str, model: &str) -> Vec<String> {
    let needle = format!("/library/{}:", model);
    let mut tags = Vec::new();
    for (idx, _) in html.match_indices(&needle) {
        let tail = &html[idx + needle.len()..];
        let tag: String = tail
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
            .collect();
        if !tag.is_empty() && !tags.contains(&tag) {
            tags.push(tag);
        }
    }
    tags
}

/// 尽力从 registry manifest 取某标签体积。
/// 大陆或受限网络下 registry.ollama.ai 可能不可达，此时返回 None，
/// UI 显示「未知」，不伪造大小。
pub(crate) async fn fetch_tag_size(c: &OllamaClient, model: &str, tag: &str) -> Option<u64> {
    let url = format!("https://registry.ollama.ai/v2/library/{}/manifests/{}", model, tag);
    let resp = c
        .http
        .get(&url)
        .header(header::ACCEPT, "application/vnd.docker.distribution.manifest.v2+json")
        .header(header::USER_AGENT, UA)
        .timeout(std::time::Duration::from_secs(3))
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let body: serde_json::Value = resp.json().await.ok()?;
    let total: u64 = body["layers"]
        .as_array()?
        .iter()
        .filter_map(|l| l["size"].as_u64())
        .sum();
    Some(total)
}

/// 查询某模型在 Ollama 官方库的可用标签。
///
/// 主数据源：`https://ollama.com/library/<model>/tags`（HTML 公开页面）。
/// 大小数据源：`https://registry.ollama.ai/v2/library/<model>/manifests/<tag>`
///（尽力而为；不可达时显示「未知」）。
pub async fn fetch_tags(c: &OllamaClient, model: &str) -> Result<Vec<LibraryTag>, OllamaError> {
    let model = model.trim();
    if model.is_empty() {
        return Err(OllamaError::api("模型名不能为空".to_string()));
    }
    // 用户可能手滑输入 qwen2.5:72b，这里只取基座名
    let base = model.split(':').next().unwrap_or(model);

    let url = format!("https://ollama.com/library/{}/tags", base);
    let resp = c
        .http
        .get(&url)
        .header(header::USER_AGENT, UA)
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| OllamaError::connect(format!("无法连接 Ollama 模型库（{}）", e)))?;
    if !resp.status().is_success() {
        return Err(OllamaError::api(format!(
            "模型库查询失败（HTTP {}）：请确认模型名拼写正确，如 llama3.2",
            resp.status()
        )));
    }
    let html = resp.text().await.map_err(|e| OllamaError::parse(e.to_string()))?;
    let tag_names = extract_tags_from_html(&html, base);
    if tag_names.is_empty() {
        return Err(OllamaError::api(
            "未找到该模型的标签（请确认模型名拼写正确，如 llama3.2）".to_string(),
        ));
    }

    let mut out = Vec::new();
    // v1.0.9：registry 不可达时（大陆常见）首个失败即短路，其余标签直接「未知」。
    // 此前逐标签串行请求、每个 10s 超时——30+ 标签 = 最长 5 分钟，详情窗长时间假死。
    let mut registry_ok = true;
    for tag in tag_names {
        let size = if registry_ok {
            match fetch_tag_size(c, base, &tag).await {
                Some(s) => s,
                None => {
                    registry_ok = false;
                    0
                }
            }
        } else {
            0
        };
        out.push(LibraryTag {
            tag,
            size,
            size_human: if size > 0 {
                human_size(size)
            } else {
                "未知".to_string()
            },
        });
    }
    // latest 置顶，其余字母序
    out.sort_by(|a, b| {
        if a.tag == "latest" {
            return std::cmp::Ordering::Less;
        }
        if b.tag == "latest" {
            return std::cmp::Ordering::Greater;
        }
        a.tag.cmp(&b.tag)
    });
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_tags_from_html_parses_url_pattern() {
        let html = r#"
        <a href="/library/llama3.2:latest">latest</a>
        <a href="/library/llama3.2:1b">1b</a>
        <a href="/library/llama3.2:3b-instruct-q4_K_M">3b-instruct-q4_K_M</a>
        <a href="/library/other:latest">ignore</a>
        "#;
        let tags = extract_tags_from_html(html, "llama3.2");
        assert_eq!(tags, vec!["latest", "1b", "3b-instruct-q4_K_M"]);
    }

    #[test]
    fn extract_tags_deduplicates_and_skips_empty() {
        let html = r#"
        <a href="/library/qwen2.5:latest">x</a>
        <a href="/library/qwen2.5:latest">x</a>
        <a href="/library/qwen2.5:">bad</a>
        "#;
        let tags = extract_tags_from_html(html, "qwen2.5");
        assert_eq!(tags, vec!["latest"]);
    }

    #[test]
    fn extract_tags_empty_when_no_match() {
        let tags = extract_tags_from_html("<html></html>", "unknown-model");
        assert!(tags.is_empty());
    }
}
