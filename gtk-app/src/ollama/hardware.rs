//! 硬件画像：检测 GPU / 显存 / 内存 / CPU，并评估模型能否流畅运行。
//!
//! 诚实原则（对齐产品「取证后动手、不擅自补缺」）：
//! - 检测不到显存就让用户手动填，绝不编造数字；
//! - 所有估算结果一律在 UI 标注「估算 / 参考」，不伪装成实测。

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::process::Command;

/// 模型对当前硬件的适配程度
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Fit {
    /// 显存充裕，流畅
    Smooth,
    /// 显存勉强够，可用但偏紧
    Tight,
    /// 放不下显存，但内存足够可纯 CPU 跑（慢）
    CpuOnly,
    /// 内存与显存都放不下
    NoFit,
}

impl Fit {
    pub fn label(&self) -> &'static str {
        match self {
            Fit::Smooth => "流畅",
            Fit::Tight => "勉强",
            Fit::CpuOnly => "内存可跑(慢)",
            Fit::NoFit => "放不下",
        }
    }
    /// 用于排序：Smooth 最优先
    pub fn rank(&self) -> u8 {
        match self {
            Fit::Smooth => 3,
            Fit::Tight => 2,
            Fit::CpuOnly => 1,
            Fit::NoFit => 0,
        }
    }

    /// 徽章悬浮说明：把判定依据讲清楚（用户不必猜「勉强」是什么意思）。
    ///
    /// 判定口径见 `fit_for`：显存 70% 以内为流畅、显存以内为勉强、
    /// 显存放不下但内存 60% 以内为内存可跑、其余放不下。
    pub fn explain(&self) -> &'static str {
        match self {
            Fit::Smooth => "显存充裕（占用不超过显存的约 70%），可全速运行",
            Fit::Tight => "显存偏紧（接近显存上限），可能部分层回落到 CPU，速度下降",
            Fit::CpuOnly => "显存放不下，但内存足够纯 CPU 运行（明显更慢）",
            Fit::NoFit => "显存与内存都放不下，不建议在本机运行",
        }
    }
}

/// 机器硬件画像
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HardwareProfile {
    /// GPU 名称；无独显或检测失败为 None
    pub gpu_name: Option<String>,
    /// 显存字节；未检测 / 用户未填为 None
    pub vram_bytes: Option<u64>,
    /// 系统内存字节
    pub ram_bytes: u64,
    /// CPU 逻辑核数
    pub cpu_cores: u32,
    /// 检测时间（本地可读串）
    pub detected_at: String,
    /// 显存是否用户手动填写（true 时说明工具未检测到，按内存估算）
    pub manual_vram: bool,
}

impl Default for HardwareProfile {
    fn default() -> Self {
        HardwareProfile {
            gpu_name: None,
            vram_bytes: None,
            ram_bytes: 0,
            cpu_cores: 1,
            detected_at: String::new(),
            manual_vram: false,
        }
    }
}

impl HardwareProfile {
    /// 显存（GB），用于展示；None 时返回 0 并在 UI 标注「未知」
    pub fn vram_gb(&self) -> f64 {
        self.vram_bytes.map(|b| b as f64 / 1e9).unwrap_or(0.0)
    }
    pub fn ram_gb(&self) -> f64 {
        self.ram_bytes as f64 / 1e9
    }
    /// 一行摘要，用于硬件横幅
    pub fn summary(&self) -> String {
        let gpu = self
            .gpu_name
            .clone()
            .unwrap_or_else(|| "无独立显卡".to_string());
        let vram = if let Some(v) = self.vram_bytes {
            format!("{:.1} GB 显存", v as f64 / 1e9)
        } else {
            "显存未检测".to_string()
        };
        let ram = if self.ram_bytes > 0 {
            format!("{:.1} GB 内存", self.ram_gb())
        } else {
            "内存未知".to_string()
        };
        format!("{} · {} · {} · {} 核", gpu, vram, ram, self.cpu_cores)
    }
    /// 是否拥有可用于 fit 判定的任何容量信息
    pub fn has_any_capacity(&self) -> bool {
        self.vram_bytes.is_some() || self.ram_bytes > 0
    }
}

fn detect_ram() -> u64 {
    if let Ok(s) = std::fs::read_to_string("/proc/meminfo") {
        for line in s.lines() {
            if line.starts_with("MemTotal:") {
                if let Some(kb_str) = line.split_whitespace().nth(1) {
                    if let Ok(kb) = kb_str.parse::<u64>() {
                        return kb * 1024;
                    }
                }
            }
        }
    }
    0
}

fn detect_cpu_cores() -> u32 {
    std::thread::available_parallelism()
        .map(|v| v.get() as u32)
        .unwrap_or(1)
        .max(1)
}

fn detect_gpu() -> (Option<String>, Option<u64>) {
    // NVIDIA：nvidia-smi 报 MiB
    if let Ok(out) = Command::new("nvidia-smi")
        .args(["--query-gpu=name,memory.total", "--format=csv,noheader,nounits"])
        .output()
    {
        if out.status.success() {
            let s = String::from_utf8_lossy(&out.stdout);
            let line = s.lines().next().unwrap_or("");
            let parts: Vec<&str> = line.split(',').map(|x| x.trim()).collect();
            if parts.len() >= 2 {
                let name = parts[0].to_string();
                if let Ok(mb) = parts[1].parse::<f64>() {
                    let vram = (mb * 1024.0 * 1024.0) as u64;
                    if !name.is_empty() && vram > 0 {
                        return (Some(name), Some(vram));
                    }
                }
            }
        }
    }
    // AMD ROCm：rocminfo / rocm-smi 解析较脆弱，这里仅尽力
    if let Ok(out) = Command::new("rocm-smi")
        .args(["--showmeminfo", "vram", "--csv"])
        .output()
    {
        if out.status.success() {
            let s = String::from_utf8_lossy(&out.stdout);
            if let Some(v) = parse_rocm_vram(&s) {
                return (Some("AMD GPU".to_string()), Some(v));
            }
        }
    }
    (None, None)
}

/// ROCm 显存解析：格式脆弱，这里只在能稳定取出一个数字时才返回，绝不臆造。
fn parse_rocm_vram(csv: &str) -> Option<u64> {
    for line in csv.lines() {
        // 形如：card0, ..., 1024, MB, ...
        let fields: Vec<&str> = line.split(',').map(|x| x.trim()).collect();
        for (i, f) in fields.iter().enumerate() {
            if let Ok(mb) = f.parse::<f64>() {
                if mb > 0.0 && i + 1 < fields.len() && fields[i + 1].eq_ignore_ascii_case("MB") {
                    return Some((mb * 1024.0 * 1024.0) as u64);
                }
            }
        }
    }
    None
}

/// 检测硬件画像。工具缺失时仍返回（gpu/vram 为 None），由 UI 提示手动填显存。
pub fn detect_hardware() -> HardwareProfile {
    let (gpu_name, vram) = detect_gpu();
    let ram = detect_ram();
    let cores = detect_cpu_cores();
    HardwareProfile {
        gpu_name,
        vram_bytes: vram,
        ram_bytes: ram,
        cpu_cores: cores,
        detected_at: now_string(),
        manual_vram: false,
    }
}

/// 评估某体积能否跑。size_bytes 为目标模型体积（字节）。
pub fn fit_for(size_bytes: u64, p: &HardwareProfile) -> Fit {
    let vram = p.vram_bytes.unwrap_or(0);
    if vram > 0 {
        if size_bytes <= (vram as f64 * 0.7) as u64 {
            return Fit::Smooth;
        }
        if size_bytes <= vram {
            return Fit::Tight;
        }
        // 放不下显存，看内存（模型 + 系统开销约需 1.5 倍余量）
        if p.ram_bytes > 0 && size_bytes <= (p.ram_bytes as f64 * 0.6) as u64 {
            return Fit::CpuOnly;
        }
        return Fit::NoFit;
    }
    // 无显存信息（纯 CPU 估算）
    if p.ram_bytes > 0 && size_bytes <= (p.ram_bytes as f64 * 0.5) as u64 {
        return Fit::CpuOnly;
    }
    Fit::NoFit
}

fn now_string() -> String {
    if let Ok(out) = Command::new("date").arg("+%Y-%m-%d %H:%M").output() {
        if out.status.success() {
            return String::from_utf8_lossy(&out.stdout).trim().to_string();
        }
    }
    String::new()
}

fn hardware_path() -> PathBuf {
    let dir = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(dir)
        .join(".config")
        .join("offideskollama")
        .join("hardware.json")
}

/// 读取持久化的硬件画像（首次运行或检测失败时为 None）
pub fn load_hardware() -> Option<HardwareProfile> {
    let p = hardware_path();
    let s = std::fs::read_to_string(&p).ok()?;
    serde_json::from_str(&s).ok()
}

/// 持久化硬件画像到独立文件（与设置隔离，重测/手动填后写入）
pub fn save_hardware(p: &HardwareProfile) {
    let path = hardware_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(s) = serde_json::to_string_pretty(p) {
        let _ = std::fs::write(&path, s);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn gb(n: f64) -> u64 {
        (n * 1e9) as u64
    }

    #[test]
    fn fit_smooth_when_plenty_vram() {
        let p = HardwareProfile {
            vram_bytes: Some(gb(12.0)),
            ram_bytes: gb(32.0),
            ..Default::default()
        };
        assert_eq!(fit_for(gb(4.0), &p), Fit::Smooth);
    }
    #[test]
    fn fit_tight_when_near_vram() {
        let p = HardwareProfile {
            vram_bytes: Some(gb(8.0)),
            ram_bytes: gb(16.0),
            ..Default::default()
        };
        assert_eq!(fit_for(gb(7.5), &p), Fit::Tight);
    }
    #[test]
    fn fit_cpu_only_when_over_vram_but_ram_ok() {
        let p = HardwareProfile {
            vram_bytes: Some(gb(4.0)),
            ram_bytes: gb(32.0),
            ..Default::default()
        };
        // 8GB > 4GB 显存，但 8GB <= 0.6*32≈19.2GB 内存
        assert_eq!(fit_for(gb(8.0), &p), Fit::CpuOnly);
    }
    #[test]
    fn fit_nofit_when_over_both() {
        let p = HardwareProfile {
            vram_bytes: Some(gb(4.0)),
            ram_bytes: gb(8.0),
            ..Default::default()
        };
        assert_eq!(fit_for(gb(10.0), &p), Fit::NoFit);
    }
    #[test]
    fn fit_cpu_only_without_vram_info() {
        let p = HardwareProfile {
            vram_bytes: None,
            ram_bytes: gb(16.0),
            ..Default::default()
        };
        assert_eq!(fit_for(gb(6.0), &p), Fit::CpuOnly);
    }
    #[test]
    fn fit_nofit_without_vram_and_small_ram() {
        let p = HardwareProfile {
            vram_bytes: None,
            ram_bytes: gb(8.0),
            ..Default::default()
        };
        assert_eq!(fit_for(gb(6.0), &p), Fit::NoFit);
    }
    #[test]
    fn summary_handles_missing_gpu_vram() {
        let p = HardwareProfile::default();
        let s = p.summary();
        assert!(s.contains("显存未检测"), "摘要应提示显存未检测: {s}");
    }
    #[test]
    fn summary_shows_vram_when_present() {
        let p = HardwareProfile {
            gpu_name: Some("RTX 3060".into()),
            vram_bytes: Some(gb(12.0)),
            ram_bytes: gb(32.0),
            cpu_cores: 8,
            ..Default::default()
        };
        let s = p.summary();
        assert!(s.contains("RTX 3060"), "应包含 GPU 名: {s}");
        assert!(s.contains("12.0 GB 显存"), "应包含显存: {s}");
    }
    #[test]
    fn save_and_load_roundtrip() {
        let p = detect_hardware();
        save_hardware(&p);
        let back = load_hardware();
        assert!(back.is_some(), "应能读回持久化的硬件画像");
    }
}
