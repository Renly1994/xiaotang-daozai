//! 壁纸亮度锚点：直接读 Windows 转码壁纸文件算亮度，不依赖抓屏。
//!
//! 液态玻璃的深/浅材质必须跟**壁纸**走，而抓屏在开机早期拿到的是桌面合成
//! 过渡帧（偏黑），采纳会把玻璃判成深色、且不易自行恢复（开机变黑、重启才好）。
//! 壁纸文件 `%APPDATA%\Microsoft\Windows\Themes\TranscodedWallpaper` 在登录前
//! 就已就绪，是稳定的真值来源：用它做锚值初始化主题亮度、校验抓屏采样。
//!
//! 只做「锚点」，不替代抓屏：壁纸的实际显示区域/动态效果仍以抓屏采样为准；
//! 这里只提供开机初值和差异过大的过渡帧过滤（见 wgc::theme_luma_commit）。

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;
use std::time::SystemTime;

/// 缓存的壁纸亮度（f32 位模式），0 表示还没算过。
static LUMA: AtomicU32 = AtomicU32::new(0);
/// 上次解码的壁纸文件修改时间，用于检测壁纸切换。
static MODIFIED: Mutex<Option<SystemTime>> = Mutex::new(None);

/// Windows 把当前壁纸转码后固定存在这里（无扩展名，内容多为 JPEG）。
/// 用 APPDATA 而非 config_dir()：后者受 DAOZAI_DATA_DIR 影响，壁纸不跟着走。
fn path() -> Option<PathBuf> {
    let appdata = std::env::var_os("APPDATA")?;
    Some(PathBuf::from(appdata).join(r"Microsoft\Windows\Themes\TranscodedWallpaper"))
}

/// 解码壁纸算平均亮度（0..1）。缩到 64×64 足够代表整体明暗，且解码快。
fn decode_luma(path: &std::path::Path) -> Option<f32> {
    let small = image::open(path).ok()?
        .resize_exact(64, 64, image::imageops::FilterType::Triangle)
        .to_rgb8();
    let mut sum = 0.0f64;
    let mut n = 0u32;
    // 每 4 个像素取一个，省一点；64×64 的 1/4 仍有 1024 个样本。
    for (x, y, pixel) in small.enumerate_pixels() {
        if x % 2 != 0 || y % 2 != 0 { continue; }
        sum += (pixel[0] as f64 + pixel[1] as f64 + pixel[2] as f64) / 3.0 / 255.0;
        n += 1;
    }
    if n == 0 { return None; }
    Some((sum / n as f64).clamp(0.0, 1.0) as f32)
}

/// 取壁纸亮度锚点：文件修改时间变了就重算并缓存；读不到/解码失败返回 None。
/// 轻量：正常情况下只做一次 metadata() 比较，不解码。
pub fn luma() -> Option<f32> {
    let path = path()?;
    let modified = std::fs::metadata(&path).ok()?.modified().ok();
    let changed = match MODIFIED.lock() {
        Ok(mut guard) => {
            if *guard == modified && LUMA.load(Ordering::Relaxed) != 0 { false }
            else { *guard = modified; true }
        }
        Err(_) => return None,
    };
    if changed {
        let Some(v) = decode_luma(&path) else { return None; };
        LUMA.store(v.to_bits(), Ordering::Relaxed);
    }
    let bits = LUMA.load(Ordering::Relaxed);
    if bits == 0 { None } else { Some(f32::from_bits(bits)) }
}

#[cfg(test)]
mod tests {
    // 只测纯逻辑边界：路径拼装不依赖真实壁纸。decode 本身靠 image crate，
    // 在真实机器上由 luma() 端到端覆盖；这里不引入对系统壁纸文件的测试依赖。
    #[test]
    fn luma_is_callable_without_wallpaper() {
        let _ = super::luma(); // 有壁纸返回 Some，没有返回 None，都不 panic
    }
}
