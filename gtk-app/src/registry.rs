//! 注册中心鉴权：读写 ~/.ollama/auths.json（与 Ollama CLI 同格式），
//! 使本程序经本地 ollama serve 代理 push / pull 私有模型时携带鉴权。
//!
//! 说明：ollama.com / ollama.ai 使用 `identitytoken`；其它私有 registry 通常使用
//! `auth`（base64("user:password")）。这里两种都写，最大化兼容。

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Default, Clone)]
struct AuthFile {
    auths: HashMap<String, AuthEntry>,
}

#[derive(Serialize, Deserialize, Clone)]
struct AuthEntry {
    #[serde(skip_serializing_if = "Option::is_none")]
    identitytoken: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    auth: Option<String>,
}

/// auths.json 路径（~/.ollama/auths.json）
pub fn auth_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home)
        .join(".ollama")
        .join("auths.json")
}

fn load() -> AuthFile {
    if let Ok(s) = std::fs::read_to_string(auth_path()) {
        if let Ok(f) = serde_json::from_str::<AuthFile>(&s) {
            return f;
        }
    }
    AuthFile::default()
}

/// 当前已登录的 registry 地址列表
pub fn registries() -> Vec<String> {
    load().auths.keys().cloned().collect()
}

/// 写入某 registry 的鉴权：同时写 identitytoken 与 auth（base64 "token:<tok>"）
pub fn save_auth(registry: &str, token: &str) {
    let mut f = load();
    let auth_b64 = base64_encode(&format!("token:{token}"));
    f.auths.insert(
        registry.to_string(),
        AuthEntry {
            identitytoken: Some(token.to_string()),
            auth: Some(auth_b64),
        },
    );
    write(&f);
}

/// 删除某 registry 的鉴权
pub fn remove_auth(registry: &str) {
    let mut f = load();
    f.auths.remove(registry);
    write(&f);
}

fn write(f: &AuthFile) {
    let path = auth_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(s) = serde_json::to_string_pretty(f) {
        let _ = std::fs::write(&path, s);
    }
}

/// 不引外部 crate，手写标准 base64（用于 `auth` 字段）
fn base64_encode(s: &str) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let b = s.as_bytes();
    let mut out = String::new();
    for chunk in b.chunks(3) {
        let n = (chunk[0] as u32) << 16
            | (chunk.get(1).copied().unwrap_or(0) as u32) << 8
            | chunk.get(2).copied().unwrap_or(0) as u32;
        out.push(TABLE[((n >> 18) & 63) as usize] as char);
        out.push(TABLE[((n >> 12) & 63) as usize] as char);
        if chunk.len() > 1 {
            out.push(TABLE[((n >> 6) & 63) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(TABLE[(n & 63) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}
