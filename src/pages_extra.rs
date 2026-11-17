//! 硬件监控 / 快捷控制两个页面。
//!
//! 坐标全部是面板局部逻辑点（419 宽），文字 y 是基线。
//! 数据全部来自后台线程推送的快照，界面线程只读，不阻塞绘制。
//! 「音频设备」原来是一个独立页面，现已并入快捷控制：长按「声音 / 麦克风」弹出设备菜单。

use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use eframe::egui::{self, Color32};

use crate::app::{App, CONTROL_H, PW};
use crate::paint::{self, Card, H};
use crate::theme::{self, size, W};

const CARD_BG: Color32 = Color32::from_rgb(24, 24, 27);
const HAIR: Color32 = Color32::from_rgba_premultiplied(255, 255, 255, 14);
const OFF_BG: Color32 = Color32::from_rgb(38, 38, 42);

fn gb(bytes: u64) -> f32 {
    bytes as f32 / 1_073_741_824.0
}

/// 占用越高越红：与 iOS「存储/电池」的配色习惯一致。
fn load_color(ratio: f32) -> Color32 {
    if ratio >= 0.88 {
        theme::RED
    } else if ratio >= 0.7 {
        theme::YELLOW
    } else {
        theme::BLUE
    }
}

/// 硬件监控与快捷控制两页共用的实时数据。字段为默认值时界面显示占位符。
#[derive(Default)]
pub(crate) struct Live {
    pub hw: crate::monitor::Sample,
    hw_rx: Option<Receiver<crate::monitor::Sample>>,
    pub ctl: crate::quick_ctl::Panel,
    ctl_rx: Option<Receiver<crate::quick_ctl::Panel>>,
    pub audio: crate::audio_switch::AudioState,
    audio_rx: Option<Receiver<crate::audio_switch::AudioState>>,
    /// 设备菜单：长按「声音 / 麦克风」后弹出，0 = 输出，1 = 输入，None = 关着。
    pub audio_pick: Option<u8>,
    /// 已经由长按顶开菜单的那一次按压：松手时不再当成单击，免得切完设备又把静音 / 麦克风关掉。
    pub hold_used: Option<u8>,
    /// 切换设备后请求一次同步刷新。
    pub refresh: bool,
    /// 滑块下发节流时间戳，避免拖动时刷爆子进程。
    pub vol_at: f64,
    /// 亮度本地值：显示器不支持 DDC/CI / WMI 时用它兜住滑块位置，换到支持的本子后自动被真实值覆盖。
    pub bright_value: Option<u32>,
    /// 挂灯亮度 / 色温滑块的节流时间戳：云端一条命令几百毫秒，拖动时不能照帧下发。
    pub lamp_bright_at: f64,
    pub lamp_ct_at: f64,
    /// 拖动中的本地值 + 时间戳：云端回包要几百毫秒，这段时间用本地值显示，滑块不往回弹。
    pub lamp_bright_local: Option<(u32, f64)>,
    pub lamp_ct_local: Option<(u32, f64)>,
    /// 极限清理：后台结果通道、进行中标记、以及（释放字节、完成时刻）。
    clean_rx: Option<Receiver<crate::mem_clean::Outcome>>,
    pub clean_running: bool,
    pub clean_done: Option<(u64, f64, u32)>,
}

/// 音频设备枚举比监控采样重，单独线程低频刷新。
fn audio_thread(ctx: egui::Context) -> Receiver<crate::audio_switch::AudioState> {
    let (tx, rx) = mpsc::sync_channel(1);
    let _ = std::thread::Builder::new()
        .name("daozai-audio-state".into())
        .spawn(move || loop {
            let state = crate::audio_switch::state();
            let _ = tx.try_send(state);
            ctx.request_repaint_after(Duration::from_secs(3));
            std::thread::sleep(Duration::from_secs(3));
        });
    rx
}

impl Live {
    /// 只有页面第一次出现时才拉起对应线程：没打开过的页面不占任何资源。
    fn hw_tick(&mut self, ctx: &egui::Context) {
        if self.hw_rx.is_none() {
            self.hw_rx = Some(crate::monitor::start(ctx.clone()));
        }
        if let Some(rx) = &self.hw_rx {
            while let Ok(sample) = rx.try_recv() {
                self.hw = sample;
            }
        }
    }

    fn ctl_tick(&mut self, ctx: &egui::Context) {
        if self.ctl_rx.is_none() {
            self.ctl_rx = Some(crate::quick_ctl::start(ctx.clone()));
        }
        if let Some(rx) = &self.ctl_rx {
            while let Ok(mut panel) = rx.try_recv() {
                // 采样拿到真实亮度就同步本地值；拿不到（显示器不支持）就用本地值兜住，
                // 滑块不会在拖动几秒后被采样打回去。
                match panel.brightness {
                    Some(b) => self.bright_value = Some(b),
                    None => panel.brightness = self.bright_value,
                }
                self.ctl = panel;
            }
        }
    }

    fn audio_tick(&mut self, ctx: &egui::Context) {
        if self.audio_rx.is_none() {
            self.audio_rx = Some(audio_thread(ctx.clone()));
        }
        if let Some(rx) = &self.audio_rx {
            while let Ok(state) = rx.try_recv() {
                self.audio = state;
            }
        }
        if self.refresh {
            self.audio = crate::audio_switch::state();
            self.refresh = false;
        }
    }
}

// ---------------------------------------------------------------------------
// 公共画法：指标卡、竖向滑块
// ---------------------------------------------------------------------------

/// 一张 iOS 风格的指标卡：左上标签、右上大数值、左下说明、底部细进度条。
fn metric(
    c: &Card,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    label: &str,
    value: &str,
    sub: &str,
    ratio: Option<f32>,
    accent: Color32,
) {
    metric_ex(c, x, y, w, h, label, value, sub, theme::DIM, ratio, accent, 0.0);
}

/// 同上，多两个可调项：`value_inset` 给右上角留出按钮位置，`sub_color` 让副标题表达清理进度。
#[allow(clippy::too_many_arguments)]
fn metric_ex(
    c: &Card,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    label: &str,
    value: &str,
    sub: &str,
    sub_color: Color32,
    ratio: Option<f32>,
    accent: Color32,
    value_inset: f32,
) {
    c.fill(x, y, w, h, 14.0, CARD_BG);
    c.stroke(x, y, w, h, 14.0, 0.8, HAIR);
    c.circle(x + 15.0, y + 15.0, 2.6, accent);
    c.text(x + 22.0, y + 19.0, label, size::TINY, W::Regular, theme::DIM, H::Left);
    let value = c.fit(value, 18.0, W::Bold, w - 46.0 - value_inset);
    c.text(x + w - 12.0 - value_inset, y + 23.0, &value, 18.0, W::Bold, theme::WHITE, H::Right);
    let sub = c.fit(sub, size::TINY, W::Regular, w - 24.0 - value_inset);
    c.text(x + 12.0, y + 41.0, &sub, size::TINY, W::Regular, sub_color, H::Left);
    if let Some(v) = ratio {
        let by = y + h - 13.0;
        let track_w = w - 24.0;
        c.fill(x + 12.0, by, track_w, 4.0, 2.0, theme::TRACK);
        let filled = track_w * v.clamp(0.0, 1.0);
        if filled > 2.0 {
            c.fill(x + 12.0, by, filled, 4.0, 2.0, accent);
        }
    }
}

/// 滑杆右侧文案预留宽度：按最长文案（"6500K"）统一取值，保证亮度与色温两根轨道等宽对齐。
const RIGHT_PAD: f32 = 46.0;

/// 滑块轨道绘制（不含交互）：左图标、轨道、填充、圆形滑块、右侧百分比。
fn slider_paint(c: &Card, x: f32, y: f32, w: f32, value: u32, accent: Color32, hover: f32, icon: u8) {
    let cy = y;
    let icon_col = paint::mix(theme::DIM, theme::WHITE, hover);
    match icon {
        0 => sun(c, x + 7.0, cy, 6.0, icon_col),
        _ => paint::speaker(c, x + 7.0, cy, false, 1.0, icon_col),
    }
    let tx = x + 22.0;
    let tw = w - 22.0 - RIGHT_PAD;
    c.fill(tx, cy - 3.0, tw, 6.0, 3.0, theme::TRACK);
    let filled = tw * (value.min(100) as f32 / 100.0);
    if filled > 2.0 {
        c.fill(tx, cy - 3.0, filled, 6.0, 3.0, paint::mix(accent, theme::WHITE, hover * 0.12));
    }
    c.circle(tx + filled.max(0.0), cy, 6.0, theme::WHITE);
    c.text(x + w - 2.0, cy + 4.0, &format!("{}%", value), size::TINY, W::Regular, theme::GRAY, H::Right);
}

/// 细长滑块（亮度 / 音量）：左图标、中间轨道、右侧百分比。按下即跳到指针位置，按住可拖动。
fn slider(c: &Card, x: f32, y: f32, w: f32, value: u32, accent: Color32, id: &'static str, icon: u8) -> Option<u32> {
    // 用 hit_drag 而不是 hit：hit 只带 Sense::click()，拖动根本不触发 dragged()，滑块会拖不动。
    let hit = c.hit_drag(x - 6.0, y - 12.0, w + 12.0, 24.0, id);
    let (hover, _) = c.hover_t(&hit);
    slider_paint(c, x, y, w, value, accent, if hit.dragged() { 1.0 } else { hover }, icon);
    let (tx, tw) = (x + 22.0, w - 22.0 - RIGHT_PAD);
    if c.live && (hit.clicked() || hit.dragged() || hit.is_pointer_button_down_on()) {
        if let Some(pos) = hit.interact_pointer_pos() {
            let local = c.to_local(pos).x;
            let ratio = ((local - tx) / tw).clamp(0.0, 1.0);
            return Some((ratio * 100.0).round() as u32);
        }
    }
    None
}

/// 色温滑块：区间是 2700–6500K 而不是 0–100%，所以另画一份，右侧直接显示当前 K 值。
fn ct_slider(c: &Card, x: f32, y: f32, w: f32, ct: u32, accent: Color32, id: &'static str) -> Option<u32> {
    let (lo, hi) = (crate::mi_light::CT_MIN, crate::mi_light::CT_MAX);
    let ct = ct.clamp(lo, hi);
    let hit = c.hit_drag(x - 6.0, y - 12.0, w + 12.0, 24.0, id);
    let (hover, _) = c.hover_t(&hit);
    let hov = if hit.dragged() { 1.0 } else { hover };
    let (tx, tw) = (x + 22.0, w - 22.0 - RIGHT_PAD);
    let ratio = ((ct - lo) as f32 / (hi - lo) as f32).clamp(0.0, 1.0);
    let filled = tw * ratio;
    let icon_col = paint::mix(theme::DIM, theme::WHITE, hov);
    sun(c, x + 7.0, y, 6.0, icon_col);
    c.fill(tx, y - 3.0, tw, 6.0, 3.0, theme::TRACK);
    if filled > 2.0 {
        c.fill(tx, y - 3.0, filled, 6.0, 3.0, paint::mix(accent, theme::WHITE, hov * 0.12));
    }
    c.circle(tx + filled, y, 6.0, theme::WHITE);
    c.text(x + w - 2.0, y + 4.0, &format!("{}K", ct), size::TINY, W::Regular, theme::GRAY, H::Right);
    if c.live && (hit.clicked() || hit.dragged() || hit.is_pointer_button_down_on()) {
        if let Some(pos) = hit.interact_pointer_pos() {
            let local = c.to_local(pos).x;
            let r = ((local - tx) / tw).clamp(0.0, 1.0);
            return Some(lo + (r * (hi - lo) as f32).round() as u32);
        }
    }
    None
}

fn sun(c: &Card, cx: f32, cy: f32, r: f32, color: Color32) {
    c.circle(cx, cy, r * 0.56, color);
    for i in 0..8 {
        let a = i as f32 * std::f32::consts::TAU / 8.0;
        let (s, n) = (a.sin(), a.cos());
        c.seg((cx + n * r * 0.92, cy + s * r * 0.92), (cx + n * r * 1.32, cy + s * r * 1.32), 1.3, color);
    }
}

fn wifi_icon(c: &Card, cx: f32, cy: f32, k: f32, color: Color32) {
    for r in [4.5f32, 8.5, 12.5] {
        c.arc(cx, cy + 6.5 * k, r * k, -2.42, -0.72, 1.6, color);
    }
    c.circle(cx, cy + 6.5 * k, 1.7 * k, color);
}

fn bt_icon(c: &Card, cx: f32, cy: f32, k: f32, color: Color32) {
    let (w, h) = (5.2 * k, 9.0 * k);
    c.path(&[(cx, cy - h), (cx + w, cy - h * 0.45), (cx - w, cy + h * 0.45), (cx, cy + h)], 1.5, color);
    c.seg((cx, cy - h), (cx, cy + h), 1.5, color);
    c.seg((cx + w, cy - h * 0.45), (cx - w, cy + h * 0.45), 1.5, color);
}

fn moon_icon(c: &Card, cx: f32, cy: f32, k: f32, color: Color32) {
    // 月牙：一段粗弧，留出右上缺口，避免用底色遮罩破坏按钮背景。
    c.arc(cx, cy, 7.0 * k, 0.75, 4.85, 2.6 * k, color);
}

fn mic_icon(c: &Card, cx: f32, cy: f32, k: f32, muted: bool, color: Color32) {
    c.fill(cx - 2.6 * k, cy - 9.0 * k, 5.2 * k, 10.0 * k, 2.6 * k, color);
    c.arc(cx, cy + 1.0 * k, 5.6 * k, 0.15, std::f32::consts::PI - 0.15, 1.6, color);
    c.seg((cx, cy + 6.6 * k), (cx, cy + 9.0 * k), 1.6, color);
    if muted {
        c.seg((cx - 7.0 * k, cy - 7.0 * k), (cx + 7.0 * k, cy + 7.0 * k), 1.6, theme::RED);
    }
}

/// 左右箭头：两段折线。比实心三角更细，缩到小格子里也不糊。
fn chevron(c: &Card, cx: f32, cy: f32, right: bool, color: Color32) {
    let (d, h) = (3.6f32, 5.4f32);
    let sx = if right { -d } else { d };
    c.path(&[(cx + sx, cy - h), (cx - sx, cy), (cx + sx, cy + h)], 1.7, color);
}

/// 锁屏：上方半圆锁梁 + 下方圆角锁体。
fn lock_icon(c: &Card, cx: f32, cy: f32, k: f32, color: Color32) {
    let pi = std::f32::consts::PI;
    // 锁梁只画露在锁体外面的那半圈，两端各往里收一点，看着才是插进锁体里。
    c.arc(cx, cy + 0.5 * k, 4.4 * k, -pi + 0.32, -0.32, 1.6, color);
    c.fill(cx - 5.8 * k, cy + 0.6 * k, 11.6 * k, 8.6 * k, 2.4 * k, color);
}

fn shot_icon(c: &Card, cx: f32, cy: f32, k: f32, color: Color32) {
    let r = 8.0 * k;
    let arm = 4.0 * k;
    for (sx, sy) in [(-1.0f32, -1.0f32), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
        let px = cx + sx * r;
        let py = cy + sy * r;
        c.seg((px, py), (px - sx * arm, py), 1.6, color);
        c.seg((px, py), (px, py - sy * arm), 1.6, color);
    }
}

// ---------------------------------------------------------------------------
// 页 9：硬件监控
// ---------------------------------------------------------------------------

impl App {
    pub(crate) fn page_hardware(&mut self, c: &Card) {
        self.live.hw_tick(c.ui.ctx());
        self.clean_tick();
        let s = self.live.hw.clone();
        let (cw, h, gap) = (190.0f32, 60.0f32, 9.0f32);
        let (x1, x2) = (15.0f32, 15.0 + cw + gap);
        let (r1, r2, r3) = (36.0f32, 104.0f32, 172.0f32);

        let cpu = s.cpu_value();
        let freq = match s.cpu_freq {
            Some(mhz) if mhz > 0 => format!("{:.1} GHz", mhz as f32 / 1000.0),
            _ => "-- GHz".to_string(),
        };
        let cpu_sub = if s.threads > 0 {
            format!("{} · {} 线程", freq, s.threads)
        } else {
            freq
        };
        metric(c, x1, r1, cw, h, "CPU", &format!("{:.0}%", cpu), &cpu_sub, Some(cpu / 100.0), load_color(cpu / 100.0));

        let mr = s.mem_ratio();
        // 副标题平时放占用率，清理时临时改成进度与结果，3 秒后自动回落。
        let fresh = self
            .live
            .clean_done
            .filter(|(_, at, _)| self.t - at < 3.0)
            .map(|(freed, _, _)| freed);
        let (mem_sub, mem_color) = if self.live.clean_running {
            ("正在极限清理…".to_string(), theme::BLUE)
        } else if let Some(freed) = fresh {
            if freed >= 52_428_800 {
                (format!("已释放 {:.2} GB", gb(freed)), theme::GREEN)
            } else {
                ("已经很干净了".to_string(), theme::SOFT_GREEN)
            }
        } else {
            (format!("共 {:.0} GB · 占 {:.0}%", gb(s.mem_total), mr * 100.0), theme::DIM)
        };
        metric_ex(
            c, x2, r1, cw, h,
            "内存",
            &format!("{:.1} GB", gb(s.mem_used)),
            &mem_sub,
            mem_color,
            Some(mr),
            load_color(mr),
            MEM_CLEAN_INSET,
        );
        if self.mem_clean_button(c, x2 + cw - 21.0, r1 + 15.0) {
            self.start_clean(c);
        }

        match &s.gpu {
            Some(gpu) => {
                let util = gpu.util.unwrap_or(0.0);
                let temp = gpu.temp.map(|t| format!("{:.0}°C", t)).unwrap_or_else(|| "--°C".to_string());
                metric(
                    c, x1, r2, cw, h,
                    "显卡",
                    &format!("{:.0}%", util),
                    &format!("{} · 显存 {:.1}/{:.1}G", temp, gb(gpu.mem_used), gb(gpu.mem_total)),
                    Some(util / 100.0),
                    load_color(util / 100.0),
                );
                let power = gpu.power.map(|p| format!("{:.0}W", p)).unwrap_or_else(|| "--W".to_string());
                let fan = gpu.fan.map(|f| format!("{:.0}%", f)).unwrap_or_else(|| "--%".to_string());
                metric(
                    c, x2, r2, cw, h,
                    "散热",
                    &temp,
                    &format!("功耗 {} · 风扇 {}", power, fan),
                    gpu.temp.map(|t| t / 100.0),
                    theme::SOFT_GREEN,
                );
            }
            None => {
                metric(c, x1, r2, cw, h, "显卡", "--", "未检测到独立显卡", None, theme::DIM);
                metric(c, x2, r2, cw, h, "散热", "--", "暂无温度传感器", None, theme::DIM);
            }
        }

        for (i, disk) in s.disks.iter().take(2).enumerate() {
            let ratio = disk.ratio();
            metric(
                c,
                if i == 0 { x1 } else { x2 },
                r3,
                cw,
                h,
                &format!("磁盘 {}:", disk.letter),
                &format!("{:.0}%", ratio * 100.0),
                &format!("可用 {:.0} / {:.0} GB", gb(disk.total.saturating_sub(disk.used)), gb(disk.total)),
                Some(ratio),
                load_color(ratio),
            );
        }
        if s.disks.is_empty() {
            metric(c, x1, r3, cw, h, "磁盘", "--", "读取中", None, theme::DIM);
        }
    }
}

// ---------------------------------------------------------------------------
// 极限清理：内存卡右上角的一颗圆钮，点一下把可回收的物理内存还给系统
// ---------------------------------------------------------------------------

/// 内存卡右上角给清理钮留的宽度。
const MEM_CLEAN_INSET: f32 = 26.0;

/// 清理图标：一道闪电，缩到 18 点的小圆钮里也认得出。
fn bolt(c: &Card, cx: f32, cy: f32, k: f32, color: Color32) {
    c.seg((cx + 1.9 * k, cy - 5.4 * k), (cx - 2.0 * k, cy + 0.7 * k), 1.5, color);
    c.seg((cx - 2.0 * k, cy + 0.7 * k), (cx + 1.3 * k, cy + 0.4 * k), 1.5, color);
    c.seg((cx + 1.3 * k, cy + 0.4 * k), (cx - 1.9 * k, cy + 5.4 * k), 1.5, color);
}

impl App {
    /// 内存卡右上角的清理钮，返回是否被点击。
    /// 进行中画一段旋转弧、完成后变绿勾，全程不额外占版面。
    fn mem_clean_button(&self, c: &Card, bx: f32, by: f32) -> bool {
        let tip = if self.live.clean_running { "正在极限清理…" } else { "极限清理内存" };
        let resp = c.hit(bx - 10.0, by - 10.0, 20.0, 20.0, "hw-mem-clean").on_hover_text(tip);
        let (hover, _) = c.hover_t(&resp);
        let done = self.live.clean_done.map(|(_, at, _)| self.t - at < 3.0).unwrap_or(false);
        c.circle(bx, by, 9.0, paint::mix(theme::TRACK, theme::BTN_HOVER, hover));
        let color = if self.live.clean_running {
            theme::BLUE
        } else if done {
            theme::GREEN
        } else {
            paint::mix(theme::DIM, theme::WHITE, hover)
        };
        if done && !self.live.clean_running {
            c.seg((bx - 3.6, by + 0.3), (bx - 0.9, by + 2.9), 1.6, color);
            c.seg((bx - 0.9, by + 2.9), (bx + 3.7, by - 2.7), 1.6, color);
        } else {
            bolt(c, bx, by, 1.0, color);
        }
        if self.live.clean_running {
            // 清理通常一两百毫秒，转一圈才知道在动。
            let a0 = (self.t as f32 * 5.0) % std::f32::consts::TAU;
            c.arc(bx, by, 10.6, a0, a0 + 1.9, 1.5, theme::BLUE);
            c.ui.ctx().request_repaint();
        }
        resp.clicked()
    }

    /// 收后台结果；收到就把通道关掉，界面切到「已释放」。
    fn clean_tick(&mut self) {
        if let Some(rx) = &self.live.clean_rx {
            let mut latest = None;
            while let Ok(outcome) = rx.try_recv() {
                latest = Some(outcome);
            }
            if let Some(o) = latest {
                self.live.clean_rx = None;
                self.live.clean_running = false;
                self.live.clean_done = Some((o.freed, self.t, o.trimmed));
            }
        }
    }

    /// 清理放后台线程跑，界面不卡；跑完主动请求一帧把数字刷出来。
    fn start_clean(&mut self, c: &Card) {
        self.live.clean_running = true;
        self.live.clean_done = None;
        let ctx = c.ui.ctx().clone();
        let (tx, rx) = mpsc::sync_channel(1);
        let spawned = std::thread::Builder::new()
            .name("daozai-mem-clean".into())
            .spawn(move || {
                let outcome = crate::mem_clean::run();
                let _ = tx.try_send(outcome);
                ctx.request_repaint();
            })
            .is_ok();
        if spawned {
            self.live.clean_rx = Some(rx);
        } else {
            self.live.clean_running = false;
        }
    }
}

// ---------------------------------------------------------------------------
// 页 10：快捷控制（控制中心式方格按钮）
// ---------------------------------------------------------------------------

/// 画一个控制中心方格按钮，返回（按钮响应、图标颜色）。长按判定要用到响应，所以不在这里吞成 bool。
fn button(c: &Card, x: f32, y: f32, w: f32, h: f32, id: &'static str, on: bool, accent: Color32) -> (egui::Response, Color32) {
    let r = c.hit(x, y, w, h, id);
    let (ht, pr) = c.hover_t(&r);
    let bg = if on {
        paint::mix(accent, theme::WHITE, ht * 0.14 + pr * 0.06)
    } else {
        paint::mix(OFF_BG, theme::BTN_HOVER, ht)
    };
    c.fill(x, y, w, h, 16.0, bg);
    if !on {
        c.stroke(x, y, w, h, 16.0, 0.8, HAIR);
    }
    (r, if on { theme::WHITE } else { theme::GRAY })
}

fn button_label(c: &Card, x: f32, y: f32, w: f32, h: f32, text: &str, on: bool) {
    let color = if on { theme::WHITE } else { theme::DIM };
    let text = c.fit(text, size::TINY, W::Regular, w - 16.0);
    c.text(x + w / 2.0, y + h - 8.0, &text, size::TINY, W::Regular, color, H::Center);
}

impl App {
    pub(crate) fn page_control(&mut self, c: &Card) {
        self.live.ctl_tick(c.ui.ctx());
        // 设备菜单开着、或刚切完设备等一次刷新时，才去取音频快照；平时一点开销都没有。
        if self.live.audio_pick.is_some() || self.live.refresh {
            self.live.audio_tick(c.ui.ctx());
        }
        let panel = self.live.ctl.clone();
        // 2 行 4 列共 8 格：Wi-Fi / 蓝牙 / 勿扰 / 截图，麦克风 / 声音 / 切换桌面 / 锁屏。
        let (bw, bh) = (87.5f32, 52.0f32);
        let xs = [15.0f32, 115.5, 216.0, 316.5];
        let ys = [32.0f32, 92.0f32];

        // Wi-Fi
        let (r, icon) = button(c, xs[0], ys[0], bw, bh, "ctl-wifi", panel.wifi_on, theme::BLUE);
        wifi_icon(c, xs[0] + bw / 2.0, ys[0] + 19.0, 1.05, icon);
        let ssid = if panel.wifi_ssid.is_empty() { "Wi-Fi".to_string() } else { panel.wifi_ssid.clone() };
        button_label(c, xs[0], ys[0], bw, bh, &ssid, panel.wifi_on);
        if r.clicked() {
            crate::quick_ctl::toggle_wifi(!panel.wifi_on);
            self.live.ctl.wifi_on = !panel.wifi_on;
            self.say(if panel.wifi_on { "正在关闭 Wi-Fi" } else { "正在打开 Wi-Fi" });
        }

        // 蓝牙
        let (r, icon) = button(c, xs[1], ys[0], bw, bh, "ctl-bt", panel.bluetooth_on, theme::BLUE);
        bt_icon(c, xs[1] + bw / 2.0, ys[0] + 19.0, 1.0, icon);
        button_label(c, xs[1], ys[0], bw, bh, "蓝牙", panel.bluetooth_on);
        if r.clicked() {
            crate::quick_ctl::toggle_bluetooth(!panel.bluetooth_on);
            self.live.ctl.bluetooth_on = !panel.bluetooth_on;
        }

        // 勿扰
        let (r, icon) = button(c, xs[2], ys[0], bw, bh, "ctl-dnd", panel.dnd, theme::BLUE);
        moon_icon(c, xs[2] + bw / 2.0, ys[0] + 19.0, 1.0, icon);
        button_label(c, xs[2], ys[0], bw, bh, "勿扰", panel.dnd);
        if r.clicked() {
            crate::quick_ctl::set_dnd(!panel.dnd);
            self.live.ctl.dnd = !panel.dnd;
        }

        // 截图
        let (r, icon) = button(c, xs[3], ys[0], bw, bh, "ctl-shot", false, theme::BLUE);
        shot_icon(c, xs[3] + bw / 2.0, ys[0] + 19.0, 0.95, icon);
        button_label(c, xs[3], ys[0], bw, bh, "截图", false);
        if r.clicked() {
            // 先让岛仔自己隐退，再拉起系统原生截图（Win+Shift+S 同款）：可框选区域、选窗口、调大小并批注。
            // 截图工具关闭后岛仔自己回来，全程不用手动关岛仔。
            if crate::quick_ctl::screen_clip_hide_self() {
                self.say("框选截图");
            } else {
                self.say("截图工具未响应");
            }
        }

        // 麦克风：单击是静音开关，长按弹出输入设备菜单（原「音频设备」页的输入半区）。
        let mic_on = !panel.mic_muted;
        let (r, icon) = button(c, xs[0], ys[1], bw, bh, "ctl-mic", mic_on, theme::GREEN);
        mic_icon(c, xs[0] + bw / 2.0, ys[1] + 19.0, 0.95, panel.mic_muted, icon);
        button_label(c, xs[0], ys[1], bw, bh, "麦克风", mic_on);
        if long_press(c, &r) {
            self.live.audio_pick = Some(1);
            self.live.hold_used = Some(1);
        }
        if r.clicked() && !self.tap_used(1) {
            crate::quick_ctl::toggle_mic();
            self.live.ctl.mic_muted = !panel.mic_muted;
        }

        // 声音：单击是静音开关，长按弹出输出设备菜单（原「音频设备」页的输出半区）。
        let sound_on = !panel.muted;
        let (r, icon) = button(c, xs[1], ys[1], bw, bh, "ctl-mute", sound_on, theme::GREEN);
        paint::speaker(c, xs[1] + bw / 2.0, ys[1] + 19.0, panel.muted, 1.0, icon);
        button_label(c, xs[1], ys[1], bw, bh, "声音", sound_on);
        if long_press(c, &r) {
            self.live.audio_pick = Some(0);
            self.live.hold_used = Some(0);
        }
        if r.clicked() && !self.tap_used(0) {
            crate::quick_ctl::toggle_mute();
            self.live.ctl.muted = !panel.muted;
        }

        // 切换桌面：一格分左右两半，左半切上一个桌面、右半切下一个桌面。
        // 整格不挂 button()，否则整块热区会和外层的左右两半抢点击。
        let (dx, dy) = (xs[2], ys[1]);
        let prev = c.hit(dx + 3.0, dy + 3.0, bw / 2.0 - 4.0, bh - 6.0, "ctl-desk-prev");
        let next = c.hit(dx + bw / 2.0, dy + 3.0, bw / 2.0 - 3.0, bh - 6.0, "ctl-desk-next");
        let (hp, _pp) = c.hover_t(&prev);
        let (hn, _pn) = c.hover_t(&next);
        c.fill(dx, dy, bw, bh, 16.0, paint::mix(OFF_BG, theme::BTN_HOVER, hp.max(hn)));
        c.stroke(dx, dy, bw, bh, 16.0, 0.8, HAIR);
        // 中间一道细竖线，一眼看出是左右两个方向，而不是一个整体按钮。
        c.fill(dx + bw / 2.0 - 0.5, dy + 15.0, 1.0, bh - 28.0, 0.5, HAIR);
        chevron(c, dx + bw * 0.25, dy + 18.0, false, paint::mix(theme::GRAY, theme::WHITE, hp));
        chevron(c, dx + bw * 0.75, dy + 18.0, true, paint::mix(theme::GRAY, theme::WHITE, hn));
        button_label(c, dx, dy, bw, bh, "切换桌面", false);
        if prev.clicked() {
            crate::quick_ctl::desktop_switch(false);
            self.say("切到上一个桌面");
        }
        if next.clicked() {
            crate::quick_ctl::desktop_switch(true);
            self.say("切到下一个桌面");
        }

        // 一键锁屏
        let (r, icon) = button(c, xs[3], ys[1], bw, bh, "ctl-lock", false, theme::BLUE);
        lock_icon(c, xs[3] + bw / 2.0, ys[1] + 19.0, 0.95, icon);
        button_label(c, xs[3], ys[1], bw, bh, "锁屏", false);
        if r.clicked() {
            // 锁屏后岛仔留在原桌面，解锁回来还在；返回 false 说明系统拒绝了调用。
            if crate::quick_ctl::lock_screen() {
                self.say("锁屏");
            } else {
                self.say("锁屏没有响应");
            }
        }

        // 按钮区与滑杆区之间的细分隔，保持 iOS 控制中心的呼吸感。
        c.fill(15.0, 154.0, 389.0, 1.0, 0.5, HAIR);

        // 亮度：滑块始终可拖。本机显示器不支持调节时下发静默失败、位置照旧保留；
        // 换到支持的本子（外接屏 DDC/CI 或内置屏 WMI）后，同一个滑块直接见效。
        let bright = panel.brightness.or(self.live.bright_value).unwrap_or(100);
        if let Some(v) = slider(c, 15.0, 176.0, 389.0, bright, theme::YELLOW, "ctl-bright", 0) {
            if self.t - self.live.vol_at > 0.12 {
                crate::quick_ctl::request_brightness(v);
                self.live.vol_at = self.t;
                self.live.bright_value = Some(v);
                self.live.ctl.brightness = Some(v);
            }
        }

        if let Some(v) = slider(c, 15.0, 208.0, 389.0, panel.volume, theme::BLUE, "ctl-vol", 1) {
            if self.t - self.live.vol_at > 0.12 {
                crate::quick_ctl::set_volume(v);
                self.live.vol_at = self.t;
                self.live.ctl.volume = v;
            }
        }

        // 长按「声音 / 麦克风」弹出的设备菜单：画在整页最上层。
        self.audio_picker(c);
    }
}

// ---------------------------------------------------------------------------
// 页 12：挂灯（米家挂灯的开关、亮度与色温）
// ---------------------------------------------------------------------------

impl App {
    pub(crate) fn page_lamp(&mut self, c: &Card) {
        // 云端控制：只在本页可见期间轮询（start 幂等，touch 续命）。
        crate::mi_light::start(c.ui.ctx());
        crate::mi_light::touch();
        let lamp = crate::mi_light::snapshot();

        // 标题右侧的状态：登录态 → 在线态 → 最近一条提示，越靠后越具体。
        let status = if !lamp.logged {
            if lamp.note.is_empty() { "未登录".to_string() } else { lamp.note.clone() }
        } else if !lamp.online {
            "离线".to_string()
        } else if !lamp.note.is_empty() {
            lamp.note.clone()
        } else {
            "已连接".to_string()
        };
        // 状态文字紧跟在「米家 挂灯」标题右边，与标题同一基线、字号不变。
        // 右边界仍留到 372：右上角设置图标（中心 394，点击区 380~408）必须完全让开。
        let title_right = 15.0 + c.text_w("米家", size::TITLE, W::Bold) + 1.5 + c.text_w("挂灯", size::TITLE, W::Bold);
        let status_x = title_right + 9.0;
        let status = c.fit(&status, size::TINY, W::Regular, (372.0 - status_x).max(24.0));
        let status_col = if !lamp.logged || !lamp.online { theme::DIM } else { theme::GRAY };
        // 用标题字形的可见中线定位，小字与大字看起来才真正在一条线上（只按基线对齐会略微下沉）。
        let title_galley = c.galley("挂灯", size::TITLE, W::Bold, theme::YELLOW);
        let title_asc = title_galley.rows.first().and_then(|row| row.glyphs.first()).map(|glyph| glyph.pos.y).unwrap_or(size::TITLE * c.s);
        let title_cy = 25.0 - title_asc / c.s + title_galley.mesh_bounds.center().y / c.s;
        c.centered_text(status_x, title_cy, &status, size::TINY, W::Regular, status_col, H::Left);

        // 开关：一整颗大药丸，够宽够高，随手一点就能开灯关灯。
        let (bx, by, bw, bh) = (15.0f32, 40.0f32, 389.0f32, 46.0f32);
        let r = c.hit(bx, by, bw, bh, "lamp-power");
        let (ht, _) = c.hover_t(&r);
        let able = lamp.logged && lamp.online;
        let bg = if lamp.on {
            paint::mix(theme::YELLOW, theme::WHITE, ht * 0.14)
        } else {
            paint::mix(OFF_BG, theme::BTN_HOVER, ht)
        };
        c.fill(bx, by, bw, bh, 16.0, bg);
        if !lamp.on {
            c.stroke(bx, by, bw, bh, 16.0, 0.8, HAIR);
        }
        let label = if !lamp.logged {
            "登录米家"
        } else if lamp.busy {
            "同步中"
        } else if lamp.on {
            "关灯"
        } else {
            "开灯"
        };
        let icon_col = if lamp.on { theme::WHITE } else { paint::mix(theme::DIM, theme::WHITE, ht) };
        sun(c, bx + 30.0, by + bh / 2.0, 9.0, icon_col);
        c.text(
            bx + bw / 2.0 + 16.0,
            by + bh / 2.0 + 4.5,
            label,
            13.0,
            W::Bold,
            if lamp.on { theme::WHITE } else { theme::DIM },
            H::Center,
        );
        if r.clicked() {
            if !lamp.logged {
                crate::mi_light::request_login();
                self.say("正在登录米家");
            } else if !able {
                self.say("挂灯离线");
            } else {
                crate::mi_light::request_power(!lamp.on);
                self.say(if lamp.on { "正在关灯" } else { "正在开灯" });
            }
        }

        c.text(15.0, 104.0, "亮度", size::TINY, W::Regular, theme::DIM, H::Left);
        // 亮度：拖动时 0.12s 节流下发，本地值先顶着显示。
        let bright = match self.live.lamp_bright_local {
            Some((v, ts)) if self.t - ts < 1.5 => v,
            _ => lamp.bright,
        };
        if let Some(v) = slider(c, 15.0, 122.0, 389.0, bright, theme::YELLOW, "lamp-bright", 0) {
            self.live.lamp_bright_local = Some((v, self.t));
            if self.t - self.live.lamp_bright_at > 0.12 {
                crate::mi_light::request_bright(v);
                self.live.lamp_bright_at = self.t;
            }
        }

        c.text(15.0, 148.0, "色温", size::TINY, W::Regular, theme::DIM, H::Left);
        // 色温：2700–6500K，右侧直接给 K 值。
        let ctv = match self.live.lamp_ct_local {
            Some((v, ts)) if self.t - ts < 1.5 => v,
            _ => lamp.ct,
        };
        if let Some(v) = ct_slider(c, 15.0, 166.0, 389.0, ctv, theme::CYAN, "lamp-ct") {
            self.live.lamp_ct_local = Some((v, self.t));
            if self.t - self.live.lamp_ct_at > 0.12 {
                crate::mi_light::request_ct(v);
                self.live.lamp_ct_at = self.t;
            }
        }

        // 未登录 / 离线时给一条明确的操作提示，免得误以为是滑杆失灵。
        let hint = if !lamp.logged {
            Some("先在手机米家登录账号，这里才能控制挂灯")
        } else if !lamp.online {
            Some("挂灯离线：检查灯体电源与网络，恢复后会自动重连")
        } else {
            None
        };
        if let Some(hint) = hint {
            let hint = c.fit(hint, size::TINY, W::Regular, 380.0);
            c.text(15.0, 196.0, &hint, size::TINY, W::Regular, theme::DIM, H::Left);
        }
    }
}

// ---------------------------------------------------------------------------
// 长按「声音 / 麦克风」弹出的设备菜单（原「音频设备」页并入这里）
// ---------------------------------------------------------------------------

/// 长按阈值：按住超过这么久就弹出设备菜单，松手那次不再算单击。
const HOLD_SECS: f64 = 0.5;

/// 按钮是否已经被按住够久。只在指针确实压在按钮上时成立。
fn long_press(c: &Card, r: &egui::Response) -> bool {
    if !c.live || !r.is_pointer_button_down_on() {
        return false;
    }
    let (now, started) = c.ui.input(|i| (i.time, i.pointer.press_start_time()));
    match started {
        Some(at) => now - at >= HOLD_SECS,
        None => false,
    }
}

impl App {
    /// 长按顶开菜单后的那次松手不算单击：吞掉它并清掉标记。
    fn tap_used(&mut self, kind: u8) -> bool {
        if self.live.hold_used == Some(kind) {
            self.live.hold_used = None;
            true
        } else {
            false
        }
    }

    /// 设备菜单：盖住整页，点设备即切换，点空白处关掉。
    fn audio_picker(&mut self, c: &Card) {
        let Some(kind) = self.live.audio_pick else {
            return;
        };
        let input = kind == 1;
        let state = self.live.audio.clone();

        // 蒙层先占住整页：菜单开着的时候，页面上的按钮一律不响应。
        let mask = c.hit(0.0, 0.0, PW, CONTROL_H, "audio-mask");
        c.fill(0.0, 0.0, PW, CONTROL_H, 0.0, Color32::from_black_alpha(168));

        let devices: Vec<(String, String)> = if input {
            state.inputs.iter().map(|d| (d.id.clone(), d.name.clone())).collect()
        } else {
            state.outputs.iter().map(|d| (d.id.clone(), d.name.clone())).collect()
        };
        let current = if input { state.current_input.clone() } else { state.current_output.clone() };

        // 卡片高度跟着设备条数走：最多露 4 条，超出的用底部一行字交代。
        let rows = devices.len().min(4) as f32;
        let (x, y, w) = (15.0f32, 24.0f32, 389.0f32);
        let h = 50.0 + rows.max(1.0) * 36.0 + if devices.len() > 4 { 14.0 } else { 0.0 };
        c.fill(x, y, w, h, 18.0, CARD_BG);
        c.stroke(x, y, w, h, 18.0, 0.8, HAIR);

        let head = if input { "麦克风" } else { "声音" };
        c.text(x + 16.0, y + 24.0, head, size::SMALL, W::Bold, theme::WHITE, H::Left);
        let current_name = if input { state.current_input_name() } else { state.current_output_name() };
        let sub = c.fit(&current_name, size::TINY, W::Regular, 230.0);
        c.text(x + w - 16.0, y + 23.0, &sub, size::TINY, W::Regular, theme::DIM, H::Right);

        if devices.is_empty() {
            c.text(x + w / 2.0, y + 52.0, "正在读取音频设备…", size::SMALL, W::Regular, theme::DIM, H::Center);
        }

        for (i, (id, name)) in devices.iter().take(4).enumerate() {
            let ry = y + 36.0 + i as f32 * 36.0;
            let r = c.hit(x + 10.0, ry, w - 20.0, 32.0, ("audio-pick", kind, i));
            let (ht, _) = c.hover_t(&r);
            let on = current.as_deref() == Some(id.as_str());
            let bg = if on {
                paint::alpha(theme::BLUE, 0.22 + ht * 0.06)
            } else {
                paint::mix(theme::CARD, theme::BTN_HOVER, ht * 0.85)
            };
            c.fill(x + 10.0, ry, w - 20.0, 32.0, 12.0, bg);
            if on {
                c.stroke(x + 10.0, ry, w - 20.0, 32.0, 12.0, 1.0, paint::alpha(theme::BLUE, 0.55));
            }
            let cy = ry + 16.0;
            let icon = if on { theme::WHITE } else { theme::GRAY };
            if input {
                mic_icon(c, x + 32.0, cy, 0.8, false, icon);
            } else {
                paint::speaker(c, x + 32.0, cy, false, 1.0, icon);
            }
            let label = c.fit(name, size::BODY, W::Regular, w - 96.0);
            c.text(x + 50.0, cy + 4.0, &label, size::BODY, W::Regular, if on { theme::WHITE } else { theme::GRAY }, H::Left);
            if on {
                // 对勾：当前正在用的那一个。
                c.seg((x + w - 38.0, cy + 1.0), (x + w - 34.0, cy + 5.0), 1.8, theme::WHITE);
                c.seg((x + w - 34.0, cy + 5.0), (x + w - 26.0, cy - 5.0), 1.8, theme::WHITE);
            }
            if r.clicked() && !on {
                let target = if input {
                    crate::audio_switch::Kind::Input
                } else {
                    crate::audio_switch::Kind::Output
                };
                if crate::audio_switch::set_default(target, id) {
                    self.live.refresh = true;
                    let short = c.fit(name, size::BODY, W::Regular, 240.0);
                    self.say(format!("已切换到 {}", short));
                    self.live.audio_pick = None;
                } else {
                    self.say("切换失败，请稍后重试");
                }
            }
        }

        // 设备太多时菜单只列前 4 个，剩下的在这里交代一句，免得以为漏了。
        if devices.len() > 4 {
            let more = format!("还有 {} 个设备", devices.len() - 4);
            c.text(x + w - 16.0, y + h - 8.0, &more, size::TINY, W::Regular, theme::DIM, H::Right);
        }

        // 点空白处关掉；此时也顺手清掉长按标记，下一次单击恢复成开关。
        if mask.clicked() {
            self.live.audio_pick = None;
            self.live.hold_used = None;
        }
    }
}