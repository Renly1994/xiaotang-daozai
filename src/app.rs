//! 灵动岛主体：状态、悬停形变、切页、顶栏与公共层。与平台无关——
//! Windows 外壳（`main.rs`）和无窗口录制器（`daozai-record`）共用同一份代码。
//!
//! 时间一律来自 `Env::t`（秒），不直接读系统时钟，所以录屏可逐帧复现。

use crate::extras::{Alert, HealthReminder, PetState};
use crate::anim::{self, lerp, smoothstep, Spring};
use crate::bar_layout::{self as bar, Layout};
use crate::calendar;
use crate::glass;
use crate::lyrics;
use crate::mail;
use crate::model;
use crate::paint::{self, Card, H};
use crate::resident_date::ResidentDay;
use crate::shape;
use crate::smtc::{NowPlaying, Transport};
use crate::theme::{self, size, W};
use eframe::egui::{self, Color32, Pos2, Rect, Vec2};
use std::collections::HashMap;
use std::path::PathBuf;

// ---------- 几何（逻辑点；原版 125% 截图 ÷ 1.25） ----------
pub const PW: f32 = 419.0; // 展开面板宽
pub const PH: f32 = 140.0; // 展开面板高
const PAGE_SHORTCUT_ROW_H: f32 = 24.0;
pub const MUSIC_PAGE: usize = 0;
pub const FOCUS_PAGE: usize = 1;
pub const WATER_PAGE: usize = 2;
/// 久坐提醒的标题（通知按钮据此区分喝水提醒）。
pub const SIT_TITLE: &str = "该起来活动了";

pub const SHELF_PAGE: usize = 3;
pub const TODO_PAGE: usize = 4;
pub const MAIL_PAGE: usize = 5;
pub const QUOTA_PAGE: usize = 6;
pub const CALENDAR_PAGE: usize = 7;
pub const WEATHER_PAGE: usize = 8;
pub const PERFORMANCE_PAGE: usize = 9;
pub const BLUETOOTH_PAGE: usize = 10;
pub(crate) const WEATHER_H: f32 = 232.0;
pub(crate) const CALENDAR_H: f32 = 216.0;
pub const BAR_W: f32 = 163.0; // 收起药丸宽
pub const BAR_H: f32 = 30.0; // 收起药丸高
pub(crate) const HUD_W: f32 = 260.0;
pub(crate) const LYRIC_BAR_W: f32 = 300.0;
pub(crate) const HUD_TRACK_X: f32 = 48.0;
pub(crate) const HUD_TRACK_W: f32 = 150.0;
pub const BAR_W_MIN: f32 = 44.0; // 极简小条
pub const BAR_H_MIN: f32 = 4.5;
pub const FLARE: f32 = 17.6; // 顶部外翻肩宽
pub const FLARE_H: f32 = 16.0; // 外翻收敛高度
pub const RAD: f32 = 33.0; // 底部圆角
pub const WIN_W: f32 = 640.0; // 窗口（透明画布）
pub const CANVAS_W: f32 = WIN_W + 320.0; // 保留右侧通知及阴影的完整空间，原岛仍按 WIN_W 定位。
pub const WIN_H: f32 = 378.0 + PAGE_SHORTCUT_ROW_H; // 性能页展开 360 高，再留出投影
/// 胶囊皮肤与屏幕顶边的间隙。
pub const PILL_GAP: f32 = 6.0;

/// 胶囊的柔和投影：四周都有，多层叠加近似高斯。
/// 圆角矩形一圈轮廓点（顺时针、首尾闭合），用于沿胶囊外形逐段着色。
fn round_rect_points(r:Rect,radius:f32)->Vec<Pos2>{
    let radius=radius.min(r.width().min(r.height())*0.5).max(0.0);
    let corners=[
        (Pos2::new(r.right()-radius,r.top()+radius),-90.0f32),
        (Pos2::new(r.right()-radius,r.bottom()-radius),0.0),
        (Pos2::new(r.left()+radius,r.bottom()-radius),90.0),
        (Pos2::new(r.left()+radius,r.top()+radius),180.0),
    ];
    let mut points=Vec::with_capacity(4*9+1);
    for (c,a0) in corners {
        for i in 0..=8 {let a=(a0+90.0*i as f32/8.0).to_radians();points.push(c+Vec2::new(a.cos(),a.sin())*radius);}
    }
    points.push(points[0]);
    points
}

fn pill_shadow(painter:&egui::Painter,rect:Rect,radius:f32,strength:f32){
    if strength<=0.01 {return;}
    for (grow,a) in [(10.0f32,0.035f32),(6.5,0.05),(3.5,0.07),(1.5,0.09)] {
        let r=Rect::from_min_max(rect.min-Vec2::new(grow*0.6,grow*0.3),rect.max+Vec2::new(grow*0.6,grow));
        painter.rect_filled(r,radius+grow*0.8,Color32::from_black_alpha((255.0*a*strength) as u8));
    }
}
pub(crate) const PAGE_NAMES: [&str; 11] = ["音乐", "专注", "健康", "置物架", "待办", "邮箱", "AI 额度", "日程", "天气", "系统性能", "无线设备"];
pub(crate) const PAGE_SHORTCUT_KEYS: [&str; PAGE_NAMES.len()] = [
    "music", "focus", "health", "shelf", "tasks", "mail", "quota", "calendar", "weather",
    "performance", "wireless",
];
pub(crate) const PAGE_SETTINGS_TABS: [usize; 11] = [1,2,3,5,4,7,11,8,12,15,crate::extras::SETTINGS_TAB_WIRELESS];

// ---------- 动效参数 ----------
/// 悬停意图缓冲：光标停留这么久才真正展开（期间药丸轻微胀大作为反馈），防误触。
const HOVER_INTENT: f64 = 0.20;
const MINIMAL_HOVER_INTENT: f64 = 0.40;
/// 移开后等待多久开始收起；这段时间内回来不会收。
const LEAVE_DELAY: f64 = 0.12;
const PAGE_EDGE_OVERLAP: f32 = 8.0;
const SWELL: (f32, f32) = (1.075, 1.13);
// 展开：欠阻尼，末段约 5% 过冲后回落；高度略软于宽度，像「垂落」下来。
const OPEN_W: (f32, f32) = (330.0, 0.70);
const OPEN_H: (f32, f32) = (290.0, 0.74);
// 收起：临界阻尼，约 0.18 s 干净收回，无回弹。
const CLOSE: (f32, f32) = (620.0, 1.0);
const SWELL_K: (f32, f32) = (420.0, 0.8);
const PAGE_SLIDE: f32 = 32.0;
const PAGE_TURN_DURATION: f32 = 0.34;

pub struct Options {
    /// 是否联网查歌词（录制/测试时关掉）。
    pub fetch_lyrics: bool,
    /// 吉祥物在多久无操作后出现（秒）。
    pub pet_idle: f64,
    /// 启动期独显偏好快照（D3D11 栈选卡用）。
    #[cfg(feature="d3d11-glass")]
    pub liquid_prefer_dgpu_boot: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self { fetch_lyrics: true, pet_idle: 45.0,
            #[cfg(feature="d3d11-glass")]
            liquid_prefer_dgpu_boot: true }
    }
}

/// 每帧由外壳提供的外部世界。
#[derive(Clone, Default)]
pub struct Env {
    pub t: f64,
    /// `HH:MM:SS`
    pub clock: String,
    /// `YYYY-MM-DD`
    pub today: String,
    /// 顶栏日期，例如 `10月28日 周三`
    pub date: String,
    pub battery: Option<u8>,
    pub charging: bool,
    /// 互联网连接状态；未知时不触发表情提醒。
    pub network: Option<bool>,
    /// 光标位置（与 `frame` 传入的 `win` 同一坐标系）；不在窗口附近时为 None。
    pub cursor: Option<Pos2>,
    /// Windows 虚拟桌面中的鼠标物理坐标，跨屏时独立于岛体窗口。
    pub desktop_cursor: Option<(i32,i32)>,
    pub cancel_position_drag: bool,
    /// 窗口移开后也能在原生桌面上识别松手。
    pub primary_down: Option<bool>,
    pub idle_seconds: Option<f64>,
    /// 当前会话是否处于锁屏（Win + L、自动锁屏、唤醒后的登录界面）。
    pub screen_locked: bool,
    /// 正在生成岛仔演示录屏。
    pub recording: bool,
    /// 因「全屏时隐藏」而隐藏岛体，沿用外壳对设置和所在屏幕的判断。
    pub fullscreen_hidden: bool,
    /// 前台浏览器的顶部是否与岛重叠；岛仔自身窗口或暂不可读时为 None。
    pub browser_at_top: Option<bool>,
    pub media: NowPlaying,
    /// 系统主音量（0..100, 是否静音）；v0.5.4 音量条与系统双向同步。
    pub volume: Option<(u32, bool)>,
    /// 蓝牙设备快照：连接状态、电量来源及本次会话的断开记录。
    pub bt: Vec<crate::bt::Device>,
    /// 蓝牙服务是否已经完成首次扫描。
    pub bt_ready: bool,
    /// Windows 当前识别到的 2.4G USB 接收器。
    pub receivers: Vec<crate::bt::Device>,
    /// 岛所在屏幕（与 `win` 同一坐标系，逻辑点）；用于让通知详情留在屏幕内。
    pub screen: Option<Rect>,
    /// 毛玻璃皮肤：岛体四周采样得到的背景色（sRGB 0..1）。
    pub backdrop: Option<[f32; 3]>,
    /// 毛玻璃皮肤：系统背景模糊当前是否生效。
    pub glass_blur: bool,
    /// 不支持逐像素透明的兼容后端需要按内容轮廓裁剪窗口。
    pub opaque_surface: bool,
    #[cfg(all(windows, feature = "sdk-material"))]
    pub material_active: bool,
    #[cfg(all(windows, feature = "sdk-material"))]
    pub material_appearance: crate::material::Appearance,
}

impl Env {
    pub fn hour(&self) -> usize {
        self.clock.get(0..2).and_then(|s| s.parse().ok()).unwrap_or(12)
    }
    pub fn minute(&self) -> u32 {
        self.clock.get(3..5).and_then(|s| s.parse().ok()).unwrap_or(0)
    }
}

pub use crate::messenger::{Messenger,Unread};

/// 展开后未读胶囊的宽度、离面板的间距和每个图标占的高度。
pub(crate) const UNREAD_W: f32 = 40.0;
pub(crate) const UNREAD_GAP: f32 = 8.0;
pub(crate) const UNREAD_SLOT: f32 = 36.0;
/// 收起时两个来源图标的间距：并排各占一格，不叠放。
fn unread_step(_horizontal: bool) -> f32 { 22.0 }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotificationSound { Silent, System, Health }

/// 交给外壳执行的副作用。
#[derive(Clone, Debug, PartialEq)]
pub enum Cmd {
    Media(Transport),
    Open(PathBuf),
    ViewMessenger(Unread),
    #[cfg(windows)]
    OpenPluginNotice { source: String, id: String, session: String },
    OpenUrl(String),
    /// 需要键盘输入：外壳应让窗口可激活并取得焦点。
    Focus,
    Reveal(PathBuf),
    Copy(String),
    Notify(String, String, NotificationSound),
    RequestNotifications,
    RequestQqNotifications,
    DisconnectNotifications,
    ToggleAutostart,
    AddFiles,
    DragFile(PathBuf),
    QuotaRefresh,
    QuotaConnected,
    QuotaChooseFile,
    /// 把系统主音量设为 0..100（v0.5.4 音量条）。
    SetVolume(u32),
    /// 切换系统静音。
    ToggleMute,
    /// 音量命令携带点击时的播放器身份，避免切换歌曲来源后误调其他应用。
    MusicVolume(Transport,String),
    /// v0.5.5：账号清单变了，让外壳重新同步收信线程。
    MailSync,
    /// 测试某个邮箱的连接（授权码只在内存里传给工作线程）。
    MailTest(mail::Account, String),
    MailManage(crate::mail_manage::Request),
    /// v0.5.6：用临时密码试连日历，成功后才允许落盘。
    CalendarTest(calendar::Config, String),
    /// 让外壳立即同步一次日历。
    CalendarSync,
    CalendarMonth(chrono::NaiveDate),
    /// 断开并删除日历配置（凭据由界面删除）。
    CalendarForget,
    /// 选择 .ics 文件导入本机日程。
    CalendarImport,
    /// 重启岛仔（切换渲染方式后生效）。
    RestartApp,
}

/// 毛玻璃皮肤下原生窗口区域的组成部分（相对 `win` 左上角，逻辑点）。
/// 系统模糊只出现在窗口区域内，所以区域要贴合岛体轮廓，而不是外接矩形。
#[derive(Clone, Debug, PartialEq)]
pub enum GlassShape {
    /// 任意闭合轮廓。
    Outline(Vec<Pos2>),
    /// 岛体 / 通知胶囊：顶部外翻肩部 + 竖直侧边 + 底部圆角（参数同 `shape::fill`）。
    Notch { rect: Rect, flare: f32, flare_h: f32, radius: f32 },
    /// 停靠在左右边缘的岛体：肩部沿屏幕边缘展开（参数同 `shape::fill_docked`）。
    Docked { rect: Rect, dock: model::Dock, flare: f32, flare_h: f32, radius: f32 },
    /// 圆角矩形（通知详情、提示、浮动确认）。
    Round(Rect, f32),
}

impl GlassShape {
    /// 闭合轮廓（逻辑点）；窗口区域按它裁剪。
    pub fn outline(&self) -> Vec<Pos2> {
        match self {
            Self::Outline(points) => points.clone(),
            Self::Notch { rect, flare, flare_h, radius } => shape::outline(*rect, *flare, *flare_h, *radius),
            Self::Docked { rect, dock, flare, flare_h, radius } => shape::outline_docked(*rect, *dock, *flare, *flare_h, *radius),
            Self::Round(r, radius) => shape::outline(*r, 0.0, 0.5, *radius),
        }
    }
}

/// 一块系统模糊：物理像素的窗口坐标与圆角半径（0 = 直角）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BlurPiece {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub radius: f32,
}

/// 把毛玻璃轮廓拆成合成引擎能直接表达的圆角矩形块：
/// 主体向上多延伸一个圆角半径，让顶部两角落在窗口外，只留下底部圆角；
/// 外翻的肩部按物理像素逐行拼出（相邻同宽的行合并），边缘误差不超过半个像素。
pub fn blur_pieces(shapes: &[GlassShape], ppp: f32) -> Vec<BlurPiece> {
    let mut out = Vec::new();
    for shape in shapes {
        match shape {
            GlassShape::Docked { rect, dock, flare, flare_h, radius } => {
                if rect.width() <= 0.0 || rect.height() <= 0.0 { continue; }
                // 在贴顶坐标系里按刘海拆块，再把每块映射到当前停靠边。
                let (map, frame) = shape::EdgeMap::of(*rect, *dock);
                let mut pieces = blur_pieces(&[GlassShape::Notch { rect: frame, flare: *flare, flare_h: *flare_h, radius: *radius }], ppp);
                let anchor=map.apply(Pos2::ZERO);
                let edge = (anchor.x * ppp).round();
                for p in &mut pieces {
                    let (x, y, w, h) = (p.x, p.y, p.w, p.h);
                    if *dock==model::Dock::Bottom {
                        p.y=(anchor.y*ppp).round()-y-h;
                        continue;
                    }
                    *p = BlurPiece { x: if *dock == model::Dock::Left { edge + y } else { edge - y - h }, y: x, w: h, h: w, radius: p.radius };
                }
                out.extend(pieces);
            }
            GlassShape::Notch { rect, flare, flare_h, radius } => {
                if rect.width() <= 0.0 || rect.height() <= 0.0 { continue; }
                let r = radius.min(rect.height() * 0.5).min(rect.width() * 0.5).max(0.0) * ppp;
                let fh = flare_h.min(rect.height() * 0.5).max(0.5) * ppp;
                let (x0, x1, y0, y1) = ((rect.left() * ppp).round(), (rect.right() * ppp).round(), (rect.top() * ppp).round(), (rect.bottom() * ppp).round());
                out.push(BlurPiece { x: x0, y: y0 - r, w: x1 - x0, h: y1 - y0 + r, radius: r });
                let rows = fh.ceil() as i32;
                let mut i = 0;
                while i < rows {
                    let wing = |row: i32| (flare * ppp * (1.0 - ((row as f32 + 0.5) / fh).min(1.0)).powf(2.4)).round();
                    let w = wing(i);
                    if w < 1.0 { break; }
                    let mut h = 1;
                    while i + h < rows && wing(i + h) == w { h += 1; }
                    let y = y0 + i as f32;
                    out.push(BlurPiece { x: x0 - w, y, w, h: h as f32, radius: 0.0 });
                    out.push(BlurPiece { x: x1, y, w, h: h as f32, radius: 0.0 });
                    i += h;
                }
            }
            GlassShape::Round(r, radius) => {
                let (x,y,right,bottom)=((r.left()*ppp).round(),(r.top()*ppp).round(),(r.right()*ppp).round(),(r.bottom()*ppp).round());
                let (w,h)=(right-x,bottom-y);
                if w<=0.0 || h<=0.0 {continue;}
                // 收起动画先按物理边界取整，再限制圆角，避免上下圆弧在中线交叠。
                out.push(BlurPiece { x,y,w,h,radius:(radius*ppp).max(0.0).min(w.min(h)*0.5) });
            }
            GlassShape::Outline(points) => {
                // 任意轮廓只能退化为外接矩形；岛体本身走 Notch，不会用到这里。
                let b = Rect::from_points(points);
                if b.is_positive() {
                    out.push(BlurPiece { x: (b.left() * ppp).round(), y: (b.top() * ppp).round(), w: (b.width() * ppp).round(), h: (b.height() * ppp).round(), radius: 0.0 });
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod blur_piece_tests {
    use super::*;

    fn covered(pieces: &[BlurPiece], x: f32, y: f32) -> bool {
        pieces.iter().any(|p| {
            if !(x >= p.x && x < p.x + p.w && y >= p.y && y < p.y + p.h) { return false; }
            // 圆角：只判断四角的圆弧外侧
            let r = p.radius;
            let cx = x.clamp(p.x + r, p.x + p.w - r);
            let cy = y.clamp(p.y + r, p.y + p.h - r);
            (x - cx).powi(2) + (y - cy).powi(2) <= r * r + 1e-3
        })
    }

    #[test]
    fn blur_follows_the_island_outline_not_its_bounding_box() {
        for ppp in [1.0, 1.25, 1.5, 2.0] {
            let rect = Rect::from_min_size(Pos2::new(110.0, 0.0), Vec2::new(419.0, 140.0));
            let shape = GlassShape::Notch { rect, flare: 17.6, flare_h: 16.0, radius: 33.0 };
            let pieces = blur_pieces(std::slice::from_ref(&shape), ppp);
            let outline = shape.outline();
            // 轮廓内部的点都被模糊覆盖，外部（含底部圆角外、肩部外）都不覆盖；边缘留 1.5 像素容差。
            let inside = |x: f32, y: f32| {
                let mut c = false;
                let mut prev = *outline.last().unwrap();
                for &next in &outline {
                    if (next.y > y) != (prev.y > y) && x < (prev.x - next.x) * (y - next.y) / (prev.y - next.y) + next.x { c = !c; }
                    prev = next;
                }
                c
            };
            let mut checked = 0;
            for yi in 0..(150.0 * ppp) as i32 {
                for xi in ((80.0 * ppp) as i32..(560.0 * ppp) as i32).step_by(3) {
                    let (x, y) = (xi as f32 + 0.5, yi as f32 + 0.5);
                    let (lx, ly) = (x / ppp, y / ppp);
                    let deep_in = inside(lx, ly) && [(-1.5, 0.0), (1.5, 0.0), (0.0, 1.5), (0.0, -1.5)].iter().all(|(dx, dy)| inside(lx + dx / ppp, (ly + dy / ppp).max(0.01)));
                    let far_out = !inside(lx, ly) && [(-1.5, 0.0), (1.5, 0.0), (0.0, 1.5), (0.0, -1.5)].iter().all(|(dx, dy)| !inside(lx + dx / ppp, ly + dy / ppp));
                    if deep_in { assert!(covered(&pieces, x, y), "{ppp} 缩放下岛体内 ({x},{y}) 没有模糊"); checked += 1; }
                    if far_out && y >= 0.0 { assert!(!covered(&pieces, x, y), "{ppp} 缩放下岛体外 ({x},{y}) 不能出现模糊"); }
                }
            }
            assert!(checked > 1000);
            assert!(pieces.len() < 60, "肩部合并后块数应很少：{}", pieces.len());
        }
    }

    #[test]
    fn docked_blur_follows_the_mapped_outline() {
        for dock in [model::Dock::Left, model::Dock::Right, model::Dock::Bottom] {
            for ppp in [1.0, 1.5] {
                let x = if dock == model::Dock::Left { 0.0 } else { 81.0 };
                let shape = GlassShape::Docked { rect: Rect::from_min_size(Pos2::new(x, 200.0), Vec2::new(419.0, 140.0)), dock, flare: 17.6, flare_h: 16.0, radius: 33.0 };
                let pieces = blur_pieces(std::slice::from_ref(&shape), ppp);
                let outline = shape.outline();
                let inside = |x: f32, y: f32| {
                    let mut c = false;
                    let mut prev = *outline.last().unwrap();
                    for &next in &outline {
                        if (next.y > y) != (prev.y > y) && x < (prev.x - next.x) * (y - next.y) / (prev.y - next.y) + next.x { c = !c; }
                        prev = next;
                    }
                    c
                };
                let mut checked = 0;
                for yi in ((170.0 * ppp) as i32..(370.0 * ppp) as i32).step_by(2) {
                    for xi in (0..(500.0 * ppp) as i32).step_by(3) {
                        let (px, py) = (xi as f32 + 0.5, yi as f32 + 0.5);
                        let (lx, ly) = (px / ppp, py / ppp);
                        let near = [(-1.5, 0.0), (1.5, 0.0), (0.0, 1.5), (0.0, -1.5)];
                        let deep_in = inside(lx, ly) && near.iter().all(|(dx, dy)| inside((lx + dx / ppp).clamp(0.01, 499.99), ly + dy / ppp));
                        let far_out = !inside(lx, ly) && near.iter().all(|(dx, dy)| !inside(lx + dx / ppp, ly + dy / ppp));
                        if deep_in { assert!(covered(&pieces, px, py), "{dock:?} {ppp}: 岛体内 ({px},{py}) 没有模糊"); checked += 1; }
                        let on_canvas = lx > 0.5 && lx < 499.5 && (dock!=model::Dock::Bottom || ly<340.0);
                        if far_out && on_canvas { assert!(!covered(&pieces, px, py), "{dock:?} {ppp}: 岛体外 ({px},{py}) 不能有模糊"); }
                    }
                }
                assert!(checked > 1000);
            }
        }
    }

    #[test]
    fn collapsing_pill_blur_radius_fits_the_physical_bounds() {
        for ppp in [1.0,1.25,1.5,2.0] {
            for height in [30.02,30.3,30.49,30.5,30.8,31.0,31.2,32.1,35.3] {
                let rect=Rect::from_min_size(Pos2::new(230.23,6.0),Vec2::new(179.31,height));
                let radius=15.0+(height-30.0)*0.1;
                let pieces=blur_pieces(&[GlassShape::Round(rect,radius)],ppp);
                assert_eq!(pieces.len(),1);
                let p=pieces[0];
                assert!(p.radius*2.0<=p.w.min(p.h),"收起时圆弧不能超过物理边界：ppp={ppp}, height={height}, piece={p:?}");
                assert_eq!(p.x+p.w,(rect.right()*ppp).round(),"模糊与岛体右边界应使用同一像素位置");
                assert_eq!(p.y+p.h,(rect.bottom()*ppp).round(),"模糊与岛体下边界应使用同一像素位置");
            }
        }
    }

    #[test]
    fn rounded_rectangles_and_empty_shapes() {
        let pieces = blur_pieces(&[GlassShape::Round(Rect::from_min_size(Pos2::new(10.0, 20.0), Vec2::new(100.0, 40.0)), 10.0)], 1.5);
        assert_eq!(pieces, vec![BlurPiece { x: 15.0, y: 30.0, w: 150.0, h: 60.0, radius: 15.0 }]);
        assert!(blur_pieces(&[GlassShape::Notch { rect: Rect::NOTHING, flare: 5.0, flare_h: 8.0, radius: 12.0 }], 1.0).is_empty());
        // 极简细条：没有肩部，只有一块
        let tiny = blur_pieces(&[GlassShape::Notch { rect: Rect::from_min_size(Pos2::new(298.0, 0.0), Vec2::new(44.0, 4.5)), flare: 0.0, flare_h: 8.0, radius: 2.25 }], 1.25);
        assert_eq!(tiny.len(), 1);
    }
}

pub struct FrameOut {
    /// 原生窗口应保留的可见区域（相对 `win` 左上角）。
    pub region: Rect,
    /// 毛玻璃 + 系统模糊时，替代 `region` 的贴合轮廓；其余情况为 None。
    pub glass: Option<Vec<GlassShape>>,
    /// 是否在 glass 轮廓下方显示系统模糊；兼容渲染只借轮廓裁剪时为 false。
    pub blur: bool,
    #[cfg(all(windows, feature = "sdk-material"))]
    pub(crate) material_layer: Option<crate::material::Layer>,
    /// 常驻条实际占用的窗口轮廓，透明留白交给下层窗口。
    pub resident_clip: Option<Vec<GlassShape>>,
    /// 鼠标命中区（相对 `win` 左上角），区域外点击穿透到下层窗口。
    pub hit: Rect,
    /// 切到较矮页面时，原页边缘临时保留的续滚范围，不参与绘制或点击命中。
    pub page_scroll_area: Option<Rect>,
    /// 极简细条只感应悬停，鼠标点击交给下面的应用。
    pub click_through: bool,
    pub notice_hit: Option<Rect>,
    /// 当前可点击的通知范围（相对 `win` 左上角），供顶部任务栏遮挡判断。
    pub top_bar_notice: Option<Rect>,
    /// 文件拖入感应区；独立于普通鼠标命中区。
    pub file_drop_zone: Option<Rect>,
    /// 展开状态立即生效，不等待展开动画结束。
    pub expanded: bool,
    pub editing: bool,
    pub animating: bool,
    /// 岛或浮层的外接范围还在变化，保持本轮动画的原生外框。
    pub geometry_transitioning: bool,
    /// 当前帧正在绘制翻页过渡。
    pub page_transition: bool,
    /// 喝水页的百分比环和柱状图正在切换。
    pub water_view_transition: bool,
    /// 展开、收起、翻页或内容滚动仍有画面需要逐帧交付。
    pub interactive_transition: bool,
    pub repaint_after: std::time::Duration,
    pub cmds: Vec<Cmd>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Phase {
    Focus,
    Rest,
}

impl Phase {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Phase::Focus => "专注时间",
            Phase::Rest => "休息",
        }
    }
}

pub(crate) struct Pomo {
    pub phase: Phase,
    pub remaining: f32,
    pub running: bool,
    #[cfg(feature="sdk-js")]
    configured_minutes: u32,
}

impl Pomo {
    pub(crate) fn reset_remaining(&mut self, seconds: f32) {
        self.remaining=seconds;
        #[cfg(feature="sdk-js")]
        {self.configured_minutes=(seconds/60.0).round() as u32;}
    }
}

/// 本地歌词（酷狗、汽水）每 5 秒复查，最多约 1 分钟；网络失败每 30 秒重试，最多约 5 分钟。手动点重试会重新计数。
const NATIVE_RETRIES:u32=12;
const NETWORK_RETRIES:u32=10;

/// 与 `format!("{title}|{artist}|{duration:.0}")` 等价，但不分配内存：界面每帧要比较好几次。
fn lyric_key_matches(key:&str,media:&NowPlaying)->bool {
    use std::io::Write;
    let Some(rest)=key.strip_prefix(media.title.as_str()).and_then(|rest|rest.strip_prefix('|')) else{return false;};
    let Some(rest)=rest.strip_prefix(media.artist.as_str()).and_then(|rest|rest.strip_prefix('|')) else{return false;};
    let mut buffer=[0u8;32];
    let mut cursor=std::io::Cursor::new(&mut buffer[..]);
    if write!(cursor,"{:.0}",media.duration).is_err(){return rest==format!("{:.0}",media.duration);}
    let written=cursor.position() as usize;
    rest.as_bytes()==&buffer[..written]
}

#[test]
fn lyric_key_matches_the_formatted_key_without_allocating(){
    for (title,artist,duration) in [("歌","手",120.4),("A|B","C",0.0),("歌","",239.5),("歌","手",240.5),("","",f64::NAN),("t","a",1e30)] {
        let media=NowPlaying{title:title.into(),artist:artist.into(),duration,..Default::default()};
        let key=format!("{}|{}|{:.0}",title,artist,duration);
        assert!(lyric_key_matches(&key,&media),"{key}");
        assert!(!lyric_key_matches(&format!("{key}0"),&media));
        assert!(!lyric_key_matches(&format!("x{key}"),&media));
    }
}

/// 发起歌词查询时的歌曲信息，用来判断之后的变化是换歌还是信息补全。
#[derive(Clone,Debug,Default)]
pub(crate) struct LyricSong {pub source:String,pub title:String,pub artist:String,pub duration:f64,pub song_id:Option<u64>}
impl LyricSong {
    fn of(media:&NowPlaying)->Self {
        Self{source:media.source.clone(),title:media.title.clone(),artist:media.artist.clone(),duration:media.duration,song_id:media.song_id}
    }
}

#[derive(Clone,Copy,Debug,PartialEq,Eq)]
enum SongChange {
    /// 换了一首歌：立即清空旧歌词并重新查询。
    Different,
    /// 同一首，只是时长、写法等补全：已有同步歌词时不再查询。
    Same,
    /// 同一首，署名有补全：重新查询，但查询期间继续显示已有歌词。
    Refined,
}

fn song_change(previous:&LyricSong,media:&NowPlaying)->SongChange {
    if !previous.source.eq_ignore_ascii_case(&media.source) {return SongChange::Different;}
    match (previous.song_id,media.song_id) {
        (Some(old),Some(new))=>return if old==new {SongChange::Same}else{SongChange::Different},
        _=>{}
    }
    // 同名的不同版本（现场版、伴奏等）时长通常不同：两边都已知且相差超过 3 秒视为不同歌曲。
    if previous.duration>0.0 && media.duration>0.0 && (previous.duration-media.duration).abs()>3.0 {return SongChange::Different;}
    match lyrics::song_relation(&previous.title,&previous.artist,&media.title,&media.artist) {
        Some(true)=>SongChange::Same,
        Some(false)=>SongChange::Refined,
        None=>SongChange::Different,
    }
}

/// 等待旧请求结束后再发起的在线查询（同一时间只保留一个网络线程）。
pub(crate) struct LyricsJob {
    source:String,title:String,artist:String,duration:f64,song_id:Option<u64>,
    cached:Option<lyrics::Lyrics>,native_only:bool,manual:bool,
}

#[derive(Default)]
pub(crate) struct LyricsState {
    pub key: String,
    /// 本次查询对应的歌曲；为空表示还没有按歌曲发起过查询。
    pub song: Option<LyricSong>,
    /// 旧请求未结束时排队的在线查询；本机歌词轮询不等它，换歌后立即开始。
    pub pending: Option<LyricsJob>,
    pub island_animation_started: bool,
    pub scroll_animation_started: bool,
    pub loading: bool,
    pub error: Option<lyrics::FetchError>,
    pub retry_at: Option<f64>,
    /// 发起本次查询时已知的歌曲 ID；之后才拿到 ID 且还没有同步歌词时，按 ID 再查一次。
    pub song_id: Option<u64>,
    /// 同一首歌已自动重试的次数；达到上限后停止，避免整首歌期间反复读盘或联网。
    pub attempts: u32,
    /// 用户点了「重试」：下一次查询跳过近期「没有歌词」的记录。
    pub manual_retry: bool,
    pub lyrics: Option<lyrics::Lyrics>,
    pub rx: Option<std::sync::mpsc::Receiver<Result<(lyrics::Lyrics,bool), lyrics::FetchError>>>,
    pub native_rx: Option<std::sync::mpsc::Receiver<Option<lyrics::Lyrics>>>,
}

/// v0.5.5：邮件面板与设置表单的界面状态。正文只在内存里过一次，不落盘。
#[derive(Default)]
pub(crate) struct MailState {
    pub privacy:crate::mail_privacy::Privacy,
    pub manage:crate::mail_manage_ui::State,
    pub accounts: Vec<mail::Account>,
    /// 每个账号的连接状态，键是邮箱小写。
    pub status: HashMap<String, mail::Status>,
    /// 最近邮件，新的在前，最多 60 封。
    pub recent: Vec<mail::Item>,
    /// 等当前帧确认避让状态后再决定通知样式。
    code_alerts: Vec<(String, u32, String)>,
    /// 设置页里「测试连接」的结果。
    pub note: String,
    pub testing: Option<String>,
    pub form: MailForm,
    /// 面板里点开的邮件（账号 + uid，避免两个账号 uid 撞车）。
    pub open_uid: Option<(String, u32)>,
    pub reading: Option<mail::Item>,
    pub reader_focus: bool,
    pub bell_at: Option<f64>,
    /// 已经提醒过的会议，键是 `UID + DTSTART`（邮件和日历两条来源共用）。
    pub fired: std::collections::HashSet<String>,
}

#[derive(Default)]
pub(crate) struct MailForm {
    pub provider: mail::Provider,
    pub custom: mail::CustomImap,
    pub use_windows_proxy: bool,
    pub email: String,
    pub password: String,
    pub show: bool,
    /// 设置页的「添加邮箱」弹窗是否打开。
    pub open: bool,
}

impl MailState {
    /// 最近一封带验证码的邮件。
    pub fn code_item(&self) -> Option<&mail::Item> {self.recent.iter().find(|m|m.code.is_some())}
    pub(crate) fn alert_code_item(&self, account: &str, uid: u32, now: f64, hide_seconds: u32) -> Option<&mail::Item> {
        self.recent.iter().find(|item| {
            item.uid == uid && item.account.eq_ignore_ascii_case(account) && item.code.is_some()
                && !self.privacy.hidden(item, now, hide_seconds)
        })
    }
    pub fn status_of(&self, email: &str) -> mail::Status {
        self.status
            .get(&email.to_ascii_lowercase())
            .cloned()
            .unwrap_or(mail::Status::Idle)
    }
}

/// v0.5.6：日程（CalDAV）的界面状态。密码只在表单里暂存，保存后立刻清掉。
#[derive(Default)]
pub(crate) struct CalendarState {
    pub local: Vec<crate::local_calendar::Entry>,
    pub local_error: String,
    pub draft: Option<crate::local_calendar::Draft>,
    pub feeds: Vec<CalendarFeed>,
    pub config: Option<calendar::Config>,
    /// 未来 24 小时的日程（提醒也读这里）。
    pub events: Vec<mail::Meeting>,
    pub view: crate::calendar_ui::State,
    pub agenda: bool,
    pub agenda_first: usize,
    pub connected: bool,
    pub last_sync: String,
    /// 连续失败次数；>= 2 才在设置页显示红字。
    pub failures: u32,
    pub error: String,
    pub testing: bool,
    /// 设置表单里的提示（成功/失败/粘贴拆分的说明）。
    pub note: String,
    /// `note` 是不是错误提示（界面显示成红字）。
    pub note_error: bool,
    pub form: CalendarForm,
    /// 「测试并保存」用的临时密码，测试成功才落盘。
    pub test_password: String,
    /// 导入 .ics 文件的结果说明。
    pub import_note: String,
    pub warning:String,
}

pub(crate) struct CalendarFeed {
    pub config: calendar::Config,
    pub events: Vec<mail::Meeting>,
    pub month: crate::calendar_ui::State,
    pub connected: bool,
    pub last_sync: String,
    pub failures: u32,
    pub error: String,
    pub warning:String,
}

impl CalendarFeed {
    fn new(config: calendar::Config) -> Self {
        Self { config, events: vec![], month: Default::default(), connected: false,
            last_sync: String::new(), failures: 0, error: String::new(), warning:String::new() }
    }
}

#[derive(Default)]
pub(crate) struct CalendarForm {
    pub provider: calendar::Provider,
    /// 自定义 CalDAV 的服务器地址。
    pub server: String,
    pub username: String,
    pub password: String,
    pub show: bool,
    pub guide_open: bool,
    /// 设置页的「添加日历」弹窗是否打开。
    pub open: bool,
}

impl CalendarState {
    pub fn load() -> Self {
        let config = calendar::load_config();
        let form = CalendarForm { provider: calendar::selected_provider(),
            username: config.as_ref().map(|cfg| cfg.username.clone()).unwrap_or_default(),
            server: config.as_ref().filter(|cfg| cfg.provider.custom_server()).and_then(|cfg| cfg.server.clone()).unwrap_or_default(), ..Default::default() };
        let feeds = calendar::load_configs().into_iter().map(CalendarFeed::new).collect();
        let (local,local_error)=match crate::local_calendar::load(){Ok(events)=>(events,String::new()),Err(error)=>(vec![],error)};
        Self { config, form, feeds, local, local_error, ..Default::default() }
    }

}

struct PositionDrag {
    original_display:String,
    original_monitor:Option<String>,
    start: Vec2,
    /// 本段拖动的起始偏移（未取整，换边后重置），单位与 `island_offset` 相同。
    offset: f32,
    original: (model::Dock, i32),
}

// ---------- 拖动位置动效 ----------
/// 原地按住这么久即「拿起」岛体，不必先移动鼠标。
const LIFT_HOLD: f64 = 0.12;
/// 拿起 / 放下：略欠阻尼，提起时轻轻弹一下，放下时回落。
const LIFT_K: (f32, f32) = (420.0, 0.38);
/// 拿起后放大：沿停靠边方向 / 离开停靠边方向。
const LIFT_SCALE: (f32, f32) = (0.08, 0.14);
/// 窗口跟手：明显拖尾并略微冲过光标；松手后落位回弹一两下。
const GLIDE_FOLLOW: (f32, f32) = (320.0, 0.72);
const GLIDE_SETTLE: (f32, f32) = (380.0, 0.38);
/// 移动越快沿运动方向拉得越长（最多 22%），厚度方向相应收窄。
const STRETCH_MAX: f32 = 0.22;
const STRETCH_SPEED: f32 = 1400.0;
/// 吸附范围（`island_offset` 单位）：居中和拖动前的位置。
const DRAG_MAGNET: f32 = 5.0;
/// 吸住居中后要拖出更远才松开，停在中间时稳稳「咬住」，不会被手抖带走。
const CENTER_RELEASE: f32 = 9.0;
/// 离居中多近开始浮现轮廓描边（`island_offset` 单位）。
const CENTER_GUIDE_RANGE: f32 = 24.0;
/// 吸附瞬间沿轮廓从中心向两端扫开的光，持续时长（秒）。
const CENTER_SWEEP: f64 = 0.55;
/// 已居中的提示色（绿）：岛体描边、外发光和扫光统一用它，颜色本身就是「对准了」。
const CENTER_TINT: (f32, f32, f32) = (88.0, 222.0, 140.0);

pub struct App {
    #[cfg(feature="sdk-js")]
    pub(crate) plugins: crate::sdk_spike::local::LocalPlugins,
    #[cfg(feature="sdk-js")]
    plugin_form_focused: bool,
    #[cfg(feature="sdk-js")]
    pub(crate) pinned_displays: Vec<crate::sdk_spike::local::PinnedDisplay>,
    #[cfg(feature="sdk-js")]
    pinned_horizontal_width: f32,
    pub opts: Options,
    pub settings: model::Settings,
    /// 由主程序应用原生截图保护后再保存。
    pub capture_exclusion_request: Option<bool>,
    /// 截图和录屏隐藏未能生效时，在设置中显示原因。
    pub capture_exclusion_error: Option<String>,
    #[cfg(all(windows, feature = "sdk-material"))]
    pub material_capture_forced: bool,
    #[cfg(all(windows, feature = "sdk-material"))]
    pub material_active_id: Option<String>,
    #[cfg(all(windows, feature = "sdk-material"))]
    material_appearance: crate::material::Appearance,
    /// 本次运行实际使用兼容渲染（Direct3D 12）；设置里的开关与它不同时提示重启。
    pub render_compat_running: bool,
    pub(crate) todos: model::TodoList,
    pub(crate) water: model::Water,
    pub(crate) shelf: model::Shelf,

    // 形变
    pub(crate) w: Spring,
    pub(crate) h: Spring,
    /// 完全展开时的版面高度。切页时与外壳同步变化，开合进度不会因目标页变高而倒退。
    pub(crate) page_height: Spring,
    pub(crate) open: bool,
    hold_open: bool,
    /// 顶栏为图钉让位的动画进度。
    pub(crate) hold_slot: f32,
    hover_since: Option<f64>,
    leave_since: Option<f64>,
    /// 前台切换导致的强制收起：光标离开前不再自动展开。
    pub hover_lock: bool,
    pub(crate) reveal: f32,
    position_drag: Option<PositionDrag>,
    /// 拖动中和松手落位期间的窗口位置（未取整），落定后回到 `settings.island_offset`。
    drag_glide: Option<anim::Spring>,
    /// 拿起程度 0..1（可略过冲）与速度拉伸量，只影响岛体外形，不影响展开进度。
    drag_lift: anim::Spring,
    seam_guide:f32,
    crossed_at:f64,
    drag_stretch: anim::Spring,
    /// 拖动时是否吸在居中；进入吸附的时刻用于触发扫光和「咔嗒」。
    pub(crate) center_locked: bool,
    center_lock_at: f64,
    /// 居中描边可见度 0..1（越靠近居中越清晰）与「已居中」点亮程度 0..1。
    pub(crate) center_guide: f32,
    pub(crate) center_lock_a: f32,
    /// 光标停在日期时间拖动区的起始时间：停留越久把手提示越明显，划过不打扰。
    grip_hover_since: Option<f64>,

    // 切页
    pub(crate) page: usize,
    page_from: usize,
    #[cfg(feature="sdk-js")]
    status_pages: Vec<(String, String)>,
    #[cfg(feature="sdk-js")]
    plugin_canvas_frame: Option<(usize, crate::plugin_status_ui::InteractionResponse)>,
    page_from_offset: f32,
    page_from_elapsed: f32,
    page_scroll_area: Option<Rect>,
    page_dir: f32,
    page_t0: f64,
    dots: [Spring; 11],
    wheel_delta: f32,
    pub(crate) content_scroll: Option<(egui::Id, f32)>,
    wheel_at: f64,
    wheel_page_at: f64,

    // 拖入
    pub(crate) drag_hot: bool,
    drag_departing: bool,
    pub(crate) drag_a: f32,
    drag_saved: Option<usize>,

    // 置物架
    pub(crate) shelf_scroll: Spring,
    pub(crate) shelf_first: usize,
    pub(crate) shelf_menu_rect: Option<Rect>,
    pub(crate) shelf_x: HashMap<PathBuf, Spring>,
    pub(crate) shelf_born: HashMap<PathBuf, f64>,
    pub(crate) icons: crate::icons::Cache,
    pub(crate) avoidance: crate::avoidance::Picker,
    pub(crate) confirm_clear: Option<f64>,
    /// 置物架页的两个分栏：0 = 文件，1 = 剪贴板。
    pub(crate) shelf_tab: usize,
    pub clipboard: crate::clipboard::Shared,
    pub(crate) clip_first: usize,
    pub(crate) clip_confirm_clear: Option<f64>,

    // 饮水
    pub(crate) water_ring: Spring,
    pub(crate) water_ml: f32,
    pub(crate) water_view: usize,
    pub(crate) water_view_t: Spring,
    pub(crate) sit_view_t: Spring,
    /// 饮水页当前是否显示「站立提醒」。
    pub(crate) sit_mode: bool,
    /// 健康提醒设置里是否打开了「提醒时段」二级页（时段 + 午休）。
    pub(crate) health_time: bool,
    pub(crate) remind_t: Spring,
    pub(crate) last_reminder: f64,
    pub(crate) health_pending: [bool;2],
    pub(crate) fullscreen_hidden: bool,
    /// 久坐计时起点，确认提醒或点「起身了」后重置。
    pub(crate) sit_start: f64,
    pub(crate) sit_t: Spring,

    // 专注
    pub(crate) pomo: Pomo,
    pub(crate) settings_editing: bool,
    pub(crate) ring_shown: Spring,
    pub(crate) reset_spin: Spring,

    // 待办
    pub(crate) adding: bool,
    pub(crate) editing: Option<usize>,
    pub(crate) buf: String,
    pub(crate) todo_input_focused: bool,
    pub(crate) todo_pop: Spring,

    // 音乐
    pub(crate) ly: LyricsState,
    pub(crate) ly_worker: Option<std::thread::JoinHandle<()>>,
    pub(crate) show_lyrics: bool,
    island_lyric_layout: Option<std::sync::Arc<egui::Galley>>,
    media_anchor: (String, f64, f64, bool),
    pub(crate) media: NowPlaying,
    pub(crate) music_artwork: Option<(std::sync::Arc<egui::ColorImage>,egui::TextureHandle)>,

    forced_open_until:f64,
    pub settings_open:bool,
    settings_window:std::sync::Arc<std::sync::Mutex<crate::settings_window::State>>,
    pub displays:Vec<crate::display_layout::Display>,
    browser_at_top:bool,
    browser_minimal_active:bool,
    /// 毛玻璃着色量（平滑过渡，避免背景变化时闪烁）；0 表示尚未初始化。
    glass_tint:f32,
    pub(crate) fluid:crate::fluid::Fluid,
    pub(crate) fluid_origin:Pos2,
    fluid_art:Option<egui::TextureId>,
    fluid_title:String,
    screen:Option<Rect>,
    horizontal_width: f32,
    pub(crate) settings_focus_requested:bool,
    pub(crate) settings_tab:usize,
    #[cfg(feature="sdk-js")]
    pub(crate) settings_plugin:Option<String>,
    pub(crate) settings_category:usize,
    pub messenger_statuses:std::collections::HashMap<Messenger,String>,
    pub(crate) notification_apps:model::NotificationApps,
    pub(crate) subpage:Option<usize>,
    pub(crate) task_dialog:Option<crate::extras::TaskDraft>,
    pub listener_status: String,
    pub(crate) notification_bell_until: f64,
    pub(crate) notification_bell_at: f64,
    pub(crate) notice: Option<Alert>,
    pub(crate) hud_notice: Option<Alert>,
    hud_alpha: f32,
    notice_width: Spring,
    notice_details: bool,
    pub pending_messengers:Vec<Unread>,
    #[cfg(windows)]
    pub(crate) plugin_notice_ui: crate::plugin_notice_ui::NoticeUi,
    #[cfg(windows)]
    pub(crate) plugin_notice_host: crate::plugin_notice_settings::NoticeHost,
    messenger_scroll: f32,
    pub(crate) alerts: std::collections::VecDeque<Alert>,
    pub(crate) history: std::collections::VecDeque<Alert>,
    pub(crate) pending: Vec<Cmd>,
    pub(crate) todo_first: usize,
    pub(crate) todo_choosing: bool,
    pub(crate) pet: crate::pet::Pet,
    pub(crate) last_drink: Option<(u32, usize)>,
    // 其它
    toast: Option<(String, f64)>,
    toast_below: bool,
    last_input: f64,
    system_idle: Option<f64>,
    last_cursor: Option<Pos2>,
    pet_a: f32,
    last_t: Option<f64>,
    pub(crate) t: f64,
    pub(crate) dt: f32,
    pub(crate) today: String,
    pub(crate) current_hour: usize,
    pub(crate) cursor_local: Option<Pos2>,
    started: bool,
    /// v0.5.4：音量条展示值（拖动时先用本地值，随后跟上系统回报）。
    pub(crate) volume_ui: Option<(u32, bool)>,
    /// 拖动/滚轮刚改过音量，这段时间内系统回声不再单独弹一条。
    pub(crate) volume_quiet_until: f64,
    /// 音量条填充的缓动量（0..1），让数值变化有「量感」。
    pub(crate) volume_a: f32,
    pub(crate) music_volume_ui: Option<(u32,bool)>,
    pub(crate) music_volume_quiet_until: f64,
    pub(crate) music_volume_open: bool,
    /// 拖动进度条松手后先用本地目标值显示：播放器回报新位置有延迟，等追上、换歌或超时再交还（来自 PR #162）。
    pub(crate) music_seek_ui: Option<(f64,String)>,
    pub(crate) music_seek_quiet_until: f64,
    /// 点了上一首 / 下一首的时刻：先本地把进度归零，等播放器报回新歌位置再交还。
    pub(crate) music_restart_at: Option<f64>,
    /// 上一次看到的蓝牙设备，用来只在连接/断开时提示一次。
    pub(crate) bt_shown: Vec<crate::bt::Device>,
    pub(crate) wireless_devices: Vec<crate::bt::Device>,
    pub(crate) wireless_offset: usize,
    pub(crate) wireless_focus: Option<String>,
    /// 展开电量页的时刻，用于入场动效；离开页面或收起后清空。
    pub(crate) wireless_enter_at: Option<f64>,
    pub(crate) wireless_fx: std::collections::HashMap<String,crate::bluetooth_ui::WirelessFx>,
    pub(crate) wireless_alerted: std::collections::HashMap<(String,&'static str),f64>,
    /// 每台已连接设备第一次出现的时刻；用来区分“正在读取电量”和“设备不提供电量”。
    pub(crate) wireless_seen_at: std::collections::HashMap<String,f64>,
    bt_initialized: bool,
    /// v0.5.5：邮件账号、最近邮件与设置表单。
    pub(crate) mail: MailState,
    /// 岛内详情与独立阅读窗口各保留一份排版缓存。
    mail_lines_cache: Vec<(std::sync::Arc<str>, std::sync::Arc<str>, u32, std::sync::Arc<Vec<crate::mail_format::Line>>)>,
    /// v0.5.6：日历（CalDAV）配置、未来 24 小时事件与设置表单。
    pub(crate) calendar: CalendarState,
    pub(crate) quota: crate::quota_ui::State,
    /// 天气页的当前数据与后台拉取状态。
    pub(crate) weather: crate::weather::State,
    pub(crate) weather_source: crate::weather_source_ui::State,
    pub(crate) updates: crate::updates::State,
    pub(crate) supporters: crate::supporters::State,
    pub(crate) resident: bar::Motion,
    pub(crate) performance: crate::performance::Monitor,
    pub(crate) resident_power: (Option<u8>,bool),
}

impl App {
    pub fn new(ctx: &egui::Context, opts: Options) -> Self {
        theme::load_fonts(ctx);
        egui_extras::install_image_loaders(ctx);
        ctx.set_theme(egui::ThemePreference::Dark);
        ctx.set_visuals(egui::Visuals::dark());
        ctx.style_mut(|s| {
            s.interaction.selectable_labels = false;
            s.visuals.text_cursor.stroke = egui::Stroke::new(1.5_f32, theme::BLUE);
            s.visuals.selection.bg_fill = theme::alpha_blue(0.45);
        });
        let settings = model::Settings::load();
        let clipboard_on = settings.clipboard_history;
        let (bw, bh) = if settings.minimal { (BAR_W_MIN, BAR_H_MIN) } else { (BAR_W, BAR_H) };
        let focus_seconds=settings.focus_minutes as f32*60.0;
        let initial_lyrics=settings.lyrics_enabled;
        let initial_page=settings.page_order.iter().copied().find(|page|settings.visible_pages[*page]).unwrap_or(0);
        let weather_source=crate::weather_source_ui::State::load();
        let mut weather=crate::weather::State::load(&settings.weather_city,settings.weather_position.as_ref());
        if weather_source.displayed==crate::weather_source_ui::Provider::Qweather {weather.snapshot=weather_source.runtime.snapshot();}
        Self {
            #[cfg(feature="sdk-js")]
            plugins: crate::sdk_spike::local::LocalPlugins::open(model::config_dir().join("local-plugins")),
            #[cfg(feature="sdk-js")]
            plugin_form_focused: false,
            #[cfg(feature="sdk-js")]
            pinned_displays: Vec::new(),
            #[cfg(feature="sdk-js")]
            pinned_horizontal_width: WIN_W,
            opts,
            render_compat_running: settings.render_compat,
            settings,
            capture_exclusion_request: None,
            capture_exclusion_error: None,
            #[cfg(all(windows, feature = "sdk-material"))]
            material_capture_forced: false,
            #[cfg(all(windows, feature = "sdk-material"))]
            material_active_id: None,
            #[cfg(all(windows, feature = "sdk-material"))]
            material_appearance: Default::default(),
            todos: model::TodoList::load(),
            water: model::Water::default(),
            shelf: model::Shelf::load(),
            w: Spring::new(bw, CLOSE.0, CLOSE.1),
            h: Spring::new(bh, CLOSE.0, CLOSE.1),
            page_height: Spring::new(PH, OPEN_H.0, OPEN_H.1),
            open: false,
            hover_since: None,
            leave_since: None,
            hover_lock: false,
            hold_open: false,
            hold_slot: 0.0,
            reveal: 0.0,
            position_drag: None,
            drag_glide: None,
            seam_guide:0.0, crossed_at:f64::NEG_INFINITY,
            drag_lift: anim::Spring::new(0.0, LIFT_K.0, LIFT_K.1),
            drag_stretch: anim::Spring::new(0.0, 260.0, 0.6),
            center_locked: false,
            center_lock_at: -10.0,
            center_guide: 0.0,
            center_lock_a: 0.0,
            grip_hover_since: None,
            page: initial_page,
            page_from: initial_page,
            #[cfg(feature="sdk-js")]
            status_pages: Vec::new(),
            #[cfg(feature="sdk-js")]
            plugin_canvas_frame: None,
            page_from_offset: 0.0,
            page_from_elapsed: PAGE_TURN_DURATION,
            page_scroll_area: None,
            page_dir: 1.0,
            page_t0: -10.0,
            dots: std::array::from_fn(|i| Spring::new(if i == initial_page { 16.0 } else { 4.0 }, 380.0, 0.86)),
            wheel_delta: 0.0,
            content_scroll: None,
            wheel_at: -1.0,
            wheel_page_at: -1.0,
            drag_hot: false,
            drag_departing: false,
            drag_a: 0.0,
            drag_saved: None,
            shelf_scroll: Spring::new(0.0, 300.0, 0.9),
            shelf_first: 0,
            shelf_menu_rect: None,
            shelf_x: HashMap::new(),
            shelf_born: HashMap::new(),
            icons: Default::default(), avoidance: Default::default(),
            confirm_clear: None,
            shelf_tab: 0,
            clipboard: crate::clipboard::Shared::new(clipboard_on),
            clip_first: 0,
            clip_confirm_clear: None,
            water_ring: Spring::new(0.0, 120.0, 0.9),
            water_ml: 0.0,
            water_view: 0,
            water_view_t: Spring::new(0.0, 200.0, 0.9),
            sit_view_t: Spring::new(0.0, 200.0, 0.9),
            sit_mode: false,
            health_time: false,
            remind_t: Spring::new(1.0, 400.0, 0.8),
            last_reminder: 0.0,
            health_pending: model::load_health_pending(),
            fullscreen_hidden: false,
            sit_start: 0.0,
            sit_t: Spring::new(1.0, 400.0, 0.8),
            pomo: Pomo {
                phase: Phase::Focus, remaining: focus_seconds, running: false,
                #[cfg(feature="sdk-js")]
                configured_minutes: (focus_seconds/60.0).round() as u32,
            }, settings_editing:false,
            ring_shown: Spring::new(1.0, 160.0, 0.95),
            reset_spin: Spring::new(0.0, 160.0, 0.82),
            adding: false,
            todo_input_focused: false,
            editing: None,
            buf: String::new(),
            todo_pop: Spring::new(0.0, 500.0, 0.45),
            ly: LyricsState::default(),
            ly_worker: None,
            show_lyrics: initial_lyrics,
            island_lyric_layout: None,
            media_anchor: (String::new(), 0.0, 0.0, false),
            media: NowPlaying::default(),
            music_artwork: None,
            #[cfg(feature="sdk-js")]
            settings_plugin:None,
            forced_open_until:0.0, settings_open:false, settings_window:std::sync::Arc::new(std::sync::Mutex::new(crate::settings_window::State::load())), displays:Vec::new(), browser_at_top:false, browser_minimal_active:false, glass_tint:0.0, fluid:Default::default(), fluid_origin:Pos2::ZERO, fluid_art:None, fluid_title:String::new(), screen:None, horizontal_width:WIN_W, settings_focus_requested:false, settings_tab:0, settings_category:0, messenger_statuses:Default::default(), notification_apps:model::NotificationApps::load(), subpage:None, task_dialog:None,
            listener_status: "尚未连接".into(),
            notification_bell_until: 0.0, notification_bell_at: -10.0, notice: None, hud_notice: None, hud_alpha: 0.0, notice_width:Spring::new(0.0,CLOSE.0,CLOSE.1), notice_details:false, pending_messengers:Vec::new(), messenger_scroll:0.0, alerts: Default::default(), history: Default::default(), pending: vec![],
            #[cfg(windows)]
            plugin_notice_ui: Default::default(),
            #[cfg(windows)]
            plugin_notice_host: Default::default(),
            todo_first: 0, todo_choosing: false, pet: Default::default(), last_drink: None,
            toast: None, toast_below: false,
            last_input: 0.0, system_idle: None,
            last_cursor: None,
            pet_a: 0.0,
            last_t: None,
            t: 0.0,
            dt: 0.0,
            today: String::new(), current_hour: 0,
            cursor_local: None,
            started: false,
            volume_ui: None,
            volume_quiet_until: 0.0,
            volume_a: 0.0,
            music_volume_ui: None,
            music_volume_quiet_until: 0.0,
            music_volume_open: false,
            music_seek_ui: None,
            music_seek_quiet_until: 0.0,
            music_restart_at: None,
            bt_shown: Vec::new(), wireless_devices:Vec::new(),wireless_offset:0,wireless_focus:None,wireless_enter_at:None,wireless_fx:Default::default(),wireless_alerted:Default::default(),wireless_seen_at:Default::default(),
            bt_initialized: false,
            mail: MailState { accounts: mail::load_accounts(), ..Default::default() },
            mail_lines_cache: Vec::new(),
            calendar: CalendarState::load(),
            quota: crate::quota_ui::State::load(),
            weather,
            weather_source,
            updates: crate::updates::State::load(model::config_dir().join("update-check.json")),
            supporters: crate::supporters::State::new(model::config_dir().join("supporters")),
            performance: Default::default(), resident: Default::default(), resident_power:(None,false),
        }
    }

    #[cfg(windows)]
    pub fn wireless_compat_store(&self) -> crate::bt::compat::Store {
        #[cfg(feature = "sdk-js")]
        return self.plugins.wireless_store();

        #[cfg(not(feature = "sdk-js"))]
        crate::bt::compat::Store::default()
    }

    #[cfg(all(windows, feature = "sdk-js", feature = "sdk-material"))]
    pub fn material_spec(&self) -> Option<crate::sdk_spike::local::MaterialSpec<'_>> {
        self.plugins.active_material()
    }

    #[cfg(all(windows, feature = "sdk-js", feature = "sdk-material"))]
    pub fn deactivate_material(&mut self) -> Result<(), String> {
        self.plugins.activate_material(None, false).map_err(|error| error.to_string())
    }

    pub fn say(&mut self, msg: impl Into<String>) {
        self.toast = Some((msg.into(), self.t));
        self.toast_below = false;
    }

    pub fn messenger_unavailable(&mut self, message: impl Into<String>) {
        self.say(message);
        self.toast_below = true;
    }

    pub(crate) fn code_copied(&mut self, _code:&str) {
        self.say("验证码已复制");
        self.toast_below = true;
    }

    pub(crate) fn mail_code_hidden(&self,item:&mail::Item)->bool {
        self.mail.privacy.hidden(item,self.t,self.settings.mail_code_hide_seconds)
    }
    pub(crate) fn reveal_mail_code(&mut self,item:&mail::Item){self.mail.privacy.reveal(item,self.t);}
    fn update_mail_code_alerts(&mut self, env: &Env) {
        use crate::extras::AlertSource::MailCode;
        let enabled = self.settings.mail_code_popup && self.settings.mail_code && self.settings.notifications && self.mail_active();
        let quiet = env.fullscreen_hidden || env.screen_locked;
        let avoiding = self.browser_minimal_active && !self.open && !self.hold_open && self.position_drag.is_none();
        let pending = std::mem::take(&mut self.mail.code_alerts);
        let mail = &self.mail;
        let now = self.t;
        let hide_seconds = self.settings.mail_code_hide_seconds;
        let allowed = |a: &Alert| match &a.source {
            MailCode(account, uid) => enabled && !quiet && avoiding && now - a.at < a.duration
                && mail.alert_code_item(account, *uid, now, hide_seconds).is_some(),
            _ => true,
        };
        self.alerts.retain(&allowed);
        if self.notice.as_ref().is_some_and(|a| !allowed(a)) { self.notice = None; }
        if self.hud_notice.as_ref().is_some_and(|a| !allowed(a)) { self.hud_notice = None; }
        if quiet { return; }
        for (account, uid, body) in pending {
            if enabled && avoiding && self.mail.alert_code_item(&account, uid, now, hide_seconds).is_some() {
                self.mail_code_alert(account, uid, body);
            } else {
                self.new_mail_alert(body);
            }
        }
    }
    /// 同一封邮件、同一段正文只解析一次排版，避免每帧重复切分几百个段落。
    pub(crate) fn mail_body_lines(&mut self, item: &mail::Item) -> std::sync::Arc<Vec<crate::mail_format::Line>> {
        // display_mail 每帧只 clone Arc：正文与 HTML 原文的指针都不变就说明是同一封邮件，
        // 长邮件不必每帧重新哈希几十 KB，更不必重新跑正则解析。
        if let Some((_,_,_,lines))=self.mail_lines_cache.iter().find(|(summary, html, uid, _)|
            *uid == item.uid
            && std::sync::Arc::ptr_eq(summary, &item.summary)
            && std::sync::Arc::ptr_eq(html, &item.html)){return lines.clone();}
        let lines=std::sync::Arc::new(crate::mail_format::item_lines(item));
        if self.mail_lines_cache.len()==2{self.mail_lines_cache.remove(0);}
        self.mail_lines_cache.push((item.summary.clone(),item.html.clone(),item.uid,lines.clone()));lines
    }

    pub(crate) fn display_mail(&self,item:&mail::Item)->mail::Item {
        let mut safe=item.clone();
        if self.mail_code_hidden(item) {
            if let Some(code)=&item.code {
                safe.subject=crate::mail_privacy::mask(&safe.subject,code);
                safe.from=crate::mail_privacy::mask(&safe.from,code);
                safe.summary=self.mail.privacy.masked_body(item);
                // HTML 原文里可能还有未遮蔽的验证码，隐藏时必须一起丢弃。
                // 用共享的空 Arc，让排版缓存对隐藏邮件也能按指针命中，不必每帧重解析。
                static EMPTY: std::sync::OnceLock<std::sync::Arc<str>> = std::sync::OnceLock::new();
                safe.html=EMPTY.get_or_init(||std::sync::Arc::from("")).clone();
                safe.links.clear(); // Avoid tooltip/URL/copy leaks while hidden.
                if let Some(event)=&mut safe.meeting {event.title=crate::mail_privacy::mask(&event.title,code);}
                safe.code=None;
            }
        }
        safe
    }

    /// 收信服务推来的事件；历史邮件静默，新邮件进入通知队列。
    pub fn mail_event(&mut self, ev: mail::Event) {
        if !self.module_on(MAIL_PAGE){return;}
        let watching=|email:&str|self.mail_active() && self.mail.accounts.iter().any(|a|a.enabled && a.email.eq_ignore_ascii_case(email));
        match ev {
            mail::Event::Managed(done)=>self.mail_managed(done),
            mail::Event::Status { account, status } => {
                if !watching(&account){return;}
                if status==mail::Status::Ready{
                    self.mail.manage.data.remove(&(account.to_ascii_lowercase(),crate::mail_manage::Folder::Inbox));self.mail.manage.next_refresh=0.0;
                    if (!self.mail.manage.trash_open||!self.settings.mail_sync_operations)&&self.mail.open_uid.as_ref().is_some_and(|(a,_)|a.eq_ignore_ascii_case(&account)){
                        self.mail.open_uid=None;self.mail.manage.auto_read=None;
                    }
                }
                self.mail.status.insert(account.to_ascii_lowercase(), status);
            }
            mail::Event::Mail { item, new } => {
                if !watching(&item.account){return;}
                if !new&&self.mail.manage.data.get(&(item.account.to_ascii_lowercase(),crate::mail_manage::Folder::Inbox)).is_some_and(|s|!s.entries.iter().any(|(uid,_)|*uid==item.uid)){return;}
                let first_new=self.mail.privacy.observe(&item,new,self.t,chrono::Utc::now().timestamp());
                if self.mail_locally_removed(&item){
                    self.mail.manage.trash.retain(|m|m.uid!=item.uid||!m.account.eq_ignore_ascii_case(&item.account));self.mail.manage.trash.insert(0,item);self.mail.manage.trash.truncate(60);return;
                }
                self.mail
                    .recent
                    .retain(|m| !(m.account.eq_ignore_ascii_case(&item.account) && m.uid == item.uid));
                let notify = new && (!self.settings.mail_code_only_notifications || item.code.is_some());
                self.mail.recent.insert(0, item);
                self.mail.recent.truncate(60);
                if new{self.mail.manage.next_refresh=0.0;}
                self.mail.privacy.retain(self.mail.recent.iter().chain(&self.mail.manage.trash).chain(&self.mail.reading));
                if notify {
                    let item=&self.mail.recent[0];
                    let body=format!("{} · {}",item.from,item.subject);
                    let body=item.code.as_ref().map(|code|crate::mail_privacy::mask(&body,code)).unwrap_or(body);
                    if item.code.is_some() && self.settings.mail_code_popup && self.settings.mail_code && self.settings.notifications {
                        if first_new { self.mail.code_alerts.push((item.account.clone(), item.uid, body)); }
                    } else {
                        self.new_mail_alert(body);
                    }
                }
            }
            mail::Event::Test { account, ok, note } => {
                self.mail.testing = None;
                let detail = if ok { "连接成功".to_string() } else { note };
                let detail = if detail.chars().count() > 120 {
                    format!("{}…", detail.chars().take(120).collect::<String>())
                } else {
                    detail
                };
                self.mail.note = format!("{account}：{detail}");
                self.say(if ok {
                    format!("{account} 连接成功")
                } else {
                    format!("{account} 连接失败")
                });
            }
        }
    }

    /// 设置窗口第 8 个标签页：邮件。
    pub fn open_mail_settings(&mut self) {
        self.settings_tab = 7;
        // 从岛上「去添加」进来时直接打开添加邮箱弹窗。
        self.mail.form.open = true;
        self.open_settings();
    }

    /// v0.5.6：日历服务推来的事件。失败不弹窗，只累计次数（连续两次才显示红字）。
    pub fn calendar_event(&mut self, ev: calendar::Event) {
        if !self.module_on(CALENDAR_PAGE) {return;}
        match ev {
            calendar::Event::Test { ok, message, count, config } => {
                self.calendar.testing = false;
                if !ok {
                    self.calendar.note = message;
                    self.calendar.note_error = true;
                    return;
                }
                let Some(cfg) = config else {
                    self.calendar.note = "服务器没有返回日历地址".into();
                    self.calendar.note_error = true;
                    return;
                };
                if let Err(e) = calendar::save_connection(&cfg, &self.calendar.test_password, self.calendar.config.as_ref()) {
                    self.calendar.note = e;
                    self.calendar.note_error = true;
                    return;
                }
                self.calendar.note = if message.is_empty(){format!("已连接 · 未来 24 小时 {count} 个日程")}else{format!("已连接 · {message}")};
                self.calendar.note_error = false;
                self.calendar.warning = message.clone();
                if self.calendar.config.as_ref()!=Some(&cfg) {
                    self.calendar.events.clear();
                    self.calendar.last_sync.clear();
                    self.calendar.view.events.clear();
                    self.calendar.view.loaded=None;
                    self.calendar.view.loading=false;
                    self.calendar.view.error.clear();
                }
                if !self.calendar.feeds.iter().any(|feed| feed.config == cfg) {
                    self.calendar.feeds.retain(|feed| feed.config.provider != cfg.provider);
                    self.calendar.feeds.push(CalendarFeed::new(cfg.clone()));
                }
                if let Some(feed) = self.calendar.feeds.iter_mut().find(|feed| feed.config == cfg) {
                    feed.connected = true;
                    feed.failures = 0;
                    feed.error.clear();
                    feed.warning = message;
                }
                self.calendar.config = Some(cfg);
                self.calendar.connected = true;
                self.calendar.failures = 0;
                self.calendar.error.clear();
                self.calendar.test_password.clear();
                self.calendar.form.password.clear();
                self.pending.push(Cmd::CalendarSync);
                self.request_calendar_month(true);
            }
            calendar::Event::Synced { config, events, at } => {
                if let Some(feed)=self.calendar.feeds.iter_mut().find(|feed|feed.config==config) {
                    feed.events=events.clone();feed.connected=true;feed.failures=0;feed.error.clear();feed.warning.clear();feed.last_sync=at.clone();
                }
                if self.calendar.config.as_ref()!=Some(&config){return;}
                self.calendar.events = events;
                self.calendar.connected = true;
                self.calendar.failures = 0;
                self.calendar.error.clear();
                self.calendar.last_sync = at;
                self.calendar.warning.clear();
            }
            calendar::Event::MonthLoaded { config, month, result } => {
                if !self.calendar.feeds.is_empty() {
                    if month!=self.calendar.view.month {return;}
                    let Some(feed)=self.calendar.feeds.iter_mut().find(|feed|feed.config==config) else{return;};
                    feed.month.loading=false;
                    feed.month.warning.clear();
                    match result {
                        Ok(events)=>{feed.month.events=events;feed.month.loaded=Some(month);feed.month.error.clear();}
                        Err(message)=>feed.month.error=message,
                    }
                    self.refresh_calendar_month_status();
                    return;
                }
                let view=&mut self.calendar.view;
                if month!=view.month || self.calendar.config.as_ref()!=Some(&config){return;}
                view.loading=false;
                view.warning.clear();
                match result {
                    Ok(events)=>{view.events=events;view.loaded=Some(month);view.error.clear();}
                    Err(message)=>view.error=message,
                }
            }
            calendar::Event::Warning{config,month,message}=>{
                if let Some(month)=month{
                    if month!=self.calendar.view.month{return;}
                    if self.calendar.feeds.is_empty(){
                        if self.calendar.config.as_ref()==Some(&config){self.calendar.view.warning=message;}
                    }else if let Some(feed)=self.calendar.feeds.iter_mut().find(|feed|feed.config==config){
                        feed.month.warning=message;self.refresh_calendar_month_status();
                    }
                }else{
                    if let Some(feed) = self.calendar.feeds.iter_mut().find(|feed| feed.config == config) {
                        feed.connected = true;
                        feed.failures = 0;
                        feed.error.clear();
                        feed.warning = message.clone();
                    }
                    if self.calendar.config.as_ref() == Some(&config) {
                        self.calendar.connected = true;
                        self.calendar.failures = 0;
                        self.calendar.error.clear();
                        self.calendar.warning = message;
                    }
                }
            }
            calendar::Event::Failed { config, message } => {
                if let Some(feed)=self.calendar.feeds.iter_mut().find(|feed|feed.config==config) {
                    feed.connected=false;feed.failures=feed.failures.saturating_add(1);feed.error=message.clone();feed.warning.clear();
                }
                if self.calendar.config.as_ref()==Some(&config) {
                    self.calendar.connected = false;
                    self.calendar.failures = self.calendar.failures.saturating_add(1);
                    self.calendar.error = message;
                    self.calendar.warning.clear();
                }
            }
        }
    }

    /// 所有已连接的平台都参与同步，设置标签只决定正在编辑的账号。
    pub fn calendar_configs(&self) -> Vec<calendar::Config> {
        if self.calendar.feeds.is_empty() {self.calendar.config.clone().into_iter().collect()}
        else {self.calendar.feeds.iter().map(|feed|feed.config.clone()).collect()}
    }

    /// 等价于 `calendar_configs()==known`，外壳每帧调用，不复制配置。
    pub fn calendar_configs_match(&self, known: &[calendar::Config]) -> bool {
        if self.calendar.feeds.is_empty() {known==self.calendar.config.as_slice()}
        else {known.len()==self.calendar.feeds.len() && known.iter().zip(&self.calendar.feeds).all(|(known,feed)|*known==feed.config)}
    }

    pub(crate) fn refresh_calendar_month_status(&mut self) {
        let month=self.calendar.view.month;
        self.calendar.view.loading=self.calendar.feeds.iter().any(|feed|feed.month.loading);
        self.calendar.view.loaded=(!self.calendar.feeds.is_empty() && self.calendar.feeds.iter().all(|feed|feed.month.loaded==Some(month))).then_some(month);
        self.calendar.view.error=self.calendar.feeds.iter().filter(|feed|!feed.month.error.is_empty())
            .map(|feed|format!("{}：{}",feed.config.provider.label(),feed.month.error)).collect::<Vec<_>>().join("；");
        self.calendar.view.warning=self.calendar.feeds.iter().filter(|feed|feed.month.loaded==Some(month)&&!feed.month.warning.is_empty())
            .map(|feed|format!("{}：{}",feed.config.provider.label(),feed.month.warning)).collect::<Vec<_>>().join("；");
    }

    pub(crate) fn select_calendar_provider(&mut self, provider: calendar::Provider) {
        if self.calendar.testing || self.calendar.form.provider == provider { return; }
        if let Err(error) = calendar::select_provider(provider) {
            self.calendar.note = error;
            self.calendar.note_error = true;
            return;
        }
        let feed=self.calendar.feeds.iter().find(|feed|feed.config.provider==provider);
        self.calendar.config=feed.map(|feed|feed.config.clone());
        self.calendar.events=feed.map(|feed|feed.events.clone()).unwrap_or_default();
        self.calendar.connected=feed.is_some_and(|feed|feed.connected);
        self.calendar.last_sync=feed.map(|feed|feed.last_sync.clone()).unwrap_or_default();
        self.calendar.failures=feed.map(|feed|feed.failures).unwrap_or_default();
        self.calendar.error=feed.map(|feed|feed.error.clone()).unwrap_or_default();
        self.calendar.warning=feed.map(|feed|feed.warning.clone()).unwrap_or_default();
        self.calendar.form=CalendarForm{provider,username:self.calendar.config.as_ref().map(|cfg|cfg.username.clone()).unwrap_or_default(),
            server:self.calendar.config.as_ref().filter(|cfg|cfg.provider.custom_server()).and_then(|cfg|cfg.server.clone()).unwrap_or_default(),..Default::default()};
        self.calendar.note.clear();self.calendar.note_error=false;
    }

    /// 只移除正在管理的平台，保留其他平台的日程与提醒。
    pub fn forget_calendar(&mut self) -> Option<calendar::Config> {
        let old = self.calendar.config.take();
        if let Some(config)=&old {
            self.calendar.feeds.retain(|feed|feed.config!=*config);
            let is_old=|alert:&crate::extras::Alert|alert.meeting.as_ref().is_some_and(|(_,source)|source==config.provider.label());
            self.alerts.retain(|alert|!is_old(alert));
            if self.notice.as_ref().is_some_and(is_old){self.notice=None;}
        }
        self.calendar.connected = false;
        self.calendar.events.clear();
        self.calendar.view.events.clear();
        self.refresh_calendar_month_status();
        self.calendar.last_sync.clear();
        self.calendar.failures = 0;
        self.calendar.error.clear();
        self.calendar.note.clear();
        self.calendar.warning.clear();
        old
    }

    /// 外壳同步收信线程时读账号清单（不含任何凭据）。
    pub fn mail_accounts(&self) -> &[mail::Account] {
        &self.mail.accounts
    }

    /// 账号增删或开关变化后：落盘并让外壳重新对齐收信线程。
    pub(crate) fn mail_accounts_changed(&mut self) {
        self.mail_lines_cache.clear();
        let accounts=&self.mail.accounts;
        let known=&self.mail.manage.identities;
        let active=|email:&str|accounts.iter().any(|a|a.enabled && mail::key(a)==email && known.get(email).is_none_or(|old|old==a));
        self.mail.manage.data.retain(|(email,_),_|active(email));
        self.mail.recent.retain(|m|active(&m.account.to_ascii_lowercase()));
        self.mail.manage.trash.retain(|m|active(&m.account.to_ascii_lowercase()));
        if self.mail.reading.as_ref().is_some_and(|m|!active(&m.account.to_ascii_lowercase())){self.mail.reading=None;}
        if self.mail.open_uid.as_ref().is_some_and(|(email,_)|!active(&email.to_ascii_lowercase())){self.mail.open_uid=None;self.mail.manage.auto_read=None;}
        self.mail.manage.identities.retain(|_,old|accounts.contains(old));
        self.mail.manage.next_refresh=0.0;
        mail::save_accounts(accounts);
        self.pending.push(Cmd::MailSync);
    }

    /// 录制/截图用的邮件样例（只在 `record` feature 下编译）。
    ///
    /// `DAOZAI_MAIL_DEMO=1|code` 面板带验证码，`=list` 只列邮件，`=fail` 用假授权码走一次
    /// 「测试连接失败」，方便在没有真实邮箱时验收失败路径。
    #[cfg(feature = "record")]
    pub fn mail_demo(&mut self) {
        use mail::{Account, Item, Provider, Status};
        let mode = std::env::var("DAOZAI_MAIL_DEMO").unwrap_or_default();
        self.settings_tab = 7;
        if mode == "fail" {
            self.mail.form.provider = Provider::Qq;
            self.mail.form.email = "test@qq.com".into();
            self.mail.form.password = "invalid-auth-code".into();
            self.mail.note = "test@qq.com：正在测试连接…".into();
            self.pending.push(Cmd::MailTest(
                Account { use_windows_proxy: false, custom: None, email: "test@qq.com".into(), provider: Provider::Qq, enabled: true },
                "invalid-auth-code".into(),
            ));
            return;
        }
        self.mail.accounts = vec![
            Account { use_windows_proxy: false, custom: None, email: "demo@qq.com".into(), provider: Provider::Qq, enabled: false },
            Account { use_windows_proxy: false, custom: None, email: "demo@icloud.com".into(), provider: Provider::ICloud, enabled: false },
        ];
        for a in &self.mail.accounts {
            self.mail.status.insert(a.email.clone(), Status::Ready);
        }
        // 会议演示：开始时间设成 10 分钟后，正好落在默认 15 分钟提前量里，
        // 启动即触发一次「会议提醒」。
        let mstart = chrono::Local::now().timestamp() + 600;
        let mat = chrono::DateTime::from_timestamp(mstart, 0)
            .map(|d| d.with_timezone(&chrono::Local).format("%m-%d %H:%M").to_string())
            .unwrap_or_default();
        let rows = [
            (
                "GitHub",
                "[GitHub] Please verify your device",
                "09:41",
                Some("482913"),
                "We noticed a sign-in attempt from a new device. Enter 482913 to finish signing in. If this wasn't you, secure your account now.",
                None,
            ),
            (
                "Apple",
                "Your Apple ID verification code",
                "09:38",
                None,
                "Your Apple ID code is 573104. Don't share it with anyone. If you didn't request it, you can ignore this message.",
                None,
            ),
            (
                "腾讯企业邮",
                "会议邀请：周五方案评审",
                "09:12",
                None,
                "本周五 14:30 在三楼会议室评审 v0.5.5 方案，请提前十分钟到场。会议邀请见附件。",
                Some((mstart, mat)),
            ),
            (
                "顺丰速运",
                "您的快件已到达菜鸟驿站",
                "08:57",
                None,
                "您的快件已到达菜鸟驿站（望京 SOHO T1 店），请凭取件码 6-3-2 在 20:00 前领取。",
                None,
            ),
        ];
        let with_code = mode != "list";
        self.mail.recent = rows
            .iter()
            .enumerate()
            .map(|(i, (from, subject, time, code, summary, meeting))| Item {
                received_unix: None, account: "demo@qq.com".into(),
                from: (*from).into(),
                subject: (*subject).into(),
                time: (*time).into(),
                code: if with_code { (*code).map(str::to_string) } else { None },
                summary: (*summary).into(),
                html: String::new().into(),
                links: Vec::new(),
                meeting: meeting.as_ref().map(|(start, at)| mail::Meeting { all_day:false, end:None, title: "周五方案评审".into(), start: *start, at: at.clone(), uid: "demo-ics-1".into() }),
                uid: 900 - i as u32,
            })
            .collect();
        self.mail.note = "demo@qq.com：连接成功".into();
        self.page = 5;
        self.dots_target();
    }

    /// 录制/截图用的日程样例（只在 `record` feature 下编译）。
    ///
    /// `DAOZAI_CAL_DEMO=paste|fail|connected|alert`。
    #[cfg(feature = "record")]
    pub fn calendar_demo(&mut self) {
        use calendar::Provider;
        let mode = std::env::var("DAOZAI_CAL_DEMO").unwrap_or_default();
        self.calendar.form.provider = Provider::Feishu;
        self.calendar.form.username = "demo***@feishu.example".into();
        self.settings_tab = 8;
        self.open_settings();
        self.settings.calendar_prompt = false;
        let later = |secs: i64| chrono::Local::now().timestamp() + secs;
        let label = |ts: i64| {
            chrono::DateTime::from_timestamp(ts, 0)
                .map(|d| d.with_timezone(&chrono::Local).format("%m-%d %H:%M").to_string())
                .unwrap_or_default()
        };
        let demo_cfg = calendar::Config {
            provider: Provider::Feishu,
            username: "demo***@feishu.example".into(),
            href: Some("https://caldav.feishu.cn/calendars/8f3c2d7a-1111-2222-3333-444455556666/".into()),
            server: None,
        };
        match mode.as_str() {
            "paste" => {
                self.calendar.form.username =
                    "服务器地址：https://caldav.feishu.cn/calendars/\n用户名：demo***@feishu.example\n密码：demo-pass-123".into();
                if let Some((u, p)) = calendar::parse_paste(&self.calendar.form.username) {
                    self.calendar.form.username = u;
                    self.calendar.form.password = p;
                    self.calendar.note = "已从粘贴内容拆出用户名和密码，服务器地址已按平台填好".into();
                }
            }
            "fail" => {
                self.calendar.form.password = "demo-pass-123".into();
                self.calendar.note = calendar::human_error(Provider::Feishu, Some(401));
                self.calendar.note_error = true;
            }
            "connected" => {
                let a = later(3600);
                let b = later(6 * 3600);
                self.calendar.form.password = "demo-pass-123".into();
                self.calendar.events = vec![
                    mail::Meeting { all_day:false, end:None, title: "产品评审".into(), start: a, at: label(a), uid: "demo-cal-1".into() },
                    mail::Meeting { all_day:false, end:None, title: "设计周会".into(), start: b, at: label(b), uid: "demo-cal-2".into() },
                ];
                self.calendar.config = Some(demo_cfg.clone());
                self.calendar.connected = true;
                self.calendar.last_sync = calendar::now_label();
                self.calendar.note.clear();
                self.calendar.note_error = false;
            }
            "alert" => {
                let s = later(600);
                self.calendar.events =
                    vec![mail::Meeting { all_day:false, end:None, title: "飞书日程：产品评审".into(), start: s, at: label(s), uid: "demo-cal-alert".into() }];
                self.calendar.config = Some(demo_cfg.clone());
                self.calendar.connected = true;
                self.calendar.last_sync = calendar::now_label();
                self.settings_open = false;
            }
            _ => {}
        }
    }

    /// v0.5.4：把系统音量与蓝牙设备状态同步进界面；只在连接/断开时提示一次。
    fn sync_device(&mut self, env: &Env) {
        if !(self.open&&self.page==BLUETOOTH_PAGE) {self.wireless_enter_at=None;}
        self.track_wireless_changes(&env.bt);
        self.wireless_devices=env.bt.clone();
        if self.module_on(BLUETOOTH_PAGE) {
            self.wireless_devices.extend(env.receivers.iter().cloned());
        }
        if self.wireless_focus.as_ref().is_some_and(|id|!self.wireless_devices.iter().any(|d|&d.id==id)) {
            self.wireless_focus=None;
        }
        if self.t >= self.volume_quiet_until {
            if let Some(v) = env.volume {
                self.volume_ui = Some(v);
            }
        }
        let battery_enabled=self.settings.bluetooth_battery_notifications&&self.module_on(BLUETOOTH_PAGE);
        let connection_enabled=self.settings.bluetooth_notifications;
        if !battery_enabled||!connection_enabled {
            let enabled=|a:&crate::extras::Alert| {
                a.source!=crate::extras::AlertSource::Internal||a.title!="蓝牙设备"||
                    if a.page==BLUETOOTH_PAGE {battery_enabled} else {connection_enabled}
            };
            self.alerts.retain(&enabled);
            if self.notice.as_ref().is_some_and(|a|!enabled(a)) {self.notice=None;}
        }
        if !connection_enabled {
            self.bt_shown.clear();
            self.bt_initialized=false;
            return;
        }
        if !env.bt_ready { return; }
        // 只把首次完整扫描作为基线，之后的连接变化都应提示。
        if !self.bt_initialized {
            self.bt_initialized = true;
            self.bt_shown = env.bt.iter().filter(|d|d.connected).cloned().collect();
            return;
        }
        let connected: Vec<_> = env.bt.iter().filter(|d| d.connected && !self.bt_shown.iter().any(|old| old.id == d.id)).cloned().collect();
        let disconnected: Vec<_> = self.bt_shown.iter().filter(|d| !env.bt.iter().any(|new| new.connected && new.id == d.id)).cloned().collect();
        for device in &connected { self.bluetooth_alert(device, true); }
        for device in &disconnected { self.bluetooth_alert(device, false); }
        self.bt_shown = env.bt.iter().filter(|d|d.connected).cloned().collect();
    }

    /// 距离上次交互的秒数（小精灵出现的依据）。
    pub fn idle_for(&self) -> f64 {
        self.system_idle.unwrap_or(self.t - self.last_input)
    }

    pub fn show_page(&mut self,page:usize) {
        if !self.module_on(page) {
            self.open_page_settings(page);
            return;
        }
        self.notice=None;
        self.go(page,1.0);
        self.open=true; self.forced_open_until=self.t+4.0;
        self.w.tune(OPEN_W.0,OPEN_W.1);self.h.tune(OPEN_H.0,OPEN_H.1);
        self.w.target=PW;self.h.target=self.expanded_height();
    }
    pub(crate) fn window_layout(&self)->std::sync::MutexGuard<'_,crate::settings_window::State>{
        self.settings_window.lock().unwrap_or_else(|e|e.into_inner())
    }

    pub fn save_window_before_exit(&self){self.window_layout().close();}

    #[cfg(windows)]
    pub fn start_tray(&self,state:crate::tray::State)->Option<crate::tray::Tray>{
        let layout=self.settings_window.clone();
        // 原生文件选择框会占用界面线程，托盘退出时直接保存最近一次观察到的布局。
        crate::tray::Tray::start_with_exit(state,move||{layout.lock().unwrap_or_else(|e|e.into_inner()).close();})
    }

    pub fn open_settings(&mut self){
        #[cfg(feature="sdk-js")]
        if self.settings_tab != crate::extras::SETTINGS_TAB_PLUGIN_DETAIL { self.settings_plugin = None; }
        self.settings_open=true;
        self.settings_focus_requested=true;
    }
    pub fn open_settings_category(&mut self,category:usize){
        self.settings_category=category.min(3);
        self.health_time=false;
        if category==0 || category>=3 {self.settings_tab=if category==0{0}else{6};self.open_settings();return;}
        let page=if category==1{2}else{1};
        self.show_page(page);
        if self.settings.visible_pages[page] && (category==1 || category==2) {self.subpage=Some(page);}
    }
    pub fn page(&self) -> usize {
        self.page
    }
    pub fn is_plugin_status_page(&self) -> bool {
        self.page >= PAGE_NAMES.len()
    }
    /// 打开健康提醒设置里的「提醒时段」二级页（演示录制用）。
    pub fn show_health_time(&mut self) { self.health_time = true; }
    /// 切到饮水页的久坐屏，并假定已连续久坐 minutes 分钟（演示录制用）。
    pub fn show_sit_view(&mut self, minutes: f64) {
        self.show_page(WATER_PAGE);
        self.sit_mode = true;
        self.sit_start = self.t - minutes * 60.0;
    }

    // ---------- 功能模块：页面开关即模块开关，关闭后页面隐藏、后台同步和提醒一并停止 ----------

    /// 模块是否开启（与「页面管理」里的页面开关一致）。
    pub fn module_on(&self,page:usize)->bool {
        self.settings.visible_pages.get(page).copied().unwrap_or_else(||page-PAGE_NAMES.len()<self.status_page_count())
    }
    /// 收信线程是否应运行：邮箱模块开启且打开了「新邮件提醒」。
    pub fn mail_active(&self)->bool {self.module_on(MAIL_PAGE) && self.settings.mail_enabled}
    /// 剪贴板监听是否应运行：置物架模块开启且打开了「记录剪贴板」。
    pub fn clipboard_active(&self)->bool {self.module_on(SHELF_PAGE) && self.settings.clipboard_history}
    /// 额度查询是否应运行：AI 额度模块开启且打开了「开启 AI 额度」。
    pub fn quota_active(&self)->bool {self.module_on(QUOTA_PAGE) && self.quota.config.enabled}
    /// 旧版设置页里「新邮件提醒」「开启 AI 额度」与页面管理的模块开关是两层开关。
    /// 合并后只保留模块开关：旧的内部开关关着时，迁移为模块关闭，内部开关恢复为开。
    /// 只剩这一个页面、模块关不掉时保持原样，不改变已有行为。
    pub fn migrate_merged_switches(&mut self){
        let mut settings_changed=false;
        if !self.settings.mail_enabled {
            self.set_page_visible(MAIL_PAGE,false);
            if !self.settings.visible_pages[MAIL_PAGE] {self.settings.mail_enabled=true;settings_changed=true;}
        }
        if !self.quota.config.enabled {
            self.set_page_visible(QUOTA_PAGE,false);
            if !self.settings.visible_pages[QUOTA_PAGE] {self.quota.config.enabled=true;self.quota.config.save();settings_changed=true;}
        }
        if settings_changed {self.settings.save();}
    }

    pub(crate) fn set_page_visible(&mut self,page:usize,visible:bool) {
        if !visible && self.settings.visible_pages.iter().filter(|enabled|**enabled).count()==1
            && self.settings.visible_pages[page] { return; }
        self.settings.visible_pages[page]=visible;
        #[cfg(feature="sdk-js")]
        if !visible {
            match page {
                TODO_PAGE=>self.plugins.pause_owner("tasks"),
                CALENDAR_PAGE=>self.plugins.pause_owner("calendar"),
                QUOTA_PAGE=>self.plugins.pause_owner("quota"),
                _=>{}
            }
        }
        if page==SHELF_PAGE {self.clipboard.set_enabled(self.clipboard_active());}
        if !visible {
            match page {
                FOCUS_PAGE=>self.pomo.running=false,
                MAIL_PAGE=>self.close_mail_module(),
                CALENDAR_PAGE=>{
                    self.calendar.view.loading=false;
                    for feed in &mut self.calendar.feeds {feed.month.loading=false;}
                    self.calendar.testing=false;
                    self.calendar.test_password.clear();
                    self.calendar.form.open=false;
                    self.pending.retain(|cmd|!matches!(cmd,Cmd::CalendarTest(..)|Cmd::CalendarSync|Cmd::CalendarMonth(_)|Cmd::CalendarForget|Cmd::CalendarImport));
                }
                BLUETOOTH_PAGE=>{
                    self.wireless_focus=None;
                    self.wireless_offset=0;
                }
                _=>{}
            }
            self.clear_module_reminders(page);
        }
        if !self.module_on(self.page) {
            if let Some(first)=self.settings.page_order.iter().copied().find(|page|self.settings.visible_pages[*page]) { self.go(first,1.0); }
        }
    }

    fn status_page_count(&self) -> usize {
        #[cfg(feature="sdk-js")]
        { self.status_pages.len() }
        #[cfg(not(feature="sdk-js"))]
        { 0 }
    }

    fn navigation_pages(&self) -> impl Iterator<Item=usize> + '_ {
        self.settings.page_order.iter().copied()
            .filter(|page|self.settings.visible_pages[*page])
            .chain(PAGE_NAMES.len()..PAGE_NAMES.len()+self.status_page_count())
    }

    fn navigation_page_name(&self, page: usize) -> &str {
        if let Some(name)=PAGE_NAMES.get(page) {return name;}
        #[cfg(feature="sdk-js")]
        if let Some((_,name))=self.status_pages.get(page-PAGE_NAMES.len()) {return name;}
        "页面管理"
    }

    fn page_settings_tab(&self, page: usize) -> usize {
        PAGE_SETTINGS_TABS.get(page).copied().unwrap_or(crate::extras::SETTINGS_TAB_PAGES)
    }

    fn open_page_settings(&mut self, page: usize) {
        #[cfg(feature="sdk-js")]
        if let Some(id) = page.checked_sub(PAGE_NAMES.len())
            .and_then(|index| self.status_pages.get(index)).map(|(id, _)| id.clone())
        {
            self.open_plugin_settings(&id);
            return;
        }
        self.settings_tab = self.page_settings_tab(page);
        self.open_settings();
    }

    #[cfg(feature="sdk-js")]
    pub(crate) fn open_plugin_settings(&mut self, id: &str) {
        if self.plugins.standalone_settings_pages().any(|(plugin, _)| plugin == id) {
            self.settings_plugin = Some(id.to_owned());
            self.settings_tab = crate::extras::SETTINGS_TAB_PLUGIN_DETAIL;
        } else {
            self.settings_plugin = None;
            self.settings_tab = self.plugins.settings_destination(id)
                .unwrap_or(crate::extras::SETTINGS_TAB_PLUGINS);
        }
        self.open_settings();
    }

    #[cfg(feature="sdk-js")]
    fn sync_status_pages(&mut self) {
        if self.status_pages.iter().map(|(id,name)|(id.as_str(),name.as_str())).eq(self.plugins.status_pages()) {
            return;
        }
        let pages:Vec<_>=self.plugins.status_pages().map(|(id,name)|(id.to_owned(),name.to_owned())).collect();
        // 数字页码只用于绘制和翻页，安装或停用其他插件时按 ID 找回原来的页面。
        let remap=|page:usize| {
            if page<PAGE_NAMES.len() {return Some(page);}
            let (id,_)=self.status_pages.get(page-PAGE_NAMES.len())?;
            pages.iter().position(|(other,_)|other==id).map(|index|PAGE_NAMES.len()+index)
        };
        let current=remap(self.page);
        let previous=remap(self.page_from);
        self.drag_saved=self.drag_saved.and_then(remap);
        self.status_pages=pages;
        if let Some(page)=current {
            self.page=page;
            self.page_from=previous.unwrap_or(page);
        } else {
            let fallback=self.navigation_pages().next().unwrap_or(MUSIC_PAGE);
            self.page=fallback;
            self.page_from=self.page;
            self.page_t0=self.t-PAGE_TURN_DURATION as f64;
            self.subpage=None;
            self.cancel_edit();
        }
        self.dots_target();
    }

    #[cfg(feature="sdk-js")]
    pub(crate) fn show_status_page(&mut self, id: &str) {
        self.sync_status_pages();
        if let Some(index)=self.status_pages.iter().position(|(source,_)|source==id) {
            self.show_page(PAGE_NAMES.len()+index);
        }
    }

    fn navigate_page(&mut self,next:bool) {
        let pages:Vec<_>=self.navigation_pages().collect();
        let Some(position)=pages.iter().position(|&page|page==self.page) else {return;};
        let offset=if next {1} else {pages.len()-1};
        self.go(pages[(position+offset)%pages.len()],if next {1.0}else{-1.0});
    }

    /// OLED 像素偏移只在收起动画完成、没有鼠标交互、不在编辑时移动；
    /// 展开、点击、拖拽时保持当前位置，避免面板内容和位移一起跳。
    pub fn oled_shift_idle(&self,ctx:&egui::Context) -> bool {
        crate::oled_shift::should_move(self.open, self.position_drag.is_some() || self.drag_glide.is_some(), self.is_editing())
            && self.hover_since.is_none() && !self.drag_hot
            && self.w.settled(PW) && self.h.settled(self.expanded_height())
            && self.drag_lift.settled(1.0) && self.drag_stretch.settled(1.0)
            && ctx.input(|i|!i.pointer.any_down() && !i.pointer.any_pressed() && i.raw.hovered_files.is_empty())
    }

    /// 慢帧日志用：当前页与展开状态。
    pub fn debug_state(&self) -> (usize, bool) {
        (self.page, self.open)
    }

    pub fn is_editing(&self) -> bool {
        #[cfg(feature="sdk-js")]
        if self.plugin_form_focused { return true; }
        self.adding || self.editing.is_some() || self.todo_choosing || self.task_dialog.is_some() || self.calendar.draft.is_some() || self.settings_editing
    }

    pub fn foreground_changed(&mut self,ctx:&egui::Context) {
        if ctx.viewport(|viewport|viewport.ime_owns_keys) {return;}
        self.hover_lock=!self.hold_open;
        if self.adding && self.buf.trim().is_empty(){self.cancel_edit();}
    }

    /// 带过渡地切到某页。`dir` > 0 表示内容从右侧进入。
    pub(crate) fn go(&mut self, page: usize, dir: f32) {
        if page == self.page || !self.module_on(page) { return; }
        if self.drag_departing {
            self.drag_departing=false;
            self.drag_saved=None;
        }
        self.music_volume_open=false;
        self.subpage=None;
        // 连点时，尚在滑入的页面从当前的位置和透明度开始淡出。
        let elapsed = self.page_e();
        self.page_from_offset = self.page_slide_offset(elapsed);
        self.page_from_elapsed = elapsed;
        self.page_from = self.page;
        self.page = page;
        if page==CALENDAR_PAGE {self.request_calendar_month(false);}
        self.page_dir = dir;
        self.page_t0 = self.t;
        self.dots_target();
        self.cancel_edit();
    }

    fn page_e(&self) -> f32 {
        (self.t - self.page_t0) as f32
    }

    fn page_slide_offset(&self, elapsed: f32) -> f32 {
        self.page_dir * PAGE_SLIDE * (1.0 - smoothstep(0.0, PAGE_TURN_DURATION, elapsed))
    }

    fn page_slide_alpha(elapsed: f32) -> f32 {
        smoothstep(0.03, 0.28, elapsed)
    }

    fn page_header_alpha(elapsed: f32) -> f32 {
        smoothstep(0.10, 0.23, elapsed)
    }

    fn dots_target(&mut self) {
        for (i, d) in self.dots.iter_mut().enumerate() {
            d.target = if i == self.page { 16.0 } else { 4.0 };
        }
    }

    pub(crate) fn cancel_edit(&mut self) {
        #[cfg(feature="sdk-js")]
        {self.plugin_form_focused=false;}
        self.settings_editing=false;
        self.adding = false;
        self.editing = None;
        self.todo_choosing = false;
        self.task_dialog=None;
        self.calendar.draft=None;
        self.buf.clear();
        self.todo_input_focused=false;
    }

    /// 外壳据此调整音乐采样频率：看着音乐页时最快，收起显示歌词次之，其余只需跟上歌名和播放状态。
    pub fn media_poll_interval(&self)->std::time::Duration {
        if self.open && self.page==MUSIC_PAGE {crate::smtc::POLL_FAST}
        else if self.island_lyrics_active() && !self.open {crate::smtc::POLL_LYRICS}
        else {crate::smtc::POLL_BACKGROUND}
    }

    fn island_lyrics_active(&self)->bool {
        if self.side() {return false;}
        self.settings.island_lyrics && self.settings.visible_pages[0] && self.media.is_music() && self.media.playing
            && !self.browser_minimal_active
    }
    pub(crate) fn compact_notices(&self)->bool {
        #[cfg(windows)]
        if self.notice.as_ref().is_some_and(|a| matches!(a.source, crate::extras::AlertSource::Plugin { .. })) {
            return false;
        }
        self.island_lyrics_active()&&!self.open&&!self.notice.as_ref().is_some_and(|a|a.meeting.is_some()||self.settings.notification_preview&&matches!(a.source,crate::extras::AlertSource::External(_)))
    }

    /// 换句、分段和展开高亮共用时间调整；正数延后，负数提前。
    pub(crate) fn lyric_position(&self)->f32 {(self.media.position-self.settings.lyric_delay_ms as f64/1000.0)as f32}

    /// 当前歌曲没有进度采样且备用读取不可用时，岛上显示歌名。
    pub(crate) fn music_lyrics_limited(&self)->bool {
        #[cfg(windows)] {self.media.sampled_at.is_none() && crate::smtc::music_compat::lyrics_limited(&self.media.source)}
        #[cfg(not(windows))] {false}
    }

    pub(crate) fn island_lyric(&self) -> Option<(usize,&str)> {
        if !self.island_lyrics_active() || self.music_lyrics_limited(){return None;}
        if !lyric_key_matches(&self.ly.key,&self.media) {return None;}
        let lyrics=self.ly.lyrics.as_ref().filter(|lyrics|lyrics.synced)?;
        let index=lyrics.index_at(self.lyric_position())?;
        let text=lyrics.lines.get(index)?.text.trim();
        (!text.is_empty()).then_some((index,text))
    }

    fn update_island_lyric_layout(&mut self,ctx:&egui::Context) {
        if !self.island_lyrics_active(){self.island_lyric_layout=None;return;}
        let current=self.island_lyric();
        let limited=self.music_lyrics_limited();
        let lyrics=self.ly.lyrics.as_ref().filter(|lyrics|!limited && lyric_key_matches(&self.ly.key,&self.media)
            && lyrics.synced && lyrics.lines.iter().any(|line|!line.text.trim().is_empty()));
        let title=lyrics.is_none();
        let text=if let Some((_,line))=current {line}
            else if let Some(lyrics)=lyrics {
                if lyrics.index_at(self.lyric_position()).is_none() {"前奏"}
                else {"间奏"}
            } else {self.media.title.trim()};
        let ink=if current.is_some()||title{theme::WHITE}else{theme::GRAY};
        let layout=|text:String| {
            // 维语等从右往左的歌词：先按阅读顺序分行再逐行转换，否则折行后第一行会是句子的结尾（#200）。
            let font=theme::font(11.0,W::Semibold);
            let text=crate::rtl_text::display_wrapped(&text,LYRIC_BAR_W-80.0,|s|ctx.fonts(|fonts|fonts.layout_no_wrap(s.to_owned(),font.clone(),ink).size().x)).into_owned();
            let mut job=egui::text::LayoutJob::simple(text,font,ink,LYRIC_BAR_W-80.0);
            job.halign=egui::Align::Center;
            job.wrap.break_anywhere=true;
            if title {job.wrap.max_rows=1;}
            ctx.fonts(|fonts|fonts.layout_job(job))
        };
        let mut galley=layout(text.to_owned());
        if let Some((index,_))=current.filter(|_|galley.rows.len()>1) {
            let lyrics=self.ly.lyrics.as_ref().unwrap();
            let start=lyrics.lines[index].t;
            let end=lyrics.lines.iter().skip(index+1).find(|line|line.t>start).map_or(self.media.duration as f32,|line|line.t);
            let progress=if end>start {((self.lyric_position()-start)/(end-start)).clamp(0.0,1.0)}else{0.0};
            // 按当前句的时间范围与各段字数切换，暂停和跳转直接跟随播放位置。
            let total:usize=galley.rows.iter().map(|row|row.glyphs.len()).sum();
            let mut count=0;
            let page=galley.rows.iter().position(|row|{count+=row.glyphs.len();progress*(total as f32)<count as f32}).unwrap_or(galley.rows.len()-1);
            let segment=galley.rows[page].glyphs.iter().map(|glyph|glyph.chr).collect::<String>();
            galley=layout(segment.trim().to_owned());
        }
        self.island_lyric_layout=Some(galley);
    }

    pub(crate) fn effective_minimal(&self)->bool {self.settings.minimal || self.browser_minimal_active}
    pub(crate) fn pet_enabled(&self)->bool {self.settings.pet && !self.effective_minimal() && !self.side()}
    pub(crate) fn pet_space(&self)->f32 {if self.pet_enabled(){44.0}else{0.0}}
    fn lyric_focus_space(&self)->f32 {if self.pomo.running{crate::focus_bar::LYRIC_WIDTH+10.0}else{0.0}}
    fn lyric_bar_width(&self)->f32 {LYRIC_BAR_W+if self.pet_enabled(){48.0}else{0.0}+self.lyric_focus_space()}
    fn notice_size(&self,a:&Alert)->(f32,f32) {
        if matches!(a.source, crate::extras::AlertSource::MailCode(..)) { return (270.0,60.0); }
        let (width,height)=if a.meeting.is_some(){(360.0,136.0)}else if a.source==crate::extras::AlertSource::Internal && a.title=="蓝牙设备"{(HUD_W,48.0)}else if a.is_hud(){(HUD_W,38.0)}else{(334.0,44.0)};
        (width,height)
    }
    pub fn set_pet(&mut self,enabled:bool) {
        self.settings.pet=enabled;if enabled {self.settings.minimal=false;}self.settings.save();
    }
    pub fn set_minimal(&mut self,enabled:bool) {
        self.settings.minimal=enabled;if enabled {self.settings.pet=false;}self.settings.save();
    }
    pub(crate) fn notification_bell(&self)->Option<f64>{
        let mail=self.mail.bell_at.filter(|at|self.module_on(MAIL_PAGE)&&self.t<at+6.0);
        if self.t<self.notification_bell_until {
            Some(mail.map_or(self.notification_bell_at,|at|self.notification_bell_at.max(at)))
        }else{mail}
    }
    fn base_size(&self) -> (f32, f32) {
        if !self.compact_notices()&&!self.open {
            if let Some(a)=&self.notice {let (width,height)=self.notice_size(a);return (width+self.pet_space(),height);}
        }
        if self.side() {
            // 左右停靠：竖向胶囊（通知仍是贴边滑出的横向卡片，见上）。
            return self.side_base_size(self.effective_minimal()&&self.notification_bell().is_none());
        }
        if self.island_lyrics_active() {
            (self.lyric_bar_width(),BAR_H)
        } else if self.effective_minimal()&&self.notification_bell().is_none() {
            (BAR_W_MIN, BAR_H_MIN)
        } else {
            (self.resident.target.width.min(self.resident_limit()), BAR_H)
        }
    }

    /// 常驻区比展开面板还宽时（比如开了综合监测），展开、收起途中岛宽会先窄于常驻内容。
    /// 这时常驻内容跟着岛宽一起等比收拢，而不是被岛的边缘直接切掉。
    fn resident_squeeze(&self)->f32 {
        if self.side()||self.island_lyrics_active()||self.notice.is_some() {return 1.0;}
        let content=self.resident.target.width.min(self.resident_limit());
        (self.w.pos/content.max(1.0)).clamp(0.5,1.0)
    }

    fn resident_fit(&self) -> f32 {
        #[cfg(feature = "sdk-js")]
        if !self.pinned_displays.is_empty() { return 1.0; }
        if self.island_lyrics_active() { return 1.0; }
        (self.resident_limit() / self.resident.target.width).min(1.0) * self.resident_squeeze()
    }

    pub fn requested_horizontal_width(&self) -> f32 {
        #[cfg(feature = "sdk-js")]
        return self.pinned_horizontal_width;
        #[cfg(not(feature = "sdk-js"))]
        WIN_W
    }

    /// 0 = 药丸，1 = 完整面板（展开时可略超过 1）。
    #[cfg(test)]
    pub(crate) fn current_width(&self) -> f32 { self.w.pos+self.notice_width.pos }

    pub fn progress(&self) -> f32 {
        let (bw, bh) = self.base_size();
        // 左右停靠时面板沿水平方向从边缘滑出，用宽度衡量展开程度。
        if self.side() {return (self.w.pos - bw) / (PW - bw).max(1.0);}
        (self.h.pos - bh) / (self.page_height.pos - bh).max(1.0)
    }

    pub fn cancel_page_scroll(&mut self) {
        self.page_scroll_area=None;
    }

    fn active_page_shortcut_colors(&self) -> Option<&std::collections::BTreeMap<String, [u8; 3]>> {
        #[cfg(feature = "sdk-js")]
        return self.plugins.active_page_shortcut_colors();
        #[cfg(not(feature = "sdk-js"))]
        { None }
    }

    fn page_shortcuts_enabled(&self) -> bool {
        self.settings.show_page_shortcuts
            && self.active_page_shortcut_colors().is_some()
            && self.navigation_pages().nth(1).is_some()
    }

    pub(crate) fn expanded_height(&self)->f32 {
        #[cfg(windows)]
        if self.plugin_notice_detail_open() { return crate::plugin_notice_ui::DETAIL_HEIGHT; }
        self.page_content_height() + if self.page_shortcuts_enabled() { PAGE_SHORTCUT_ROW_H } else { 0.0 }
    }

    fn page_content_height(&self)->f32 {
        #[cfg(feature="sdk-js")]
        {
            if let Some(index)=self.page.checked_sub(PAGE_NAMES.len()) {
                let page=self.status_pages.get(index);
                let height=page.map_or(
                    crate::plugin_status_ui::STATUS_PAGE_H, |(id,_)|self.plugins.status_height(id),
                );
                #[cfg(windows)]
                if page.is_some_and(|(id,_)|self.plugins.source_attribution(id).is_some()) {
                    return height+crate::plugin_status_ui::ATTRIBUTION_HEIGHT;
                }
                return height;
            }
            if self.page==TODO_PAGE && self.plugins.has_card(){return PH+46.0;}
        }
        if self.page==PERFORMANCE_PAGE {crate::performance_ui::PAGE_H}else if self.page==BLUETOOTH_PAGE {194.0}else if self.page==CALENDAR_PAGE {CALENDAR_H}else if self.page==WEATHER_PAGE {WEATHER_H}else if self.page==6{176.0}else if self.page==5&&self.mail.open_uid.is_some(){236.0}else{PH}
    }

    // ---------------------------------------------------------------
    // 每帧入口
    // ---------------------------------------------------------------
    pub fn frame(&mut self, ui: &mut egui::Ui, win: Rect, env: &Env) -> FrameOut {
        let ctx = ui.ctx().clone();
        #[cfg(feature="sdk-js")]
        {
            self.pet.expressions.set_sheets(self.plugins.appearance_pet());
            self.plugins.sync_canvas_fonts(&ctx);
        }
        #[cfg(all(windows, feature = "sdk-material"))]
        {
            self.material_appearance = if env.material_active { env.material_appearance } else { Default::default() };
        }
        let dt = match self.last_t {
            Some(t0) => (env.t - t0).clamp(0.0, 0.1) as f32,
            None => 0.0,
        };
        let elapsed = self.last_t.map(|at| (env.t - at).max(0.0) as f32).unwrap_or(0.0);
        self.last_t = Some(env.t);
        self.t = env.t;
        self.fullscreen_hidden=env.fullscreen_hidden;
        self.quota_tick();
        self.weather_tick();
        self.current_hour=env.hour();
        self.system_idle=env.idle_seconds;
        self.dt = dt;
        if let Some(at_top)=env.browser_at_top {self.browser_at_top=at_top;}
        self.screen=env.screen;
        if !self.side() { self.horizontal_width = win.width(); }
        if self.effective_skin().is_glass() {
            let target=if self.effective_skin()==model::Skin::GlassLight {glass::light_tint(env.backdrop,env.glass_blur)}else{glass::glass_tint(env.backdrop,env.glass_blur)};
            self.glass_tint=if self.glass_tint<=0.0 {target}else{anim::approach(self.glass_tint,target,3.0,dt)};
        }
        let browser_minimal=self.settings.browser_minimal && self.browser_at_top;
        if browser_minimal && !self.browser_minimal_active && !self.hold_open {
            self.open=false;
            self.hover_lock=true;
            self.hover_since=None;
            self.leave_since=None;
            self.forced_open_until=0.0;
        }
        self.browser_minimal_active=browser_minimal;
        if !self.started {
            self.started = true;
            self.page_height.snap(self.expanded_height());
            self.last_input = env.t;
            self.last_reminder = env.t;
            self.sit_start = env.t;
            self.page_t0 = env.t - 10.0;
            if self.page==CALENDAR_PAGE {self.request_calendar_month(false);}
        }
        let mut cmds = Vec::new();

        self.tick(env, dt, elapsed);
        #[cfg(feature="sdk-js")]
        if self.plugins.watch_files_enabled() { ctx.request_repaint_after(std::time::Duration::from_millis(200)); }
        self.sync_device(env);
        self.update_mail_code_alerts(env);
        self.tick_alerts();
        #[cfg(windows)]
        self.tick_plugin_notices(&ctx, env);
        #[cfg(windows)]
        let plugin_notice_detail = self.plugin_notice_detail_open();
        #[cfg(not(windows))]
        let plugin_notice_detail = false;
        let hud_active=self.notice.clone();
        if hud_active.as_ref().map(|a|&a.title)!=self.hud_notice.as_ref().map(|a|&a.title)||hud_active.is_none(){self.notice_details=false;}
        if let Some(a)=&hud_active {self.hud_notice=Some(a.clone());}
        self.hud_alpha=anim::approach(self.hud_alpha,if hud_active.is_some(){1.0}else{0.0},if hud_active.is_some(){16.0}else{8.0},dt);
        if hud_active.is_none() && self.hud_alpha<0.003 {self.hud_notice=None;}
        let notice_target=if !self.notice.as_ref().is_some_and(|a|a.messenger().is_some()) && self.compact_notices()&&(self.notice.is_some()||self.notification_bell().is_some()) {44.0}else{0.0};
        self.notice_width.target=notice_target;
        if self.compact_notices(){self.notice_width.step(dt);}else{self.notice_width.snap(0.0);self.notice_details=false;}
        self.update_media(env,&ctx);
        let bell=self.notification_bell().is_some()&&!self.compact_notices();
        self.resident_power=(env.battery,env.charging);
        let mut layout=Layout::new(self.settings.show_battery&&env.battery.is_some(),self.quota_bar_indices().len(),self.media.is_music()&&!bell,self.pomo.running,bell,self.pet_enabled(),self.weather_pill().is_some());
        let performance_bar=self.performance_on_bar();
        let performance_visible=!env.fullscreen_hidden && !env.screen_locked &&
            (if self.open {self.page==PERFORMANCE_PAGE} else {performance_bar && self.notice.is_none() && !self.island_lyrics_active()});
        let performance_detail=performance_visible && self.open && self.page==PERFORMANCE_PAGE;
        self.performance.tick(self.module_on(PERFORMANCE_PAGE),performance_visible,performance_detail,&ctx);
        // 鼠标移开收起再移回来，接着看刚才的详情；切到别的页面才回到设置里的起始页。
        if self.page!=PERFORMANCE_PAGE {self.performance.view=None;}
        if performance_bar {layout.add_performance(self.settings.performance_bar.width(self.settings.performance_metrics));}
        layout.reorder(&self.settings.resident_order);
        for (position,extra) in self.quota_balance_layout(&ctx){layout.balance_tail(position,extra);}
        let resident_day=if self.settings.resident_date==model::ResidentDate::Off {None}else{
            chrono::NaiveDate::parse_from_str(&env.today,"%Y-%m-%d").ok()
                .and_then(|today|ResidentDay::new(self.settings.resident_date,today))
        };
        if let Some(day)=resident_day.as_ref() {
            let clock=self.settings.show_clock.then(||self.settings.clock_text(&env.clock));
            layout.set_clock_width(day.group_width(&ctx,clock.as_deref()));
        }else {
            if self.settings.clock_period!=0 {layout.widen_clock(40.0);}
            if self.settings.clock_seconds {layout.widen_clock(24.0);}
        }
        if let Some(extra)=self.weather_pill_extra(&ctx){ layout.widen(bar::WEATHER,extra); }
        if resident_day.is_none() && !self.settings.show_clock {layout.hide_clock();}
        let wireless=self.wireless_resident();
        // 放不下整组时不挤压其他常驻项，整组隐藏。
        let wireless_w=self.wireless_bar_width();
        if !wireless.is_empty()&&wireless_w<=self.resident_limit()-layout.width-12.0 {layout.append_bluetooth(wireless_w);}
        #[cfg(feature="sdk-js")]
        {
            let suppress = self.effective_minimal() || self.notice.is_some() || self.hud_alpha > 0.003
                || plugin_notice_detail || self.island_lyrics_active() || env.fullscreen_hidden || env.screen_locked;
            let (mut displays, repaint) = self.plugins.pinned_displays(self.open || suppress);
            if let Some(delay) = repaint { ctx.request_repaint_after(delay); }
            for display in &mut displays { display.measure_width(ui); }
            let previous_pinned_width = self.pinned_horizontal_width;
            self.pinned_horizontal_width = WIN_W;
            if !self.side() {
                let widths: Vec<_> = displays.iter().map(|display| display.width(156.0)).collect();
                let pinned_width = if self.open && !suppress && self.render_compat_running
                    && self.placement_offset() == 0.0 && self.position_drag.is_none()
                    && self.drag_glide.is_none() && self.w.settled(PW) && self.h.settled(self.expanded_height()) {
                    // 先扩背景交换链，收起时再按原节奏加入常驻项。
                    self.plugins.pinned_reserved_width(ui)
                } else {
                    widths.iter().map(|width| width + 12.0).sum()
                };
                if pinned_width > 0.0 {
                    self.pinned_horizontal_width =
                        (layout.width + pinned_width + 32.0 + self.unread_stack_len(true)).max(WIN_W);
                    if self.open && self.pinned_horizontal_width > previous_pinned_width
                        && self.pinned_horizontal_width > self.horizontal_width {
                        ctx.request_repaint();
                    }
                }
                layout.append_pinned(&widths, self.resident_limit());
            }
            self.pinned_displays = displays;
        }
        if self.pet_enabled() {
            if let Some(a)=self.notice.as_ref().filter(|_|!self.compact_notices()&&!self.open){layout.x[bar::PET]=Some(22.0-(self.notice_size(a).0+self.pet_space())/2.0);}
            else if self.island_lyrics_active() {layout.x[bar::PET]=Some(60.0-self.lyric_bar_width()/2.0);}
        }
        let centered=!self.island_lyrics_active() && (self.notice.is_none() || self.open);
        self.resident.step(layout,dt,ctx.style().animation_time>0.0&&bar::animations_enabled(),centered);
        if self.opts.fetch_lyrics {
            self.poll_lyrics();
        }
        self.update_island_lyric_layout(&ctx);

        // 胶囊皮肤悬浮在屏幕顶边下方。
        let pill = self.settings.island_shape==model::IslandShape::Pill;
        let top = win.top() + if pill { PILL_GAP } else { 0.0 };
        let bottom=self.dock()==model::Dock::Bottom;
        let orient=|r:Rect| if bottom {
            let axis=win.top()+win.bottom();
            Rect::from_min_max(Pos2::new(r.left(),axis-r.bottom()),Pos2::new(r.right(),axis-r.top()))
        }else{r};
        let side = self.side();
        let dock = self.dock();

        // ---------- 输入：拖放 ----------
        let (hovering_files, dropped) = ctx.input(|i| {
            (
                self.settings.visible_pages[3] && !i.raw.hovered_files.is_empty(),
                i.raw.dropped_files.iter().filter_map(|f| f.path.clone()).collect::<Vec<_>>(),
            )
        });
        if !dropped.is_empty() && self.settings.visible_pages[3] {
            self.on_drop(dropped);
            self.show_page(3);
        } else if hovering_files && !self.drag_hot {
            self.drag_hot = true;
            self.drag_departing = false;
            if self.page != 3 {
                self.drag_saved = Some(self.page);
                self.go(3, if self.page < 3 { 1.0 } else { -1.0 });
            }
        } else if !hovering_files && self.drag_hot {
            self.drag_hot = false;
            self.drag_departing = true;
        }

        // ---------- 输入：悬停 ----------
        let p_prev = self.progress();
        let (offset,badge,notice)=self.bar_parts(win,self.island_rect(win,self.w.pos,self.h.pos));
        let cx=win.center().x+offset.x;
        let hit = if side {self.side_hit_rect(win,p_prev,self.page_height.pos).translate(offset)} else {orient(self.hit_rect(cx, top, p_prev))};
        let page_before=self.page;
        let previous_menu=self.shelf_menu_rect.take();
        let inside = env.cursor.is_some_and(|c| hit.contains(c) || previous_menu.is_some_and(|r|r.contains(c)));
        self.page_scroll_area=self.page_scroll_area.filter(|area|self.open && !self.hover_lock
            && !env.fullscreen_hidden && !env.screen_locked && env.cursor.is_some_and(|c|area.contains(c))
            && !ctx.input(|i|i.pointer.any_pressed() || i.key_pressed(egui::Key::Escape)));
        let page_hover=inside || self.page_scroll_area.is_some();
        // 展开时，穿过面板到通知图标之间的空隙也算悬停；按钮仍只在图标本身响应点击。
        let on_notice=env.cursor.is_some_and(|c|badge.into_iter().chain(notice).any(|mut r|{
            if self.open {
                r.extend_with_x(hit.x_range().clamp(r.center().x));
                if !side {r.extend_with_y(hit.top());}
            }
            r.contains(c)
        }) || self.notice_details_rect(notice).is_some_and(|r|r.contains(c)));
        if !inside {
            self.hover_lock = false;
        }
        if env.cursor != self.last_cursor || ctx.input(|i| !i.events.is_empty()) {
            if inside || hovering_files || ctx.input(|i| !i.events.is_empty()) {
                self.last_input = env.t;
            }
            self.last_cursor = env.cursor;
        }
        if self.music_volume_open && ctx.input_mut(|i|i.consume_key(egui::Modifiers::NONE,egui::Key::Escape)) {
            self.music_volume_open=false;
        }
        #[cfg(feature="sdk-js")]
        if self.plugin_form_focused && !ctx.viewport(|viewport|viewport.ime_owns_keys)
            && ctx.input_mut(|i|i.consume_key(egui::Modifiers::NONE,egui::Key::Escape))
        {
            ctx.memory_mut(|memory| {
                if let Some(id)=memory.focused() {memory.surrender_focus(id);}
            });
            self.plugin_form_focused=false;
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) && self.is_editing() {
            if self.calendar.draft.as_mut().is_none_or(|draft|draft.picker.take().is_none()) {
                self.cancel_edit();
            }
        }
        if !plugin_notice_detail && (!self.is_editing() || self.todo_choosing) && page_hover && self.open && self.reveal > 0.95 {
            let (scroll, left, right, esc) = ctx.input(|i| (i.raw_scroll_delta.y, i.key_pressed(egui::Key::ArrowLeft), i.key_pressed(egui::Key::ArrowRight), i.key_pressed(egui::Key::Escape)));
            if esc { self.hover_lock = true; }
            if (left || right) && !self.todo_choosing {
                self.wheel_delta=0.0;
                self.navigate_page(right);
            } else if scroll!=0.0 {
                let cursor=env.cursor.unwrap()-(self.content_origin(win)+offset).to_vec2();
                let mail_reading=inside && self.page==5 && self.mail.open_uid.is_some();
                let mail_list=self.page==5 && !self.mail.accounts.is_empty() && self.mail.open_uid.is_none() && self.mail_list_bounds().contains(cursor);
                let music_volume=self.page==0 && (Rect::from_min_max(Pos2::new(354.0,106.0),Pos2::new(380.0,130.0)).contains(cursor)
                    || self.music_volume_open && self.music_volume_bounds().contains(cursor));
                #[cfg(feature="sdk-js")]
                let status_scroll=self.plugin_canvas_frame.as_ref().is_some_and(|(page, frame)| {
                    *page==self.page && frame.canvas_scroll.is_some_and(|area|area.contains(env.cursor.unwrap()))
                }) || self.page.checked_sub(PAGE_NAMES.len())
                    .and_then(|index|self.status_pages.get(index))
                    .and_then(|(id,_)|{
                        let (_,status,health)=self.plugins.status_page(id)?;
                        if health!=crate::plugin_status::StatusHealth::Ready {return None;}
                        crate::plugin_status_ui::status_scroll_area(status?,self.plugins.status_layout(id)?)
                    }).is_some_and(|area|area.contains(cursor));
                #[cfg(not(feature="sdk-js"))]
                let status_scroll=false;
                if mail_reading || mail_list || music_volume || status_scroll {self.wheel_delta=0.0;}
                else {
                    // 累积高精度滚轮的小幅输入；停顿或反向时丢弃上一段余量。
                    if self.t-self.wheel_at>0.18 || self.wheel_delta*scroll<0.0 {self.wheel_delta=0.0;}
                    self.wheel_at=self.t;
                    self.wheel_delta+=scroll;
                    if self.wheel_delta.abs()>=8.0 {
                        let next=self.wheel_delta<0.0;
                        self.wheel_delta=0.0;
                        // 性能 CPU、内存详情的程序列表：滚轮在列表上只滚列表，滚到头也不翻页。
                        let performance_list=self.page==PERFORMANCE_PAGE && crate::performance_ui::list_area().contains(cursor);
                        let performance_max=if performance_list {self.performance_list_max()}else{0};
                        let clipboard_tab=self.page==3 && self.shelf_tab==1;
                        let clipboard_body=clipboard_tab && Rect::from_min_max(Pos2::new(42.0,31.0),Pos2::new(377.0,124.0)).contains(cursor);
                        let list=match self.page {
                            CALENDAR_PAGE=>self.calendar_scroll(cursor,env),
                            4 if Rect::from_min_max(Pos2::new(196.0,31.0),Pos2::new(377.0,124.0)).contains(cursor)
                                =>Some((&mut self.todo_first,self.todos.items.len().saturating_sub(3))),
                            3 if clipboard_body
                                =>Some((&mut self.clip_first,self.clipboard.lock().items.len().saturating_sub(crate::pages::CLIP_ROWS))),
                            3 if !clipboard_tab && Rect::from_min_max(Pos2::new(36.0,28.0),Pos2::new(383.0,128.0)).contains(cursor)
                                =>Some((&mut self.shelf_first,self.shelf.items.len().saturating_sub(5))),
                            PERFORMANCE_PAGE if performance_list && performance_max>0
                                =>Some((&mut self.performance.list_first,performance_max)),
                            _=>None,
                        };
                        let content_scrolls=list.as_ref().is_some_and(|(_,max)|*max>0);
                        let scrolled=list.is_some_and(|(first,max)|{
                            let target=if next {(*first+1).min(max)}else{first.saturating_sub(1)};
                            let changed=target!=*first;*first=target;changed
                        });
                        // 内容溢出时滚动列表并停在边界；未溢出时滚轮翻页。
                        if !scrolled && !content_scrolls && !self.todo_choosing && self.t-self.wheel_page_at>=0.08 {
                            self.navigate_page(next);
                            self.wheel_page_at=self.t;
                        }
                        ctx.input_mut(|i|{i.raw_scroll_delta=Vec2::ZERO;i.smooth_scroll_delta=Vec2::ZERO;});
                    }
                }
            }
        } else {self.wheel_delta=0.0;}
        let volume_dragging=self.music_volume_open && ctx.is_being_dragged(egui::Id::new(("card-drag",true,"m-volume-track")));
        let want = plugin_notice_detail || self.hold_open || (page_hover && !self.hover_lock && (self.compact_notices()||self.notice.is_none()||self.open)) || self.open && on_notice || (self.open && self.t<self.forced_open_until) || self.drag_hot || volume_dragging || (self.is_editing() && self.open);
        self.morph(want, inside, env.t, dt);
        let staying_open = self.open && want;
        if !self.drag_departing || staying_open {
            self.drag_a = anim::approach(self.drag_a, if self.drag_hot { 1.0 } else { 0.0 }, 16.0, dt);
        }
        if self.drag_departing && (staying_open && self.drag_a <= 0.004
            || !self.open && self.w.settled(PW) && self.h.settled(self.expanded_height())) {
            // 先收完外形再静默恢复页面；保持展开时则等放置框淡出。
            self.drag_departing = false;
            self.drag_a = 0.0;
            if let Some(page) = self.drag_saved.take() {
                self.go(page, if page < SHELF_PAGE { -1.0 } else { 1.0 });
                self.page_from = self.page;
                self.page_t0 = self.t - 1.0;
            }
        }
        self.hold_slot=ctx.animate_bool_with_time(egui::Id::new("hold-open-slot"),self.shows_hold_open(),0.18);

        // ---------- 绘制 ----------
        let p = self.progress();
        let (sx, sy) = self.drag_scale();
        // 拿起放大不能超出透明画布，否则两端会被切掉。
        let (max_w, max_h) = if side {(crate::side_dock::SIDE_W, crate::side_dock::SIDE_H)} else {(win.width(), WIN_H)};
        let (w, h) = ((self.w.pos * sx).min((max_w - 24.0).max(self.w.pos)), (self.h.pos * sy).min((max_h - 24.0).max(self.h.pos)));
        let panel = self.island_rect(win, w, h);
        let (offset,messenger_rect,notice_rect)=self.bar_parts(win,panel);
        let panel=panel.translate(offset);
        let cx=panel.center().x;
        self.update_pet(env,cx,panel.top());
        let appear=self.messenger_appear();
        let outside_badge=messenger_rect.filter(|_|appear>0.02);
        let pc = anim::clamp01(p);
        let painter = ui.painter_at(ui.max_rect().expand(40.0));
        let feather = 1.0 / ctx.pixels_per_point();
        let notice_shape=notice_rect.map(|r|Rect::from_min_max(Pos2::new(r.left()-36.0,r.top()),r.max));
        let body=messenger_rect.filter(|_|outside_badge.is_none()).map_or(panel,|badge|panel.union(badge));
        let body=notice_shape.map_or(body,|notice|body.union(notice));
        let minimal_strip = self.effective_minimal() && !self.island_lyrics_active() && self.notice.is_none() && self.notification_bell().is_none();
        let minimal_pill = minimal_strip && pc < 0.02;
        let flare = if minimal_pill || pill { 0.0 } else { lerp(5.0, FLARE, pc) };
        let k = (w / PW).clamp(0.3, 1.1);
        let thickness = if side { crate::side_dock::SIDE_BAR_W } else { BAR_H };
        let radius = if minimal_pill { BAR_H_MIN / 2.0 } else if pill { (w.min(h) * 0.5).min(lerp(thickness * 0.5, 26.0, pc)) } else { lerp(12.5, RAD * k.min(1.0), pc) };
        let flare_h = lerp(8.0, FLARE_H, pc);
        let thick = glass::thickness(PH);
        let glass_skin = self.effective_skin().is_glass() && !self.glassy();
        let layer = painter.layer_id();
        let palette = self.island_palette();
        // 换色时跳过的图形范围（岛体底色、阴影、吉祥物）。
        let mut keep_colors:Vec<(usize,usize)>=Vec::new();
        // 毛玻璃：真实模糊下保留柔和投影增加层次；无模糊时阴影再减淡一些，保持轻盈。
        let skin_shadow = if !self.effective_skin().is_glass() {1.0} else if env.glass_blur {0.75} else {0.7};
        #[cfg(all(windows, feature = "sdk-material"))]
        let skin_shadow = self.material_appearance.shadow.unwrap_or(skin_shadow);
        let shadow_k = glass::shadow_alpha(0.0, thick) / 0.12 * skin_shadow;
        let island_shadow=|r:Rect,flare:f32,flare_h:f32,radius:f32,strength:f32| if pill {pill_shadow(&painter,r,radius,strength)} else if dock!=model::Dock::Top {shape::shadow_docked(&painter,r,dock,flare,flare_h,radius,strength)} else {shape::shadow(&painter,r,flare,flare_h,radius,strength)};
        // 拿起时投影加深，像被从屏幕边缘提起来。
        let lifted = self.drag_lift.pos.clamp(0.0, 1.2);
        island_shadow(body, flare, flare_h, radius, (smoothstep(0.1, 0.9, pc) + lifted * 0.9) * shadow_k);
        if let Some(r)=outside_badge{
            let appear=self.messenger_appear();
            painter.rect_filled(r,r.width()/2.0,paint::alpha(self.island_overlay_fill(),appear));
            if glass_skin {painter.rect_stroke(r,r.width()/2.0,egui::Stroke::new(1.0_f32,paint::alpha(self.glass_rim(),appear)),egui::StrokeKind::Inside);}
        }
        if !self.glassy() && self.effective_skin()==model::Skin::Fluid {
            self.fluid_origin=body.min;
            let art=self.music_artwork.as_ref().map(|(_,texture)|texture.id());
            if art!=self.fluid_art || self.media.title!=self.fluid_title {
                let tint=if self.media.title.is_empty(){crate::fluid::Fluid::neutral()}
                    else if let Some((image,_))=&self.music_artwork{crate::fluid::Fluid::tint_from_image(image)}
                    else{crate::fluid::Fluid::tint_from_title(&self.media.title)};
                self.fluid.set_tint(tint);
                self.fluid_art=art;
                self.fluid_title.clone_from(&self.media.title);
            }
            self.fluid.set_cursor(env.cursor.filter(|p|body.contains(*p)).map(|p|(p.x-body.left(),p.y-body.top())));
            if !self.open || self.page!=0 || self.media.title.is_empty(){self.fluid.set_seek(None,0.0);}
            self.fluid.advance(env.t,dt);
        }
        if !self.glassy() { self.paint_body(&painter, body, flare, flare_h, radius, feather); }
        #[cfg(all(windows, feature = "sdk-material"))]
        let material_layer = env.material_active.then(|| {
            let radius=if pill {radius.min(body.width().min(body.height())*0.5)} else {radius};
            let notch=(!pill).then_some((flare,flare_h));
            let fallback = if self.glassy() {
                egui::Shape::Rect(egui::epaint::RectShape::new(
                    body, radius, self.material_glass_fill(225),
                    egui::Stroke::new(1.0, Color32::from_white_alpha(80)), egui::StrokeKind::Inside,
                ))
            } else { egui::Shape::Noop };
            crate::material::Layer::reserve(&painter, ui.max_rect(), body, radius, pc, notch, dock, self.music_artwork.as_ref(), fallback)
        });
        if lifted > 0.01 {self.paint_lift_halo(&painter, body, flare, flare_h, radius, lifted.min(1.0));}
        let arrival=1.0-smoothstep(0.0,0.12,(self.t-self.crossed_at) as f32);
        if self.seam_guide>0.01 || arrival>0.0 {
            let (center,along)=match dock {
                model::Dock::Left=>(body.left_center()+Vec2::X*2.0,Vec2::Y),
                model::Dock::Right=>(body.right_center()-Vec2::X*2.0,Vec2::Y),
                model::Dock::Bottom=>(body.center_bottom()-Vec2::Y*2.0,Vec2::X),
                model::Dock::Top=>(body.center_top()+Vec2::Y*2.0,Vec2::X),
            };
            let strength=self.seam_guide.max(arrival);
            let length=lerp(6.0,20.0,strength)*(1.0-0.45*arrival);
            painter.line_segment([center-along*length,center+along*length],egui::Stroke::new(2.5,Color32::from_white_alpha((210.0*strength) as u8)));
        }
        let center_guide=self.paint_center_rim(&painter, body, panel, flare, flare_h, radius, env.screen);
        let content_from=crate::skin_paint::shape_count(&ctx,layer);

        // 内容只随形体「揭开」：不缩放，按面板裁剪，到 55% 才开始淡入
        let reveal = if self.open { smoothstep(0.50, 0.97, p) } else { smoothstep(0.55, 0.92, p) };
        self.reveal = reveal;
        let mut origin = self.content_origin(win)+offset;
        if bottom {origin.y=panel.top();}
        let clip = panel.intersect(win);
        let base = Card::new(ui, origin, clip);
        self.cursor_local = env.cursor.map(|p| base.to_local(p));

        let mut top_bar_notice=None;
        let banner=!self.compact_notices()&&self.notice.is_some()&&!self.open;
        let restoring_lyrics=!self.open&&self.notice.is_none()&&self.island_lyrics_active()&&self.hud_notice.as_ref().is_some_and(|a|a.meeting.is_some()||a.source!=crate::extras::AlertSource::Internal);
        let fading_hud=!self.compact_notices()&&!self.open&&self.hud_notice.as_ref().is_some_and(|a|a.is_hud())&&(banner||self.hud_alpha>0.003);
        if fading_hud {
            let from=self.resident.target.width;let to=HUD_W+self.pet_space();
            let width_progress=if (to-from).abs()<1.0 {if banner{1.0}else{0.0}}else{((self.w.pos-from)/(to-from)).clamp(0.0,1.0)};
            let (opacity,header_opacity)=if banner {(self.hud_alpha*smoothstep(0.45,0.85,width_progress),1.0-smoothstep(0.05,0.40,width_progress))}
                else {(self.hud_alpha*smoothstep(0.20,0.60,width_progress),1.0-smoothstep(0.05,0.25,width_progress))};
            if !minimal_pill||pc>0.0 {self.resident_header(&base.faded(header_opacity),panel,cx,pc,reveal,env,resident_day.as_ref());}
            if let Some(a)=self.hud_notice.clone(){
                let clip=panel.intersect(ui.max_rect());
                let interactive=opacity>=0.85 && banner;
                let overlay=Card::new(ui,panel.min+Vec2::new(self.pet_space(),0.0),clip).faded(opacity);
                let overlay=if interactive {overlay}else{overlay.ghost()};
                if interactive && a.title=="系统音量" {top_bar_notice=Some(clip);}
                self.draw_notice(&overlay,panel,&a,&mut cmds);
            }
        }else if banner {
            if let Some(a)=self.notice.clone(){
                let clip=panel.intersect(ui.max_rect());
                top_bar_notice=Some(clip);
                self.draw_notice(&Card::new(ui,panel.min+Vec2::new(self.pet_space(),0.0),clip),panel,&a,&mut cmds);
            }
        }else if !plugin_notice_detail && (!minimal_pill || pc > 0.0) {
            self.resident_header(&base, panel, cx, if restoring_lyrics{0.0}else{pc}, reveal, env, resident_day.as_ref());
        }
        if let Some(rect)=notice_rect {
            let c=Card::new(ui,rect.min,rect.intersect(ui.max_rect()));
            if let Some(a)=self.hud_notice.clone() {
                let opacity=self.hud_alpha*smoothstep(0.45,0.95,self.notice_width.pos/44.0);
                let interactive=self.notice.is_some()&&opacity>=0.85;
                let c=c.faded(opacity);let c=if interactive {c}else{c.ghost()};
                if a.source!=crate::extras::AlertSource::Internal {
                    paint::bell(&c.scaled(18.0,15.0,0.85),18.0,15.0,theme::WHITE,paint::bell_swing(self.t-a.at));
                    c.circle(22.0,10.5,2.0,Color32::from_rgb(255,72,72));
                }else{crate::extras::notice_badge(&c,18.0,15.0,&a);}
                let button=crate::quota_ui::bar_tooltip(c.hit(0.0,0.0,36.0,BAR_H,"notice-badge"),format!("{}\n{}",a.title,a.body));
                if interactive {top_bar_notice=Some(button.interact_rect);}
                if button.clicked(){if let Some(kind)=a.health(){self.confirm_health_reminder(kind);}else{self.notice_details=!self.notice_details;}}
            }else if let Some(at)=self.notification_bell(){
                paint::bell(&c.scaled(20.0,14.0,1.2),20.0,14.0,theme::WHITE,paint::bell_swing(self.t-at));
                c.circle(24.0,9.5,2.0,Color32::from_rgb(255,72,72));
                if self.module_on(MAIL_PAGE) {
                    let button=c.hit(6.0,0.0,28.0,30.0,"bell-mail");
                    top_bar_notice=Some(button.interact_rect);
                    if button.clicked(){self.show_page(MAIL_PAGE);}
                }
            }
        }
        if let Some(rect)=messenger_rect {
            let from=crate::skin_paint::shape_count(&ctx,layer);
            let c=Card::new(ui,rect.min,rect.intersect(ui.max_rect()));
            if outside_badge.is_some() {
                let c=c.scaled(0.0,0.0,appear);
                self.draw_unread_capsule(&ctx,&c,rect,env.cursor,&mut cmds);
            }else{
                let horizontal=!(self.side() && self.notice.is_none());
                let scale=self.messenger_stack_appear();
                self.draw_unread_stack(&c.scaled(0.0,0.0,scale).faded(scale),horizontal,&mut cmds);
            }
            keep_colors.push((from,crate::skin_paint::shape_count(&ctx,layer)));
        }
        let notice_details=self.notice_details_rect(notice_rect);
        if let Some(rect)=notice_details {
            let radius=if self.hud_notice.as_ref().is_some_and(|a|a.meeting.is_some()){14.0}else{10.0};
            let from=crate::skin_paint::shape_count(&ctx,layer);
            shape::shadow(&painter,rect,0.0,0.0,radius,0.5*shadow_k);
            painter.rect_filled(rect,radius,self.island_overlay_fill());
            if glass_skin {
                painter.rect_stroke(rect,radius,egui::Stroke::new(1.0_f32,self.glass_rim()),egui::StrokeKind::Inside);
            }
            keep_colors.push((from,crate::skin_paint::shape_count(&ctx,layer)));
            self.draw_notice(&Card::new(ui,rect.min,rect.intersect(ui.max_rect())),rect,&self.hud_notice.clone().unwrap(),&mut cmds);
            if ctx.input(|i|i.pointer.any_pressed()&&i.pointer.interact_pos().is_some_and(|p|!rect.contains(p)&&!notice_rect.is_some_and(|r|r.contains(p)))){self.notice_details=false;}
        }
        let pet_alpha=self.pet_a*(1.0-smoothstep(0.0,0.3,pc));
        if pet_alpha>0.004 && !minimal_pill {
            let y=(h.min(self.base_size().1)-BAR_H).max(0.0)/2.0;
            // 吉祥物跟随皮肤换色：浅色底上变成深色版本，保持清晰。
            let fit=if self.notice.is_some(){1.0}else{self.resident_fit()};
            self.pet.draw(&base.scaled(cx-origin.x,15.0,fit).shifted(0.0,y).faded(pet_alpha),cx-origin.x+self.resident.position(bar::PET),self.t,palette);
        }
        let previous_scroll=self.content_scroll.take();
        let mut page_transition=false;
        #[cfg(feature="sdk-js")]
        {self.plugin_form_focused=false;self.plugin_canvas_frame=None;}
        if plugin_notice_detail {
            #[cfg(windows)]
            if self.open && reveal > 0.6 {
                self.draw_plugin_notice_detail(&base, &mut cmds);
            }
        } else if reveal > 0.004 && !banner && !restoring_lyrics {
            let c = base.shifted(0.0, (1.0 - reveal) * -5.0).faded(reveal);
            let c = if reveal < 0.6 { c.ghost() } else { c };
            if self.drag_departing {
                self.drag_overlay(&c.ghost().faded(self.drag_a));
            } else if self.calendar.draft.is_some(){self.draw_calendar_editor(&c);}else if self.task_dialog.is_some(){self.draw_task_editor(&c);}else{page_transition=self.panel_body(&c, env, &mut cmds);}
            self.draw_hold_open(&c);
            if !self.toast_below {self.toast_layer(&c,106.0);}
        }
        // 岛下的确认提示使用独立裁剪区域，收起时跟随岛体下沿。
        let floating_toast=if self.toast_below && self.toast.is_some() {
            // 侧边停靠：提示出现在胶囊正下方，靠边时向内挪，保证完整显示。
            let toast_origin=if side {
                let center=if dock==model::Dock::Left {panel.center().x.max(win.left()+90.0)} else {panel.center().x.min(win.right()-90.0)};
                Pos2::new(center-PW/2.0,panel.top())
            } else {origin};
            self.toast_layer(&Card::new(ui,toast_origin,win),if bottom {-36.0}else{h+8.0})
        }else{None};
        if !plugin_notice_detail && (self.adding||self.editing.is_some()) && reveal > 0.9 && self.page == 4 && self.page_e() > 0.25 {
            self.input_field(ui, origin, &mut cmds);
        }

        if !plugin_notice_detail && self.task_dialog.is_some() && reveal>0.9 {self.task_editor_inputs(ui,origin,&mut cmds);}
        if !plugin_notice_detail && self.calendar.draft.is_some() && reveal>0.9 {self.calendar_editor_inputs(ui,origin,&mut cmds);}

        // ---------- 输出 ----------
        let rel = |r: Rect| r.translate(-win.min.to_vec2());
        let mut region = if side {
            // 侧边：面板从边缘滑出，区域包含肩部（沿边缘上下）与外侧阴影，并一直延伸到屏幕边缘。
            let r=if p > 0.02 || self.open {
                let height=self.expanded_height().max(self.h.pos)*1.08+FLARE*2.0+24.0;
                self.island_rect(win,PW+20.0,height).union(panel.expand(20.0))
            } else {
                let (bw,bh)=self.base_size();
                self.island_rect(win,bw*SWELL.1+14.0,bh*SWELL.0+FLARE+16.0).union(panel.expand(12.0))
            };
            if dock==model::Dock::Left {Rect::from_min_max(Pos2::new(win.left(),r.top()),r.max)} else {Rect::from_min_max(r.min,Pos2::new(win.right(),r.bottom()))}
        } else if p > 0.02 || self.open {
            let height=self.expanded_height().max(self.h.pos)*1.08+16.0;
            // 常驻条比展开面板宽时（比如开了综合监测），展开开头和收起末尾岛还比面板宽；
            // 窗口区域按当前岛宽取大，否则两侧会被系统按面板宽度直接切掉。
            Rect::from_center_size(Pos2::new(cx, top + height/2.0), Vec2::new(PW.max(self.w.pos) + FLARE * 2.0 + 56.0, height))
        } else {
            let (bw, bh) = self.base_size();
            Rect::from_center_size(Pos2::new(cx, top + (bh * SWELL.1 + 12.0) / 2.0), Vec2::new(bw * SWELL.0 + 24.0, bh * SWELL.1 + 12.0))
        };
        region=orient(region);
        // 拿起放大、甩动拉伸以及加深的投影都要留在窗口区域内，否则两端会被系统裁掉。
        if self.drag_lift.pos.abs()>0.005 || self.drag_stretch.pos.abs()>0.001 {region=region.union(panel.expand2(Vec2::new(18.0,22.0)));}
        if let Some(r)=center_guide {region=region.union(r);}
        if let Some(r)=messenger_rect{region=region.union(r.expand(5.0));}
        if let Some(rect)=notice_shape {region=region.union(rect.expand2(Vec2::new(12.0,8.0)));}
        if let Some(rect)=notice_details {region=region.union(rect.expand(12.0));}
        // 胶囊上方留空的那一小条也要在窗口区域内，阴影才不会被截断。
        if pill && bottom {region.max.y=win.bottom();} else if pill && !side {region.min.y=region.min.y.min(win.top());}
        // 浅色皮肤：把这一帧岛体内容统一换成皮肤配色。
        if let Some(palette)=palette {
            let end=crate::skin_paint::shape_count(&ctx,layer);
            let mut ranges=Vec::new();let mut at=content_from;
            keep_colors.sort();
            for (from,to) in keep_colors {if from>at {ranges.push((at,from));} at=at.max(to);}
            if end>at {ranges.push((at,end));}
            crate::skin_paint::recolor_layer(&ctx,layer,&ranges,&palette);
        }
        // 原生裁剪还需包含悬停提示；只扩大绘制区域，保持桌面点击穿透。
        let mut tooltips=Vec::new();
        ctx.memory(|memory|{
            for layer in memory.areas().visible_layer_ids() {
                if layer.order==egui::Order::Tooltip {
                    if let Some(rect)=memory.area_rect(layer.id){region=region.union(rect.expand(12.0));tooltips.push(rect);}
                }
            }
        });
        // 原生模糊与窗口裁剪共用岛体和浮层的绘制轮廓。
        let outlines=||{
            let outline=|r:Rect,flare:f32,flare_h:f32,radius:f32| if pill {GlassShape::Round(rel(r),radius.min(r.height().min(r.width())*0.5))}
                else if dock!=model::Dock::Top {GlassShape::Docked{rect:rel(r),dock,flare,flare_h,radius}} else {GlassShape::Notch{rect:rel(r),flare,flare_h,radius}};
            let mut shapes=vec![outline(body,flare,flare_h,radius)];
            if let Some(r)=outside_badge{shapes.push(GlassShape::Round(rel(r),r.width()/2.0));}
            if let Some(r)=notice_details {
                let radius=if self.hud_notice.as_ref().is_some_and(|a|a.meeting.is_some()){14.0}else{10.0};
                shapes.push(GlassShape::Round(rel(r),radius));
            }
            if let Some(r)=floating_toast {shapes.push(GlassShape::Round(rel(r),11.0));}
            shapes.extend(tooltips.iter().map(|r|GlassShape::Round(rel(*r),6.0)));
            shapes
        };
        // 常驻条两端的动画留白不能盖住相邻窗口按钮。
        let resident_clip=(!side && !self.open && !minimal_strip && pc<=0.1 && lifted<0.001 && self.drag_stretch.pos.abs()<=0.001 && !self.drag_hot
            && self.w.settled(PW) && self.h.settled(self.expanded_height()) && center_guide.is_none() && self.shelf_menu_rect.is_none()).then(&outlines);
        let glass_region=(glass_skin && self.settings.glass_blur || env.opaque_surface)
            .then(||resident_clip.clone().unwrap_or_else(outlines));
        self.update_position_drag(&ctx,env,dt);
        let water_view_transition = self.open && self.page == WATER_PAGE && self.subpage.is_none() && !self.sit_mode
            // 点击在本帧推进弹簧之后处理，先识别尚未同步的目标。
            && (self.water_view_t.target != self.water_view as f32 || !self.water_view_t.settled(1.0));
        let scrolling=previous_scroll.zip(self.content_scroll).is_some_and(|((old_id,old_y),(id,y))| old_id==id && old_y!=y);
        let resizing=!self.w.settled(PW) || !self.h.settled(self.expanded_height());
        let shelf_scrolling=self.open && self.page==3 && self.shelf_tab==0 && !self.shelf.items.is_empty()
            && (self.shelf_scroll.target!=self.shelf_first as f32 || !self.shelf_scroll.settled(1.0)
                || self.shelf_x.values().any(|s|!s.settled(68.0)));
        let pet_fading=self.pet_a>0.001 && self.pet_a<0.999;
        let resident_moving=!self.resident.settled();
        let closing_tail=!self.open && (resident_moving || (!minimal_pill && pet_fading));
        let interactive_transition=closing_tail || (self.hold_slot>0.0 && self.hold_slot<1.0) || resizing || page_transition || scrolling || shelf_scrolling || water_view_transition;
        let transitioning = interactive_transition || self.position_drag.is_some() || self.drag_glide.is_some() || !self.drag_lift.settled(1.0) || !self.drag_stretch.settled(1.0)
            || self.center_guide>0.002 || self.center_lock_a>0.002 || self.t-self.center_lock_at<CENTER_SWEEP
            || self.page_e() < 0.7
            || self.dots.iter().any(|d| !d.settled(16.0))
            || self.hover_since.is_some() && !self.open
            || self.leave_since.is_some()
            || (self.drag_a > 0.001 && self.drag_a < 0.999)
            || self.drag_hot
            || pet_fading || resident_moving
            || self.open && self.page==2 && (!self.water_ring.settled(1.0) || !self.sit_view_t.settled(1.0) || !self.sit_t.settled(1.0))
            || self.toast.is_some()
            || self.notification_bell().is_some_and(|at|self.t-at<0.95)
            || self.notice.as_ref().is_some_and(|a|a.source!=crate::extras::AlertSource::Internal&&self.t-a.at<0.95)
            || self.hud_alpha > 0.003 && self.hud_alpha < 0.997
            || !self.notice_width.settled(1.0);
        let pet_visible=self.pet_a>0.01 && !self.open && !minimal_pill;
        let ambient=!self.glassy() && self.effective_skin()==model::Skin::Fluid && self.fluid.animating() || self.pomo.running && (self.open && self.page==1 || !self.open && !minimal_pill && self.notice.is_none() && bar::animations_enabled())
            || self.media.playing && (self.open && self.page==0 || !self.open && !minimal_pill && self.notice.is_none() && self.media.is_music())
            || self.notice.as_ref().is_some_and(|notice|notice.meeting.is_some())
            || self.wireless_animating();
        #[cfg(feature="sdk-js")]
        let (canvas_animating, canvas_scroll, canvas_repaint) = self.plugin_canvas_frame.as_ref().map_or(
            (false,None,None),
            |(_, frame)| (frame.canvas_animating,frame.canvas_scroll,frame.canvas_repaint),
        );
        #[cfg(not(feature="sdk-js"))]
        let (canvas_animating, canvas_scroll, canvas_repaint) = (false,None,None);
        let animating=transitioning || ambient || canvas_animating || pet_visible && self.pet.state!=PetState::Sleeping;
        // 交互形变保持 60 帧，媒体 30 帧；小表情 20 帧，睡眠 10 帧，静态界面每秒核对提醒。
        let repaint_after=std::time::Duration::from_secs_f64(if transitioning{1.0/60.0}else if ambient{1.0/30.0}else if pet_visible{if self.pet.state==PetState::Sleeping{0.1}else{0.05}}else{1.0})
            .min(canvas_repaint.unwrap_or(std::time::Duration::MAX));
        cmds.append(&mut self.pending);
        let next_hit=if side {self.side_hit_rect(win,self.progress(),self.page_height.pos).translate(offset)} else {orient(self.hit_rect(cx,top,self.progress()))};
        #[cfg(feature="sdk-js")]
        if page_before!=self.page || !self.open || self.page<PAGE_NAMES.len()
            || self.status_values_blocked(env) {
            crate::plugin_status_ui::clear_sensitive_reveal(&ctx);
        }
        if page_before!=self.page && self.open {
            let previous=self.page_scroll_area.map_or(hit,|area|hit.union(area));
            // 滚轮续翻的空白带按最终页边界计算，点击区域则跟随当前画面。
            let next_hit=if side {self.side_hit_rect(win,self.progress(),self.expanded_height()).translate(offset)}else{next_hit};
            // 保留缩短一侧原有的整段宽度。和新页重叠一点，避免在边缘来回挪动时误收起。
            self.page_scroll_area=env.cursor.filter(|&c|previous.contains(c)).and_then(|c|{
                let mut band=previous;
                if previous.bottom()>next_hit.bottom() && c.y>=next_hit.bottom()-PAGE_EDGE_OVERLAP {
                    band.min.y=next_hit.bottom()-PAGE_EDGE_OVERLAP;
                } else if previous.top()<next_hit.top() && c.y<=next_hit.top()+PAGE_EDGE_OVERLAP {
                    band.max.y=next_hit.top()+PAGE_EDGE_OVERLAP;
                } else {
                    return None;
                }
                Some(band)
            });
        }
        let geometry_transitioning = resizing
            || !self.notice_width.settled(1.0) || !self.resident.settled()
            || !self.drag_lift.settled(1.0) || !self.drag_stretch.settled(1.0)
            || self.position_drag.is_some() || self.drag_glide.is_some() || self.drag_hot
            || self.hover_since.is_some() && !self.open || self.leave_since.is_some() || page_transition;
        FrameOut {
            geometry_transitioning,
            region: rel(floating_toast.into_iter().chain(self.shelf_menu_rect.map(|r|r.union(r + ctx.style().visuals.popup_shadow.margin()).expand(1.0 / ctx.pixels_per_point()))).fold(region,Rect::union).intersect(ui.max_rect())),
            glass: glass_region,
            blur: glass_skin && self.settings.glass_blur,
            #[cfg(all(windows, feature = "sdk-material"))]
            material_layer,
            resident_clip,
            hit: rel(next_hit),
            page_scroll_area:self.page_scroll_area.or(canvas_scroll).map(rel),
            click_through: minimal_strip && !self.open && self.position_drag.is_none(),
            notice_hit:notice_shape.into_iter().chain(notice_details).chain(messenger_rect).chain(self.shelf_menu_rect).reduce(|a,b|a.union(b)).map(rel),
            top_bar_notice:top_bar_notice.into_iter().chain(notice_details.map(|r|r.intersect(ui.max_rect()))).filter(|r|r.is_positive()).reduce(|a,b|a.union(b)).map(rel),
            file_drop_zone:self.settings.visible_pages[3].then(||rel(if side {self.island_rect(win,PW,self.expanded_height()).translate(offset)} else {orient(Rect::from_min_size(Pos2::new(cx-PW/2.0,top),Vec2::new(PW,96.0)))})),
            expanded: self.open,
            editing: self.is_editing(),
            animating,
            page_transition,
            water_view_transition,
            interactive_transition:interactive_transition || canvas_animating,
            repaint_after,
            cmds,
        }
    }

    fn hit_rect(&self, cx: f32, top: f32, p: f32) -> Rect {
        let r=self.hit_rect_base(cx,top,p);
        // 胶囊离屏幕顶边有一段空隙：把光标推到屏幕最顶端也要能唤出岛。
        if self.settings.island_shape==model::IslandShape::Pill {Rect::from_min_max(Pos2::new(r.left(),r.top()-PILL_GAP),r.max)} else {r}
    }

    fn hit_rect_base(&self, cx: f32, top: f32, p: f32) -> Rect {
        if self.effective_minimal() && !self.open {
            let (width,height)=self.base_size();
            let width=self.w.pos.min(width);
            let height=self.h.pos.min(height);
            Rect::from_min_size(Pos2::new(cx-width/2.0,top),Vec2::new(width,height))
        } else if self.notice.is_some()&&!self.compact_notices()&&!self.open {
            let (w,h)=self.base_size();Rect::from_min_size(Pos2::new(cx-(w+8.0)/2.0,top),Vec2::new(w+8.0,h+6.0))
        } else if p > 0.04 || self.open {
            Rect::from_min_size(Pos2::new(cx - PW / 2.0 - 4.0, top), Vec2::new(PW + 8.0, self.expanded_height() + 2.0))
        } else {
            let (bw,bh)=self.base_size();
            let (hit_w,hit_h)=(self.w.pos.min(bw),bh+4.0);
            Rect::from_min_size(Pos2::new(cx-hit_w/2.0,top),Vec2::new(hit_w,hit_h))
        }
    }

    /// 展开后未读胶囊的出现程度：面板基本展开后从面板旁淡入。
    pub(crate) fn messenger_appear(&self)->f32 {
        if self.calendar.draft.is_some() || self.task_dialog.is_some(){return 0.0;}
        smoothstep(0.0,1.0,anim::clamp01((self.progress()-0.7)/0.3))
    }
    fn messenger_stack_appear(&self)->f32 {1.0-smoothstep(0.04,0.30,self.progress())}
    /// 同一来源有多个账号时给图标编号（1、2…）；只有一个账号不编号。
    pub(crate) fn unread_tag(&self,i:usize)->Option<usize>{
        let unread=self.pending_messengers.get(i)?;let source=unread.source;
        if unread.unidentified(){return None;}
        let known=|u:&&Unread|u.source==source && (!source.adapter().keep_unidentified || u.pid!=0);
        let same=self.pending_messengers.iter().filter(known).count();
        (same>1).then(||self.pending_messengers[..=i].iter().filter(known).count())
    }
    fn unread_id(&self,i:usize)->String{
        let name=self.pending_messengers[i].source.name();
        let n=self.pending_messengers[..=i].iter().filter(|u|u.source==self.pending_messengers[i].source).count();
        if n>1{format!("{name}{n}")}else{name.to_string()}
    }
    /// 展开胶囊高度：一个是圆，两个是短胶囊，三个及以上与面板等高（多出的滚动）。
    fn unread_capsule_height(&self,panel_h:f32)->f32 {
        let n=self.unread_count();
        if n>=3 {panel_h} else {4.0+UNREAD_SLOT*n as f32}
    }
    pub(crate) fn unread_top_inset(&self)->f32 {if self.dock()==model::Dock::Top {UNREAD_GAP}else{0.0}}
    fn unread_slot(&self,height:f32)->f32 {
        if self.unread_count()>=3 {(height-4.0)/3.0} else {UNREAD_SLOT}
    }
    fn unread_count(&self)->usize {
        let count=self.pending_messengers.len()+self.health_badges().len();
        #[cfg(windows)]
        let count=count+self.plugin_notice_ui.item_count();
        count
    }
    /// 收起时叠放的来源图标，每个客户端各占一个。
    fn unread_sources(&self)->Vec<Messenger>{
        let mut sources=Vec::new();
        for u in &self.pending_messengers {if !sources.contains(&u.source){sources.push(u.source);}}
        sources
    }
    /// 收起时叠放区沿胶囊方向的长度。
    fn unread_stack_len(&self,horizontal:bool)->f32 {
        let icons=self.unread_sources().len()+self.health_badges().len();
        #[cfg(windows)]
        let icons=icons+self.plugin_notice_ui.collapsed_count();
        let icons=icons as f32;
        if icons==0.0 {0.0}else{32.0+unread_step(horizontal)*(icons-1.0)}
    }

    pub(crate) fn resident_limit(&self)->f32 {
        let limit = if self.side() {
            crate::side_dock::SIDE_H - 24.0
        } else {
            self.screen.map_or(self.horizontal_width, |screen| screen.width().min(self.horizontal_width)) - 32.0
        };
        (limit - self.unread_stack_len(!self.side())).max(1.0)
    }

    /// 并入岛体的图标与主内容共用停靠中线；展开后独立显示的胶囊自行定位。
    fn bar_parts(&self,win:Rect,panel:Rect)->(Vec2,Option<Rect>,Option<Rect>) {
        let badge=self.messenger_badge_rect(win,panel);
        let notice=self.notice_rect(panel);
        let merged=badge.filter(|_|self.messenger_appear()<=0.02).into_iter().chain(notice);
        let body=merged.fold(panel,Rect::union);
        let bounds=self.screen.map_or(win,|screen|screen.intersect(win));
        let along=|center:f32,mid:f32,low:f32,high:f32|{
            let shift=center-mid;
            if low<=high {shift.clamp(low,high)}else{shift}
        };
        let offset=if body==panel {Vec2::ZERO} else if self.side(){
            Vec2::new(0.0,along(win.center().y,body.center().y,bounds.top()-body.top(),bounds.bottom()-body.bottom()))
        }else{
            Vec2::new(along(win.center().x,body.center().x,bounds.left()-body.left(),bounds.right()-body.right()),0.0)
        };
        (offset,badge.map(|r|r.translate(offset)),notice.map(|r|r.translate(offset)))
    }

    fn messenger_badge_rect(&self,win:Rect,panel:Rect)->Option<Rect>{
        if self.unread_count()==0 || self.browser_minimal_active && !self.open {return None;}
        // 完整横幅已包含该来源图标。歌词模式和横幅收起后使用常驻入口。
        if !self.open && !self.compact_notices() && self.unread_count()==1 && self.pending_messengers.len()==1
            && self.notice.as_ref().and_then(|a|a.messenger())==self.pending_messengers.first().map(|u|u.source){return None;}
        let bounds=self.screen.map_or(win,|screen|screen.intersect(win));
        let appear=self.messenger_appear();
        if appear>0.02 {
            // 展开：贴在面板外侧、顶端对齐的竖向胶囊；顶部停靠和左停靠在右边，右停靠在左边。
            let full=self.island_rect(win,PW,self.page_height.pos);
            // 顶部停靠时面板贴着屏幕上沿，胶囊往下让一点，圆顶不会被屏幕边切掉。
            let inset=self.unread_top_inset();
            let height=self.unread_capsule_height(full.height()-inset);
            let left=self.dock()==model::Dock::Right || !self.side() && full.right()+UNREAD_GAP+UNREAD_W>bounds.right();
            // 胶囊在展开后的中心原地显现，避免跟着岛体边缘越过终点再弹回来。
            let cx=if left {full.left()-UNREAD_GAP-UNREAD_W/2.0}else{full.right()+UNREAD_GAP+UNREAD_W/2.0};
            let center=Pos2::new(cx,full.top()+inset+height/2.0);
            return Some(Rect::from_center_size(center,Vec2::new(UNREAD_W,height)*appear));
        }
        // 极简刘海：横幅照常弹出，收起后只留细线，未读不常挂在线条上；展开岛体时在面板旁出现。
        if self.effective_minimal(){return None;}
        let scale=self.messenger_stack_appear();
        if scale<0.02 || self.open && (self.calendar.draft.is_some() || self.task_dialog.is_some()){return None;}
        let side_column=self.side() && self.notice.is_none();
        let length=self.unread_stack_len(!side_column);
        let (width,height)=if side_column {(32.0,length)}else{(length,BAR_H)};
        let (width,height)=(width*scale,height*scale);
        let (x,y)=if side_column {
            (panel.center().x-width/2.0,panel.bottom())
        }else{
            let right=panel.right()+self.notice_width.pos;
            let y=if self.notice.is_some() && !self.compact_notices(){panel.center().y-BAR_H/2.0}else{panel.top()};
            (if !self.side() || right+width<=bounds.right(){right}else{panel.left()-width},y)
        };
        Some(Rect::from_min_size(Pos2::new(x,y),Vec2::new(width,height)))
    }

    /// 展开后的未读胶囊：每个账号一个图标，点击跳到对应窗口；超过三个时滚轮翻看。
    fn draw_unread_capsule(&mut self,ctx:&egui::Context,c:&Card,rect:Rect,cursor:Option<Pos2>,cmds:&mut Vec<Cmd>){
        let n=self.unread_count();
        let height=rect.height()/c.s;
        let slot=self.unread_slot(height);
        let max_scroll=(n as f32-3.0).max(0.0)*slot;
        if max_scroll>0.0 && cursor.is_some_and(|p|rect.contains(p)) {
            let delta=ctx.input(|i|i.raw_scroll_delta.y);
            self.messenger_scroll-=delta*0.5;
        }
        self.messenger_scroll=self.messenger_scroll.clamp(0.0,max_scroll);
        let c=c.faded(self.messenger_appear());
        let inner=c.clipped(0.0,2.0,UNREAD_W,height-4.0);
        let x=UNREAD_W/2.0;
        for i in 0..self.pending_messengers.len() {
            let u=self.pending_messengers[i];
            let cy=2.0+slot*(i as f32+0.5)-self.messenger_scroll;
            if cy< -slot || cy>height+slot {continue;}
            let scale=u.source.adapter().icon_scale;
            crate::extras::messenger_icon(&inner.scaled(x,cy,scale),x,cy,u.source);
            inner.circle(x+10.0,cy-9.0,2.6,Color32::from_rgb(255,72,72));
            let tag=self.unread_tag(i).map(|tag|tag.to_string()).or_else(||(u.unidentified()).then(||"?".to_string()));
            if let Some(tag)=tag{
                inner.circle(x+10.0,cy+8.5,6.0,Color32::from_gray(238));
                inner.centered_text(x+10.0,cy+8.5,&tag,8.5,W::Bold,Color32::from_gray(28),H::Center);
            }
            let name=if u.unidentified(){format!("{} · 账号未确认",u.source.name())}
                else{match self.unread_tag(i){Some(n)=>format!("{} · 账号 {n}",u.source.name()),None=>u.source.name().to_string()}};
            let button=inner.hit(0.0,cy-slot/2.0,UNREAD_W,slot,self.unread_id(i)).on_hover_text(format!("查看{name}"));
            if button.clicked(){cmds.push(Cmd::ViewMessenger(u));}
        }
        let health=self.health_badges();
        let health_offset=self.pending_messengers.len();
        #[cfg(windows)]
        self.draw_plugin_notice_capsule(&inner, health_offset+health.len(), slot, height, self.messenger_scroll, UNREAD_W, cmds);
        for (i,kind) in health.into_iter().enumerate() {
            let cy=2.0+slot*((health_offset+i) as f32+0.5)-self.messenger_scroll;
            if cy< -slot || cy>height+slot {continue;}
            kind.icon(&inner,x,cy,0.8);
            inner.circle(x+10.0,cy-9.0,2.6,Color32::from_rgb(255,59,48));
            let button=crate::quota_ui::bar_tooltip(inner.hit(0.0,cy-slot/2.0,UNREAD_W,slot,kind.id().to_string()),format!("{} · 点击确认，重新计时",kind.label()));
            if button.clicked(){self.confirm_health_reminder(kind);}
        }
        if max_scroll>0.0 {
            // 还有没露出的图标：上下边缘各一条淡淡的指示。
            let ink=paint::alpha(theme::WHITE,0.35);
            if self.messenger_scroll>0.5 {c.fill(x-6.0,4.0,12.0,2.0,1.0,ink);}
            if self.messenger_scroll<max_scroll-0.5 {c.fill(x-6.0,height-6.0,12.0,2.0,1.0,ink);}
        }
    }

    /// 收起时的未读入口：每个来源一个图标加小红点（同一来源多个账号合成一个）。只有一条时点它直接跳转，多条时展开面板挑选。
    fn draw_unread_stack(&mut self,c:&Card,horizontal:bool,cmds:&mut Vec<Cmd>){
        let sources=self.unread_sources();
        let health=self.health_badges();
        let count=self.pending_messengers.len();
        let icons=sources.len()+health.len();
        #[cfg(windows)]
        let icons=icons+self.plugin_notice_ui.collapsed_count();
        let step=unread_step(horizontal);
        let at=|k:usize|if horizontal {(16.0+step*k as f32,14.0)}else{(16.0,14.0+step*k as f32)};
        for k in 0..sources.len() {
            let (x,y)=at(k);
            let scale=sources[k].adapter().collapsed_icon_scale;
            crate::extras::messenger_icon(&c.scaled(x,y,scale),x,y,sources[k]);
        }
        for (i,kind) in health.iter().enumerate(){let (x,y)=at(sources.len()+i);kind.icon(c,x,y,0.7);}
        // 每个来源图标右上角一个小红点，不显示数字。
        for k in 0..sources.len()+health.len() {let (x,y)=at(k);c.circle(x+8.0,y-7.0,2.6,Color32::from_rgb(255,59,48));}
        let length=self.unread_stack_len(horizontal);
        let messenger_length=if icons==sources.len(){length}else{sources.len() as f32*step+if horizontal{5.0}else{3.0}};
        let (w,h)=if horizontal {(messenger_length,BAR_H)}else{(32.0,messenger_length)};
        if count==1 {
            let u=self.pending_messengers[0];
            let button=c.hit(0.0,0.0,w,h,u.source.name()).on_hover_text(format!("查看{}",u.source.name()));
            if button.clicked(){cmds.push(Cmd::ViewMessenger(u));}
        }else if count>1 {
            let button=c.hit(0.0,0.0,w,h,"messenger-stack").on_hover_text(format!("{count} 条未读 · 展开挑选"));
            if button.clicked(){self.reveal_unread();}
        }
        #[cfg(windows)]
        self.draw_plugin_notice_stack(c,horizontal,sources.len()+health.len(),step,length,cmds);
        for (i,kind) in health.into_iter().enumerate() {
            let k=sources.len()+i;
            let (x,y)=at(k);
            let start=if k==0{0.0}else{(if horizontal{x}else{y})-step/2.0};
            let end=if k+1==icons{length}else{(if horizontal{x}else{y})+step/2.0};
            let button=if horizontal{c.hit(start,0.0,end-start,BAR_H,kind.id().to_string())}else{c.hit(0.0,start,32.0,end-start,kind.id().to_string())};
            if crate::quota_ui::bar_tooltip(button,format!("{} · 点击确认，重新计时",kind.label())).clicked(){self.confirm_health_reminder(kind);}
        }
    }
    /// 展开面板，让旁边的未读胶囊露出来。
    pub(crate) fn reveal_unread(&mut self){
        self.notice=None;
        self.open=true; self.forced_open_until=self.t+4.0;
        self.w.tune(OPEN_W.0,OPEN_W.1);self.h.tune(OPEN_H.0,OPEN_H.1);
        self.w.target=PW;self.h.target=self.expanded_height();
    }
    fn notice_rect(&self,panel:Rect)->Option<Rect>{
        if !self.compact_notices(){return None;}
        let width=self.notice_width.pos-8.0;
        (width>0.5).then(||Rect::from_min_size(Pos2::new(panel.right()+8.0,panel.top()),Vec2::new(width,BAR_H)))
    }
    fn notice_details_rect(&self,badge:Option<Rect>)->Option<Rect>{
        let a=self.hud_notice.as_ref().filter(|_|self.notice_details&&self.notice.is_some())?;
        let badge=badge?;let (width,height)=self.notice_size(a);
        let mut x=badge.left();
        // 岛体靠近屏幕右缘时，详情卡片向左展开，始终完整留在屏幕内。
        if let Some(screen)=self.screen {x=x.min(screen.right()-width-8.0).max(screen.left()+8.0);}
        Some(Rect::from_min_size(Pos2::new(x,if self.dock()==model::Dock::Bottom {badge.top()-height-6.0}else{badge.bottom()+6.0}),Vec2::new(width,height)))
    }

    pub(crate) fn glassy(&self) -> bool {
        #[cfg(feature="d3d11-glass")]
        { return self.settings.skin == model::Skin::Liquid; }
        #[cfg(all(windows, feature = "sdk-material"))]
        { self.material_appearance.glass_foreground }
        #[cfg(not(all(windows, feature = "sdk-material")))]
        { false }
    }

    pub(crate) fn glass_controls_ready(&self) -> bool {
        #[cfg(feature="d3d11-glass")]
        { return self.glassy() && !self.calendar.draft.is_some() && self.open && self.progress() >= 0.985; }
        #[cfg(all(windows, feature = "sdk-material"))]
        { self.glassy() && self.material_appearance.controls && self.material_appearance.ready && self.open && self.reveal >= 0.6 && self.progress() >= 0.985 }
        #[cfg(not(all(windows, feature = "sdk-material")))]
        { false }
    }

    pub(crate) fn glass_control_scale(&self, _response: &egui::Response, press: f32) -> f32 {
        #[cfg(all(windows, feature = "sdk-material"))]
        if self.glass_controls_ready() {
            if let Some(scale) = crate::material::control_scale(&_response.ctx, _response.id) { return scale; }
        }
        1.0 + 0.14 * press
    }

    #[cfg(all(windows, feature = "sdk-material"))]
    fn material_glass_fill(&self, alpha: u8) -> Color32 {
        let (r, g, b) = if self.material_appearance.light { (248, 248, 250) } else { (24, 27, 33) };
        Color32::from_rgba_unmultiplied(r, g, b, alpha)
    }

    fn island_overlay_fill(&self) -> Color32 {
        #[cfg(all(windows, feature = "sdk-material"))]
        if self.glassy() { return self.material_glass_fill(215); }
        self.glass_fill()
    }

    pub(crate) fn island_palette(&self)->Option<crate::skin_paint::Palette> {
        #[cfg(all(windows, feature = "sdk-material"))]
        if self.glassy() {
            return Some(if self.material_appearance.light { crate::skin_paint::GLASS_LIGHT_READABLE } else { crate::skin_paint::GLASS_DARK });
        }
        self.skin_palette()
    }

    /// 岛体绘制使用的内置皮肤，插件选中时由它提供基础外观。
    pub fn effective_skin(&self)->model::Skin {
        #[cfg(feature="sdk-js")]
        if let Some(skin)=self.plugins.active_skin() {return skin.base;}
        self.settings.skin
    }

    /// 基础皮肤的换色调色板，也用于独立设置窗口的预览。
    pub(crate) fn skin_palette(&self)->Option<crate::skin_paint::Palette> {
        #[cfg(feature="sdk-js")]
        if let Some(skin)=self.plugins.active_skin() {return Some(skin.palette);}
        match self.settings.skin {model::Skin::Cream=>Some(crate::skin_paint::CREAM),model::Skin::GlassLight=>Some(crate::skin_paint::GLASS_LIGHT),model::Skin::GlassDark=>Some(crate::skin_paint::GLASS_DARK),model::Skin::Classic|model::Skin::Fluid=>None,
            #[cfg(feature="d3d11-glass")] model::Skin::Liquid=>None}
    }

    /// 设置页预览用的岛体底色：没有真实模糊，毛玻璃按较实的玻璃表现。
    pub(crate) fn preview_fill(&self)->Color32 {
        match self.effective_skin() {
            model::Skin::GlassDark=>{let (r,g,b)=glass::GROUND_DARK;Color32::from_rgba_unmultiplied(r,g,b,215)}
            model::Skin::GlassLight=>{let [r,g,b]=glass::LIGHT_GROUND.map(|v|(v*255.0).round() as u8);Color32::from_rgba_unmultiplied(r,g,b,225)}
            _=>self.glass_fill(),
        }
    }

    /// 岛体底色：经典为纯黑；奶油为暖白；毛玻璃按当前着色量叠加深色或浅色玻璃。
    fn glass_fill(&self)->Color32 {
        #[cfg(feature="sdk-js")]
        if let Some(skin)=self.plugins.active_skin().filter(|skin|matches!(skin.base,model::Skin::Classic|model::Skin::Cream)) {
            let [r,g,b]=skin.palette.paper;
            return Color32::from_rgb(r,g,b);
        }
        let alpha=(self.glass_tint.clamp(0.0,1.0)*255.0).round() as u8;
        match self.effective_skin() {
            model::Skin::GlassDark=>{let (r,g,b)=glass::GROUND_DARK;Color32::from_rgba_unmultiplied(r,g,b,alpha)}
            model::Skin::GlassLight=>{let [r,g,b]=glass::LIGHT_GROUND.map(|v|(v*255.0).round() as u8);Color32::from_rgba_unmultiplied(r,g,b,alpha)}
            model::Skin::Cream=>{let [r,g,b]=crate::skin_paint::CREAM.paper;Color32::from_rgb(r,g,b)}
            model::Skin::Classic=>Color32::BLACK,
            model::Skin::Fluid=>self.fluid.compose(0.5,0.5,1.0,1.0,0.0),
            #[cfg(feature="d3d11-glass")] model::Skin::Liquid=>Color32::TRANSPARENT,
        }
    }

    /// 毛玻璃边缘高光：浅色玻璃更亮，像苹果的玻璃边缘。
    fn glass_rim(&self)->Color32 {
        let strength=if self.effective_skin()==model::Skin::GlassLight {0.55} else {glass::rim_alpha()*0.7};
        Color32::from_white_alpha((255.0*strength).round() as u8)
    }

    /// 绘制岛体（或通知胶囊）的本体：经典皮肤为纯黑；毛玻璃为着色底 + 顶部柔光 + 边缘高光。
    fn paint_body(&self,painter:&egui::Painter,rect:Rect,flare:f32,flare_h:f32,radius:f32,feather:f32){
        let skin=self.effective_skin();
        #[cfg(feature="d3d11-glass")]
        {
            // 只有选中「液态玻璃」皮肤才发布几何给 HLSL pass；其它皮肤每帧关掉，
            // 避免残留上一次的 body_rect 造成折射残影（与 custom 行为一致）。
            if skin==model::Skin::Liquid {
                let is_notch=self.settings.island_shape!=model::IslandShape::Pill;
                crate::gpu_glass::set_shape(flare,flare_h,is_notch);
                let center=rect.center();
                crate::gpu_glass::set_body(rect,radius,(center.x,center.y),crate::gpu_glass::theme_level());
                // 核心控件透镜：齿轮/播放/切歌/音量/翻页箭头（坐标与 pages.rs 绘制一致）。
                // 融球水滴不接；其余控件的透镜在后续 PR 补。
                let mut ctls = Vec::new();
                if self.page == 0 && self.media.title.len() > 0 {
                    for (x, r) in [(74.0, 15.0), (162.0, 15.0), (210.5, 15.0), (258.0, 15.0), (367.0, 15.0)] {
                        ctls.push(crate::gpu_glass::Ctl {
                            rect: Rect::from_center_size(egui::pos2(x, 118.0), egui::vec2(r * 2.0, r * 2.0)),
                            radius: r, mag: 1.06, refract: (r / 70.0).clamp(0.3, 1.0), style: 0 });
                    }
                }
                ctls.push(crate::gpu_glass::Ctl {
                    rect: Rect::from_center_size(egui::pos2(394.0, 18.0), egui::vec2(26.0, 26.0)),
                    radius: 13.0, mag: 1.06, refract: 0.3, style: 0 });
                crate::gpu_glass::set_controls(&ctls);
            } else {
                crate::gpu_glass::set_active(false);
            }
        }
        if skin==model::Skin::Fluid {
            let pill=self.settings.island_shape==model::IslandShape::Pill;
            let (map,frame)=shape::EdgeMap::of(rect,if pill {model::Dock::Top}else{self.dock()});
            let mut rows=shape::sections(frame,flare,flare_h,radius);
            if pill {
                let mid=frame.center().y;
                rows.retain(|row|row.0>mid && row.0>=frame.bottom()-radius);
                let top=rows.iter().rev().map(|&(y,l,r)|(2.0*mid-y,l,r)).collect::<Vec<_>>();
                rows.splice(0..0,top);
            }
            let expand=self.progress().clamp(0.0,1.0);
            let mut mesh=shape::shade_mesh(frame,rows,feather,&|x,y|{
                let p=map.apply(Pos2::new(x,y))-rect.min;
                self.fluid.compose(p.x,p.y,rect.width(),rect.height(),expand)
            });
            if self.dock()!=model::Dock::Top && !pill {
                for vertex in &mut mesh.vertices {vertex.pos=map.apply(vertex.pos);}
            }
            painter.add(egui::Shape::mesh(mesh));
            return;
        }
        if self.settings.island_shape==model::IslandShape::Pill {
            let radius=radius.min(rect.height().min(rect.width())*0.5);
            painter.rect_filled(rect,radius,self.glass_fill());
            if skin.is_glass() && rect.height()>=10.0 {
                painter.rect_stroke(rect,radius,egui::Stroke::new(1.0_f32,self.glass_rim()),egui::StrokeKind::Inside);
            }
            return;
        }
        let dock=self.dock();
        if dock!=model::Dock::Top {
            shape::fill_docked(painter,rect,dock,flare,flare_h,radius,self.glass_fill(),feather);
            if !skin.is_glass() {return;}
            if skin==model::Skin::GlassLight {
                let sheen=Color32::from_white_alpha(15);
                shape::fill_gradient_docked(painter,rect,dock,flare,flare_h,radius,sheen,Color32::TRANSPARENT,(rect.width()*0.6).min(64.0),feather);
            }
            if rect.height()>=20.0 && rect.width()>=10.0 {
                let edge=if dock==model::Dock::Bottom {Rect::from_min_max(rect.min+Vec2::splat(0.5),rect.max-Vec2::new(0.5,0.0))}
                    else if dock==model::Dock::Left {Rect::from_min_max(rect.min+Vec2::new(0.0,0.5),rect.max-Vec2::new(0.5,0.5))}
                    else {Rect::from_min_max(rect.min+Vec2::new(0.5,0.5),rect.max-Vec2::new(0.0,0.5))};
                painter.add(egui::Shape::line(shape::outline_docked(edge,dock,flare,flare_h,(radius-0.5).max(0.0)),egui::Stroke::new(1.0_f32,self.glass_rim())));
            }
            return;
        }
        shape::fill(painter,rect,flare,flare_h,radius,self.glass_fill(),feather);
        if !skin.is_glass() {return;}
        // 玻璃厚度带来的上亮下暗：顶部白光在前 60% 高度（最多 64 点）内淡出；浅色玻璃更亮。
        if skin==model::Skin::GlassLight {
            let sheen=Color32::from_white_alpha(15);
            shape::fill_gradient(painter,rect,flare,flare_h,radius,sheen,Color32::TRANSPARENT,(rect.height()*0.6).min(64.0),feather);
        }
        if rect.height()>=10.0 && rect.width()>=20.0 {
            let edge=Rect::from_min_max(rect.min+Vec2::new(0.5,0.0),rect.max-Vec2::new(0.5,0.5));
            painter.add(egui::Shape::line(shape::outline(edge,flare,flare_h,(radius-0.5).max(0.0)),egui::Stroke::new(1.0_f32,self.glass_rim())));
        }
    }

    /// 拿起时沿岛体轮廓的一圈柔光：外圈宽而淡，内圈细而亮。
    fn paint_lift_halo(&self,painter:&egui::Painter,rect:Rect,flare:f32,flare_h:f32,radius:f32,amount:f32) {
        if rect.width()<10.0 || rect.height()<10.0 {return;}
        let pill=self.settings.island_shape==model::IslandShape::Pill;
        let dock=self.dock();
        for (grow,width,alpha) in [(2.0f32,4.0f32,22.0f32),(0.5,1.2,90.0)] {
            let stroke=egui::Stroke::new(width,Color32::from_white_alpha((alpha*amount) as u8));
            let edge=rect.expand(grow);
            if pill {
                let r=radius.min(rect.height().min(rect.width())*0.5)+grow;
                painter.rect_stroke(edge,r,stroke,egui::StrokeKind::Middle);
            } else if dock!=model::Dock::Top {
                painter.add(egui::Shape::line(shape::outline_docked(edge,dock,flare,flare_h,radius+grow),stroke));
            } else {
                painter.add(egui::Shape::line(shape::outline(edge,flare,flare_h,radius+grow),stroke));
            }
        }
    }

    fn update_position_drag(&mut self,ctx:&egui::Context,env:&Env,dt:f32) {
        let limit=model::ISLAND_OFFSET_MAX as f32;
        let mut seam_strength=0.0;
        if let Some(mut drag)=self.position_drag.take() {
            let current_id=&self.settings.display_id;
            let monitor=if *current_id==drag.original_display {drag.original_monitor.as_ref()}else{Some(current_id)};
            let disconnected=monitor.is_some_and(|id|!self.displays.iter().any(|d|d.id==*id));
            if disconnected || env.cancel_position_drag || ctx.input(|i|i.key_pressed(egui::Key::Escape)) {
                self.settings.display_id=drag.original_display;
                (self.settings.dock,self.settings.island_offset)=drag.original;
                self.drag_glide=None;
                self.drag_stretch.target=0.0;
                self.hover_lock=true;
                self.center_locked=false;
                self.center_guide=0.0;
                self.crossed_at=f64::NEG_INFINITY;
                self.seam_guide=0.0;
                return;
            }
            let mut speed=0.0;
            let mut geometry=env.cursor.zip(env.screen).map(|(p,r)|(p-r.min,r.size()));
            let mut crossed=false;
            let mut seam=None;
            let mut bottom_inset=0.0;
            if let (Some(point),Some(active))=(env.desktop_cursor,crate::display_layout::selected(&self.displays,current_id)) {
                let zoom=self.settings.island_scale()*ctx.zoom_factor();
                let target=crate::display_layout::drag_target(&self.displays,active,point,zoom);
                crossed=target.id!=active.id;
                let scale=target.scale(zoom);
                let b=target.dock_bounds(self.dock(),self.settings.avoids_top_bar());
                if self.dock()!=model::Dock::Bottom {bottom_inset=(b.bottom-target.bottom_edge()) as f32/scale;}
                geometry=Some((Vec2::new((point.0-b.left) as f32,(point.1-b.top) as f32)/scale,
                    Vec2::new((b.right-b.left) as f32,(b.bottom-b.top) as f32)/scale));
                seam=self.displays.iter().filter(|d|d.id!=target.id)
                    .filter_map(|d|crate::display_layout::shared_edge(target.bounds,d.bounds,point))
                    .map(|(depth,dock)|(depth as f32/scale,dock))
                    .min_by(|a,b|a.0.abs().total_cmp(&b.0.abs()));
                if crossed {
                    self.settings.display_id=if Some(&target.id)==drag.original_monitor.as_ref() {drag.original_display.clone()}else{target.id.clone()};
                    self.crossed_at=self.t;
                    self.center_locked=false;
                }
            }
            if let Some((cursor,size))=geometry {
                let edges=[(model::Dock::Top,cursor.y.max(0.0)),(model::Dock::Left,cursor.x.max(0.0)),(model::Dock::Right,(size.x-cursor.x).max(0.0)),(model::Dock::Bottom,(size.y-bottom_inset-cursor.y).max(0.0))];
                let &(nearest,distance)=edges.iter().min_by(|a,b|a.1.total_cmp(&b.1)).unwrap();
                let current=edges.iter().find(|(dock,_)|*dock==self.dock()).unwrap().1;
                // 靠近新边缘才换边；角落保留缓冲，防止横竖形态来回跳变。
                let mut target=Some(nearest).filter(|dock|*dock!=self.dock() && distance<=64.0 && distance+24.0<current);
                // 左右对侧：过了中线、明显更靠近另一侧就换过去，不必甩到屏幕尽头；中线附近留缓冲不来回跳。
                if target.is_none() && self.side() {
                    let (opposite,far)=if self.dock()==model::Dock::Left {(model::Dock::Right,edges[2].1)}else{(model::Dock::Left,edges[1].1)};
                    if far+size.x*0.07<current {target=Some(opposite);}
                }
                if crossed {target=Some(seam.filter(|(depth,_)|depth.abs()<=64.0).map_or(nearest,|(_,dock)|dock));}
                let switch=target.is_some();
                if let Some(dock)=target {self.settings.dock=dock;}
                if let Some((depth,_))=seam.filter(|(_,dock)|*dock==self.dock()) {
                    seam_strength=(1.0-smoothstep(16.0,72.0,depth))*smoothstep(-16.0,0.0,depth);
                }
                let (position,length,extent,start)=if self.side() {(cursor.y,size.y,crate::side_dock::SIDE_HALF_EXTENT,drag.start.y)}
                    else {(cursor.x,size.x,crate::display_layout::horizontal_half_extent(self.horizontal_width),drag.start.x)};
                let travel=(length/2.0-extent).max(0.0);
                let raw=if travel<=0.0 {0.0}else if switch {(position-length/2.0)/travel*limit}
                    else {drag.offset+(position-start)/travel*limit};
                let raw=raw.clamp(-limit,limit);
                // 居中和拖动前的位置有轻微吸附，窗口弹簧会把这一下「吸过去」。
                // 居中带滞回：进入范围小、松开范围大；刚吸住时岛体轻轻一沉，像卡进了槽位。
                let centered=raw.abs()<=if self.center_locked {CENTER_RELEASE}else{DRAG_MAGNET};
                if centered && !self.center_locked {
                    self.center_lock_at=self.t;
                    self.drag_lift.vel-=2.8;
                }
                self.center_locked=centered;
                let original=Some(drag.original.1 as f32).filter(|_|self.dock()==drag.original.0 && self.settings.display_id==drag.original_display);
                let target=if centered {0.0}else{original.filter(|p|(raw-p).abs()<=DRAG_MAGNET).unwrap_or(raw)};
                self.settings.island_offset=target.round() as i32;
                let glide=self.drag_glide.get_or_insert_with(||anim::Spring::new(drag.offset,GLIDE_FOLLOW.0,GLIDE_FOLLOW.1));
                glide.target=target;
                if switch {
                    // 换边时画布横竖互换，位置和外形直接到位，避免从旧边缘滑过去。
                    glide.snap(target);
                    drag.start=cursor;drag.offset=raw;
                    let (bw,bh)=self.base_size();
                    self.w.snap(bw);self.h.snap(bh);
                    self.drag_stretch.snap(0.0);
                }
                if travel>0.0 {speed=self.drag_glide.map_or(0.0,|g|g.vel.abs()/limit*travel);}
            }
            self.drag_stretch.target=(speed/STRETCH_SPEED).min(1.0)*STRETCH_MAX;
            if !env.primary_down.unwrap_or_else(||ctx.input(|i|i.pointer.primary_down())) {
                self.position_drag=None;
                self.hover_lock=true;
                self.drag_stretch.target=0.0;
                if let Some(glide)=&mut self.drag_glide {
                    glide.tune(GLIDE_SETTLE.0,GLIDE_SETTLE.1);
                    glide.target=self.settings.island_offset as f32;
                }
                if self.settings.display_id!=drag.original_display || (self.dock(),self.settings.island_offset)!=drag.original {self.settings.save();}
            }else{
                self.position_drag=Some(drag);
            }
        }
        self.seam_guide=anim::approach(self.seam_guide,seam_strength,24.0,dt);
        self.drag_lift.target=if self.position_drag.is_some() {1.0}else{0.0};
        self.drag_lift.step(dt);
        self.drag_stretch.step(dt);
        if let Some(glide)=&mut self.drag_glide {
            glide.step(dt);
            if self.position_drag.is_none() && glide.settled(limit) {self.drag_glide=None;}
        }
        // 描边：拖动中按离居中的距离渐显；松手后稍作停留再淡出，让人确认落在了中间。
        let dragging=self.position_drag.is_some();
        let distance=self.drag_glide.map_or(f32::INFINITY,|g|g.pos.abs());
        let want=if dragging {1.0-smoothstep(1.5,CENTER_GUIDE_RANGE,distance)}else{0.0};
        self.center_guide=anim::approach(self.center_guide,want,if dragging {18.0}else{5.0},dt);
        let lit=self.center_locked && (dragging || self.center_guide>0.05);
        self.center_lock_a=anim::approach(self.center_lock_a,if lit {1.0}else{0.0},16.0,dt);
        if !dragging && self.center_guide<0.002 {self.center_locked=false;}
    }

    /// 拖动接近居中时由岛体轮廓和一根中轴细线提示，不用文字：
    /// 越靠近居中，轮廓浮出越清晰的淡白描边，屏幕中轴细线与岛体中心刻度逐渐重合；
    /// 吸住瞬间描边和细线一起转为绿色、一道亮光从中心沿轮廓向两端扫开，停在中间期间绿边缓慢呼吸。
    /// 返回需要留在窗口区域内的范围。
    fn paint_center_rim(&self,painter:&egui::Painter,body:Rect,panel:Rect,flare:f32,flare_h:f32,radius:f32,screen:Option<Rect>)->Option<Rect> {
        let sweep=((self.t-self.center_lock_at)/CENTER_SWEEP) as f32;
        let sweeping=(0.0..1.0).contains(&sweep);
        let vis=self.center_guide;
        if (vis<0.004 && !sweeping) || body.width()<10.0 || body.height()<10.0 {return None;}
        let dock=self.dock();
        let side=dock.is_side();
        let along=if side {Vec2::Y}else{Vec2::X};
        let pill=self.settings.island_shape==model::IslandShape::Pill;
        let outline=|grow:f32| if pill {round_rect_points(body.expand(grow),radius+grow)}
            else if dock!=model::Dock::Top {shape::outline_docked(body.expand(grow),dock,flare,flare_h,radius+grow)}
            else {shape::outline(body.expand(grow),flare,flare_h,radius+grow)};
        // 颜色即状态：接近时中性白，吸住后过渡为绿色。
        let tint=|mix:f32,alpha:f32| {
            let c=|w:f32,a:f32|(lerp(w,a,mix)) as u8;
            Color32::from_rgba_unmultiplied(c(255.0,CENTER_TINT.0),c(255.0,CENTER_TINT.1),c(255.0,CENTER_TINT.2),(alpha.clamp(0.0,1.0)*255.0) as u8)
        };
        let lock=self.center_lock_a;
        let points=outline(0.5);
        if vis>0.004 {
            // 停在中间时绿边以 1.6 秒为周期缓慢呼吸，持续表明「就在中间」。
            let breathe=0.5+0.5*((self.t*std::f64::consts::TAU/1.6).sin() as f32);
            let strength=vis*lerp(0.32,lerp(0.78,1.0,breathe),lock);
            if lock>0.01 {painter.add(egui::Shape::line(outline(2.0),egui::Stroke::new(5.0,tint(1.0,0.22*strength*lock))));}
            painter.add(egui::Shape::line(points.clone(),egui::Stroke::new(lerp(1.0,1.6,lock),tint(lock,0.95*strength))));
        }
        let mut bounds=body.expand(8.0);
        // 中轴细线：屏幕中轴一条向外渐隐的细线，岛体中心一截短刻度，两者重合即居中；吸住后细线同样转绿。
        // 细线画在桌面上，底色未知：彩芯加淡黑描边，深浅壁纸都看得清。
        if let Some(screen)=screen.filter(|_|vis>0.004) {
            let travel=if side {screen.height()/2.0-crate::side_dock::SIDE_HALF_EXTENT}else{screen.width()/2.0-crate::display_layout::horizontal_half_extent(self.horizontal_width)}.max(0.0);
            // 屏幕中轴相对岛体中心的位置（与窗口跟随同源，不会差一帧）。
            let off=self.placement_offset()*travel;
            let (out,edge)=match dock {
                model::Dock::Bottom=>(-Vec2::Y,panel.center_top()),
                model::Dock::Top=>(Vec2::Y,Pos2::new(panel.center().x,panel.bottom())),
                model::Dock::Left=>(Vec2::X,Pos2::new(panel.right(),panel.center().y)),
                model::Dock::Right=>(-Vec2::X,Pos2::new(panel.left(),panel.center().y)),
            };
            let start=edge+out*6.0;
            let axis=start-along*off;
            let length=lerp(24.0,32.0,lock);
            let line=|from:Pos2,len:f32,strength:f32,width:f32,mix:f32| {
                const N:usize=12;
                for i in 0..N {
                    let (a,b)=(i as f32/N as f32,(i+1) as f32/N as f32);
                    let fade=(1.0-(a+b)*0.5).powf(1.3)*strength;
                    if fade<0.004 {continue;}
                    let (p,q)=(from+out*len*a,from+out*len*b);
                    painter.line_segment([p,q],egui::Stroke::new(width+2.0,Color32::from_black_alpha((40.0*fade) as u8)));
                    if mix>0.01 {painter.line_segment([p,q],egui::Stroke::new(width+3.0,tint(1.0,0.10*fade*mix)));}
                    painter.line_segment([p,q],egui::Stroke::new(width,tint(mix,0.80*fade)));
                }
            };
            // 细线只做辅助：比描边更短更淡，吸住后也只转成偏浅的绿。
            line(axis,length,vis*lerp(0.40,0.70,lock),lerp(1.0,1.3,lock),lock*0.7);
            if off.abs()>0.5 {line(start,8.0,vis*0.65*(1.0-lock),1.0,0.0);}
            bounds=bounds.union(Rect::from_two_pos(start,axis+out*length).expand(6.0));
        }
        // 扫光：吸住瞬间从岛体中心沿轮廓向两端对称扫开，扫到两端时淡尽。
        if sweeping {
            let half=if side {body.height()}else{body.width()}*0.5;
            let front=anim::ease_out_cubic(sweep)*(half+12.0);
            let fade=(1.0-sweep).powf(1.2);
            let mid=body.center();
            for pair in points.windows(2) {
                let m=pair[0]+(pair[1]-pair[0])*0.5;
                let d=(m-mid).dot(along).abs();
                let g=(-((d-front)/18.0).powi(2)).exp()*fade;
                if g<0.01 {continue;}
                painter.line_segment([pair[0],pair[1]],egui::Stroke::new(6.0,tint(1.0,0.30*g)));
                painter.line_segment([pair[0],pair[1]],egui::Stroke::new(1.8,tint(0.45,0.95*g)));
            }
        }
        Some(bounds)
    }

    pub fn dragging_position(&self)->bool {self.position_drag.is_some()}

    /// 拖动中和落位期间窗口使用的位置（-1 … 1，不取整）；其余时候就是保存的位置。
    pub fn placement_offset(&self)->f32 {
        let limit=model::ISLAND_OFFSET_MAX as f32;
        self.drag_glide.map_or(self.settings.island_offset_ratio(),|g|(g.pos/limit).clamp(-1.0,1.0))
    }

    /// 拿起与速度拉伸带来的外形缩放：(宽, 高)。
    fn drag_scale(&self)->(f32,f32) {
        let lift=self.drag_lift.pos.max(0.0);let stretch=self.drag_stretch.pos.max(0.0);
        let along=1.0+LIFT_SCALE.0*lift+stretch;
        let arrival=1.0-smoothstep(0.0,0.12,(self.t-self.crossed_at) as f32);
        let across=(1.0+LIFT_SCALE.1*lift-stretch*0.5)*(1.0-0.22*arrival);
        if self.side() {(across,along)}else{(along,across)}
    }

    fn start_position_drag(&mut self,start:Pos2,screen:Rect) {
        let offset=self.settings.island_offset;
        let original_monitor=crate::display_layout::selected(&self.displays,&self.settings.display_id).map(|d|d.id.clone());
        self.position_drag=Some(PositionDrag{original_display:self.settings.display_id.clone(),original_monitor,start:start-screen.min,offset:offset as f32,original:(self.dock(),offset)});
        let mut glide=anim::Spring::new(offset as f32,GLIDE_FOLLOW.0,GLIDE_FOLLOW.1);
        if let Some(g)=self.drag_glide {glide.pos=g.pos;glide.vel=g.vel;}
        self.drag_glide=Some(glide);
        // 本来就在中间：直接显示已居中，不重复扫光。
        self.center_locked=offset==0;
        // 拿起时给一点初速度，按下后立刻有反馈。
        self.drag_lift.vel=self.drag_lift.vel.max(4.0);
    }

    /// 悬停状态机：意图缓冲 → 弹簧展开 → 离开延迟 → 临界阻尼收起；全程可打断。
    fn morph(&mut self, want: bool, inside: bool, t: f64, dt: f32) {
        let (bw, bh) = self.base_size();
        if self.position_drag.is_some() {
            // 拿起后面板快速收成常驻形态；放大效果由 `drag_scale` 叠加在外形上。
            self.open=false;
            self.hover_lock=true;
            self.hover_since=None;
            self.leave_since=None;
            self.forced_open_until=0.0;
            self.music_volume_open=false;
            self.w.tune(CLOSE.0, CLOSE.1);
            self.h.tune(CLOSE.0, CLOSE.1);
            self.w.target=bw;
            self.h.target=bh;
        } else if want {
            self.leave_since = None;
            let since = *self.hover_since.get_or_insert(t);
            let intent=if self.effective_minimal() {MINIMAL_HOVER_INTENT}else{HOVER_INTENT};
            if !self.open && (self.drag_hot || t - since >= intent) {
                self.open = true;
                self.w.tune(OPEN_W.0, OPEN_W.1);
                self.h.tune(OPEN_H.0, OPEN_H.1);
                self.w.target = PW;
                self.h.target = self.expanded_height();
                if self.drag_hot {
                    // 拖入不等缓冲，直接给一点初速度，30 ms 内就能看到形变
                    self.w.vel = self.w.vel.max(600.0);
                    self.h.vel = self.h.vel.max(260.0);
                }
            } else if !self.open {
                self.w.tune(SWELL_K.0, SWELL_K.1);
                self.h.tune(SWELL_K.0, SWELL_K.1);
                // 悬停时轻微胀大：厚度方向胀得多一点（侧边停靠时厚度是宽度）。
                let (sw,sh)=if self.side(){(SWELL.1,SWELL.0)}else{SWELL};
                self.w.target = bw * sw;
                self.h.target = bh * sh;
            }
        } else {
            self.hover_since = None;
            if self.open {
                let since = *self.leave_since.get_or_insert(t);
                if t - since >= LEAVE_DELAY {
                    self.open = false;
                    self.music_volume_open=false;
                    self.leave_since = None;
                    self.w.tune(CLOSE.0, CLOSE.1);
                    self.h.tune(CLOSE.0, CLOSE.1);
                    self.w.target = bw;
                    self.h.target = bh;
                    self.cancel_edit();
                    self.confirm_clear = None;
                }
            } else {
                self.w.tune(CLOSE.0, CLOSE.1);
                self.h.tune(CLOSE.0, CLOSE.1);
                self.w.target = bw;
                self.h.target = bh;
            }
        }
        let _ = inside;
        self.page_height.target=self.expanded_height();
        if self.open {self.h.target=self.page_height.target;}
        self.page_height.step(dt);
        let (prev_w,prev_h)=(self.w.pos,self.h.pos);
        self.w.step(dt);
        self.h.step(dt);
        // 常驻条比展开面板还宽时（比如开了综合监测），展开是「由宽变窄」。宽高各走各的弹簧会出现又宽又高的空黑框。
        // 这时宽度不再单独弹，直接跟着高度的进度走：同一个动作里一边往下长一边收窄，收起时反过来，和短岛一样一气呵成。
        if !self.side() && self.position_drag.is_none() && bw > PW + 1.0 {
            let full = self.expanded_height();
            // 从悬停微胀的形态起步；收起时回到微胀高度就交还给弹簧，悬停微胀照旧。
            let start = bh * SWELL.1;
            let from = if self.open {bw * SWELL.0} else {bw};
            if self.open || self.h.pos > start + 0.5 {
                let k = ((self.h.pos - start) / (full - start).max(1.0)).clamp(0.0, 1.0);
                let ease = 1.0 - (1.0 - k) * (1.0 - k);
                self.w.pos = lerp(from, PW, ease);
                self.w.vel = 0.0;
                self.w.target = if self.open {PW} else {bw};
            }
        }
        // 仅在从外侧越过目标时收住，扩展提示条时不强制跳到目标宽度。
        if !self.open {
            if prev_w>=bw && self.w.pos<bw {self.w.snap(bw);}
            if prev_h>=bh && self.h.pos<bh {self.h.snap(bh);}
        }
        for d in &mut self.dots {
            d.step(dt);
        }
    }

    // ---------------------------------------------------------------
    // 顶栏：电池 + 日期 + 时钟。药丸里的电池与时钟会「飞」到顶栏位置（共享元素过渡）。
    // ---------------------------------------------------------------
    fn shared_header(&mut self, base: &Card, cx: f32, p: f32, reveal: f32, env: &Env, resident_day: Option<&ResidentDay>) {
        let local_cx = cx - base.origin.x;
        let fit = self.resident_fit();
        let fitted=base.scaled(local_cx,15.0,lerp(fit,1.0,p));
        let base=&fitted;
        let lyric=self.island_lyric().map(|(index,_)|index);
        let lyric_alpha=if self.island_lyrics_active() {1.0-smoothstep(0.0,0.35,p)}else{0.0};
        if let Some(galley)=self.island_lyric_layout.clone().filter(|_|lyric_alpha>0.003) {
            let c=base.faded(lyric_alpha);
            let content_width=self.lyric_bar_width()-self.lyric_focus_space();
            let width=self.w.pos.max(content_width);
            let left=local_cx-width/2.0;
            let center_y=BAR_H/2.0;
            if let Some((image,texture))=&self.music_artwork {
                let [width,height]=image.size;
                let uv_size=if width>height {egui::vec2(height as f32/width as f32,1.0)}else{egui::vec2(1.0,width as f32/height as f32)};
                let uv=Rect::from_center_size(Pos2::new(0.5,0.5),uv_size);
                c.painter.add(egui::epaint::RectShape::filled(c.rect(left+12.0,center_y-10.0,20.0,20.0),5.0*c.s,Color32::WHITE).with_texture(texture.id(),uv));
            } else {
                c.fill(left+12.0,center_y-10.0,20.0,20.0,5.0,theme::CARD);
                paint::note(&c,left+22.0,center_y,13.0,theme::WHITE);
            }
            let index=lyric.unwrap_or(0);
            let id=egui::Id::new("island-lyric-line");
            if !self.ly.island_animation_started {
                c.ui.ctx().animate_value_with_time(id,index as f32,0.0);
                self.ly.island_animation_started=true;
            }
            let shown=c.ui.ctx().animate_value_with_time(id,index as f32,0.18);
            let fade=1.0-(index as f32-shown).abs().min(1.0);
            let position=c.pos(left+content_width-LYRIC_BAR_W/2.0,center_y-galley.size().y/2.0+(1.0-fade)*3.0);
            c.painter.galley(position,galley,if lyric.is_some(){theme::WHITE}else{theme::GRAY});
            paint::playback_bars(&c,left+content_width-21.0,center_y,self.t,self.media.playing,theme::WHITE);
            if self.pomo.running {
                let alpha=smoothstep(0.88,1.0,(width-content_width)/self.lyric_focus_space());
                crate::focus_bar::draw(&c.faded(alpha),left+width-14.0-crate::focus_bar::LYRIC_WIDTH/2.0,center_y,self.t,&self.pomo,self.phase_secs(self.pomo.phase),true);
            }
        }
        if lyric_alpha>=0.997 {return;}
        let header=base.faded(1.0-lyric_alpha);
        let base=&header;
        let m = anim::ease_in_out(smoothstep(0.0, 0.85, p));
        let clock = self.settings.clock_text(&env.clock);
        let show_battery=self.settings.show_battery && env.battery.is_some();

        // 顶栏终点布局：[电池][日期][时钟] 作为一组整体居中
        let date_w = base.text_w(&env.date, size::DATE, W::Regular);
        let clock_w = paint::clock_width(base,&clock,size::CLOCK);
        let battery_space=if show_battery {24.0}else{0.0};
        let group = battery_space + date_w + 8.0 + clock_w;
        // 顶栏共用日程「新增」和天气「刷新」之间的空间，切页时日期和时钟保持原位。
        let (left,right)=(155.0,300.0-self.header_music_shift());
        // 直接过渡时钟实际需要移动的距离，避免先等边界追上，再在最后几帧挤动文字。
        let ctx=base.ui.ctx();
        let target_fit=((right-left)/group).min(1.0);
        let target_x=(local_cx-group*target_fit/2.0+if show_battery {6.0}else{0.0}).min(right-group*target_fit).max(left);
        let fit=ctx.animate_value_with_time(egui::Id::new("header-clock-fit"),target_fit,0.18);
        let gx=ctx.animate_value_with_time(egui::Id::new("header-clock-x"),target_x,0.18);
        let hb = (gx + 8.5*fit, 18.6);
        let hd = gx + battery_space*fit;
        let hc = hd + (date_w + 8.0)*fit;
        // 药丸起点布局
        let has_music=self.media.is_music();
        let layout=self.resident.current();
        let pb=(local_cx+layout.x[bar::BATTERY].unwrap_or(0.0),15.0);
        let pcx=layout.x[bar::CLOCK].map_or(hc,|x|{
            let width=if resident_day.is_some(){layout.widths[bar::CLOCK]}else{
                paint::clock_width(base,&clock,size::PILL_CLOCK)
            };
            local_cx+x-width/2.0
        });
        let clock_clearance=if resident_day.is_some(){layout.date_group_clearance()}else{layout.clearance(bar::CLOCK)};

        if self.effective_minimal() && self.notification_bell().is_none() && p < 0.02 { return; }
        // 电池：位置/大小连续插值
        let bk = lerp(1.0, 0.8*fit, m);
        let bpos = (lerp(pb.0, hb.0, m), lerp(pb.1, hb.1, m));
        if show_battery {paint::battery(&base.faded(lerp(layout.clearance(bar::BATTERY),1.0,m)), bpos.0, bpos.1, env.battery, env.charging, bk);}

        // 时钟：两个字号沿同一路径交叉淡化，避免逐帧换字号
        let cpos = (lerp(pcx, hc, m), lerp(21.0, 23.0, m));
        let a_pill = (1.0 - smoothstep(0.25, 0.6, m)) * clock_clearance;
        let a_head = smoothstep(0.35, 0.75, m);
        if self.settings.show_clock && a_pill > 0.0 {
            paint::clock(&base.faded(a_pill),cpos.0,cpos.1,&clock,size::PILL_CLOCK);
        }
        if a_head > 0.0 {
            paint::clock(&base.faded(a_head),cpos.0,cpos.1-0.5,&clock,size::CLOCK*fit);
        }
        // 日期随内容淡入，从药丸方向轻轻滑出
        if reveal > 0.0 {
            base.faded(reveal).text(hd + (1.0 - reveal) * 10.0, 22.5, &env.date, size::DATE*fit, W::Regular, Color32::from_rgb(210, 210, 214), H::Left);
        }
        if self.open && reveal>0.95 && p>0.98 && !self.is_editing() && !self.drag_hot {
            if let Some(screen)=env.screen {
                // 中间的日期时间区不占用两侧的新增、刷新和设置按钮。
                let left=(hd-12.0).max(140.0).max(left);let right=(hc+clock_w*fit+12.0).min(303.0).min(right);
                if right>left {
                    // Windows 没有「抓手」光标，Grab 会变成四向箭头；保持普通箭头，改用岛体内的把手提示。
                    let response=base.hit_drag(left,4.0,right-left,26.0,"island-position").on_hover_text("按住拖动位置");
                    let (_,press)=base.hover_t(&response);
                    // 展开时保留淡把手；悬停后加亮，按下立即反馈。
                    let dwell=(if response.hovered() {self.t-*self.grip_hover_since.get_or_insert(self.t)}else{self.grip_hover_since=None;0.0}) as f32;
                    if response.hovered() && dwell<1.2 {base.ui.ctx().request_repaint();}
                    let ctx=base.ui.ctx();
                    let grip_in=ctx.animate_value_with_time(response.id.with("grip"),smoothstep(0.30,0.65,dwell),0.12);
                    let fill_in=ctx.animate_value_with_time(response.id.with("grip-fill"),smoothstep(0.50,1.00,dwell),0.12);
                    let (grip_in,fill_in)=(grip_in.max(press).max(0.40),fill_in.max(press));
                    if grip_in>0.003 {
                        let pad=6.0;
                        let (x,w)=(left-pad,right-left+pad*2.0);
                        if fill_in>0.003 {base.fill(x,5.0,w,26.0,13.0,Color32::from_white_alpha((fill_in*7.0+press*12.0) as u8));}
                        let grip=lerp(14.0,22.0,press)*lerp(0.5,1.0,grip_in);
                        // 圆角由绘制器按实际像素高度截到半高，避免小数缩放时两端的抗锯齿边缘重叠。
                        let height=2.6;
                        base.fill(local_cx-grip/2.0,7.5,grip,height,height,Color32::from_white_alpha((grip_in*60.0+press*60.0) as u8));
                    }
                    // 开始移动立即拿起；原地按住片刻也会拿起，先给反馈再移动。
                    let held=response.is_pointer_button_down_on() && base.ui.input(|i|i.pointer.primary_down() && i.pointer.press_start_time().is_some_and(|at|i.time-at>=LIFT_HOLD));
                    if response.is_pointer_button_down_on() {base.ui.ctx().request_repaint();}
                    if response.drag_started_by(egui::PointerButton::Primary) || held {
                        if let Some(start)=base.ui.input(|i|i.pointer.press_origin()) {self.start_position_drag(start,screen);}
                    }
                }
            }
        }
        if has_music && (p>0.1 || layout.x[bar::MUSIC].is_some()) {
            let x=lerp(local_cx+layout.x[bar::MUSIC].unwrap_or(0.0),367.0-self.header_music_shift(),m);
            let y=lerp(15.0,18.0,m);
            paint::playback_bars(base,x,y,self.t,self.media.playing,theme::WHITE);
            if self.open && reveal>0.6 && base.hit(x-13.0,y-14.0,26.0,28.0,"header-music")
                .on_hover_text("返回音乐播放页")
                .on_hover_cursor(egui::CursorIcon::PointingHand).clicked(){self.show_page(0);}
        }
        let compact_alpha=1.0-smoothstep(0.0,0.22,p);
        let date_alpha=compact_alpha*clock_clearance;
        if date_alpha>0.004 {
            if let Some(day)=resident_day {
                day.draw_after_clock(&base.faded(date_alpha),pcx,self.settings.show_clock.then_some(clock.as_str()));
            }
        }
        if !self.compact_notices()&&p<0.1 {
            if let (Some(at),Some(x))=(self.notification_bell(),layout.x[bar::BELL]){
                let x=local_cx+x;paint::bell(&base.scaled(x,14.0,1.2),x,14.0,theme::WHITE,paint::bell_swing(self.t-at));
                base.circle(x+4.0,9.5,2.0,Color32::from_rgb(255,72,72));
                if self.module_on(MAIL_PAGE)&&base.hit(x-14.0,3.0,28.0,25.0,"bell-mail").clicked(){self.show_page(MAIL_PAGE);}
            }
        }
        if compact_alpha>0.004 {
            let c=base.faded(compact_alpha);let c=if compact_alpha<0.8{c.ghost()}else{c};
            self.draw_quota_bar(&c,local_cx,&layout);
            if let Some(x)=layout.x[bar::PERFORMANCE] {self.performance_bar(&c,local_cx+x);}
            self.draw_wireless_bar(&c,local_cx,&layout);
            if let Some(x)=layout.x[bar::WEATHER] { self.draw_pill_weather(&c,local_cx+x,&layout); }
            #[cfg(feature="sdk-js")]
            {
                let theme = self.pinned_canvas_theme();
                let mut clicked = None;
                for (slot, display) in layout.pinned_slots().zip(&self.pinned_displays) {
                    if let Some(x) = layout.x[slot] {
                        if display.draw(&c, local_cx + x, 15.0, layout.widths[slot], false, theme) {
                            clicked = Some(display.plugin.clone());
                        }
                    }
                }
                if let Some(id) = clicked {
                    self.show_status_page(&id);
                }
            }
        }
        if self.pomo.running && compact_alpha>0.004 {
            crate::focus_bar::draw(&base.faded(compact_alpha),local_cx+layout.x[bar::FOCUS].unwrap(),15.0,self.t,&self.pomo,self.phase_secs(self.pomo.phase),false);
        }
    }

    #[cfg(feature="sdk-js")]
    pub(crate) fn pinned_canvas_theme(&self) -> crate::plugin_canvas_ui::CanvasTheme {
        crate::plugin_canvas_ui::CanvasTheme {
            palette: self.island_palette(),
            accent: None,
            background: self.island_overlay_fill(),
        }
    }

    /// 常驻内容：顶部停靠走共享元素过渡（药丸里的电池、时钟飞到顶栏）；
    /// 左右停靠时竖排内容随展开淡出，顶栏随面板内容淡入。
    fn resident_header(&mut self, base: &Card, panel: Rect, cx: f32, p: f32, reveal: f32, env: &Env, resident_day: Option<&ResidentDay>) {
        if !self.side() {self.shared_header(base,cx,p,reveal,env,resident_day);return;}
        let bar_alpha=1.0-smoothstep(0.0,0.3,p);
        let minimal=self.effective_minimal()&&self.notification_bell().is_none();
        if bar_alpha>0.004 && !minimal {
            let c=base.shifted(panel.min.x-base.origin.x,panel.min.y-base.origin.y).faded(bar_alpha);
            let c=if bar_alpha<0.8 {c.ghost()} else {c};
            self.side_bar(&c,panel.width(),panel.height(),env,resident_day);
        }
        if reveal>0.004 {self.shared_header(&base.faded(reveal),base.origin.x+PW/2.0,1.0,reveal,env,None);}
    }

    fn update_pet(&mut self, env: &Env, cx: f32, top:f32) {
        let cx=cx+self.resident.position(bar::PET);
        let idle = self.idle_for();
        if self.pet.last_idle >= self.opts.pet_idle && idle < 1.0
            && matches!(self.pet.state, PetState::Dozing | PetState::Sleeping) {
            self.pet.waking_until = self.t + 2.2;
        }
        self.pet.last_idle = idle;
        self.pet.hovered = env.cursor.is_some_and(|p| (p.x-cx).abs() <= BAR_W/2.0+24.0 && (top..top+70.0).contains(&p.y));
        self.pet.worried = self.settings.show_battery && env.battery.is_some_and(|v|v<=20) && !env.charging || env.network == Some(false);
        let allowed = self.pet_enabled();
        if !allowed { self.pet.reaction = None; self.pet.waking_until = 0.0; self.pet.expressions.clear(); }
        let available = allowed && !self.open && !self.drag_hot;
        let shown = available;
        self.pet_a = anim::approach(self.pet_a, if shown {1.0}else{0.0}, 9.0, self.dt);
        let visible = available && self.pet_a > 0.7 && self.progress() < 0.1;
        self.pet.advance(self.dt, visible);
        let state = self.pet_state();
        let gaze = env.cursor.map(|p| [((p.x-cx)/28.0).clamp(-3.0,3.0), ((p.y-top-15.0)/40.0).clamp(-1.0,1.5)])
            .unwrap_or([(self.t as f32 * 0.35).sin()*0.8, 0.0]);
        self.pet.step(state, self.t, self.dt, gaze, visible);
    }

    // ---------------------------------------------------------------
    // 面板主体：页标题 + 页面（带切页过渡）+ 翻页箭头 + 页码豆
    // ---------------------------------------------------------------
    fn shows_hold_open(&self)->bool {
        self.hold_open || self.page==SHELF_PAGE && self.calendar.draft.is_none() && self.task_dialog.is_none()
    }

    pub(crate) fn header_music_shift(&self)->f32 {
        if self.media.is_music() {28.0*smoothstep(0.0,0.65,self.hold_slot)}else{0.0}
    }

    fn draw_hold_open(&mut self,c:&Card) {
        // 声波先让出位置，再显示图钉；收回时按相反顺序。
        let alpha=smoothstep(if self.media.is_music(){0.65}else{0.0},1.0,self.hold_slot);
        if alpha<=0.001 {return;}
        let Some(button)=c.edge_control(354.0,5.0,25.0,26.0) else{return;};
        let visible=self.shows_hold_open();
        let button=button.faded(alpha);
        let button=if visible && alpha>0.8 {button}else{button.ghost()};
        let response=button.hit(354.0,5.0,25.0,26.0,"hold-open")
            .on_hover_text(if self.hold_open {"取消保持展开"}else{"保持展开"})
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        if response.clicked() {
            self.hold_open=!self.hold_open;
            self.hover_lock=false;
            self.leave_since=None;
            self.forced_open_until=0.0;
        }
        let (hover,press)=button.hover_t(&response);
        let active=button.ui.ctx().animate_bool_with_time(egui::Id::new("hold-open-active"),self.hold_open,0.12);
        let bg=paint::mix(Color32::from_white_alpha((hover*30.0) as u8),Color32::from_rgb(35,77,65),active);
        let fg=paint::mix(paint::mix(theme::GRAY,theme::WHITE,hover),Color32::from_rgb(181,244,220),active);
        button.circle(367.0,18.0,10.5,bg);
        paint::hold_pin(&button.scaled(367.0,18.0,1.0-press*0.1),367.0,18.0,active,fg);
    }

    fn panel_body(&mut self, c: &Card, env: &Env, cmds: &mut Vec<Cmd>) -> bool {
        let settings_card=if self.side() {c.edge_control(380.0,4.0,28.0,28.0)} else {Some(c.clone())};
        if let Some(c)=settings_card {
            let settings=c.hit(380.0,4.0,28.0,28.0,"page-settings").on_hover_text(format!("打开{}设置窗口",self.navigation_page_name(self.page)));
            let (hover,press)=c.hover_t(&settings);
            #[cfg(all(windows, feature = "sdk-material"))]
            if !self.glassy() || self.glass_controls_ready() {
                c.material_circle_with_role(&settings,394.0,18.0,13.0,crate::material::ControlRole::Gear);
            }
            if self.glassy() {
                let scale=self.glass_control_scale(&settings,press);
                if !self.glass_controls_ready() {
                    paint::glass_disc(&c.scaled(394.0,18.0,scale),394.0,18.0,13.0,0.12+hover*0.72);
                }
                paint::gear(&c.scaled(394.0,18.0,0.8*scale),394.0,18.0,theme::WHITE);
            } else {
                c.circle(394.0,18.0,13.0,Color32::from_white_alpha((hover*24.0) as u8));
                paint::gear(&c.scaled(394.0,18.0,0.8*(1.0-press*0.08)),394.0,18.0,paint::mix(theme::GRAY,theme::WHITE,hover));
            }
            if settings.clicked(){self.open_page_settings(self.page);}
        }
        if self.subpage==Some(self.page) {
            c.text(15.0,25.0,"‹",19.0,W::Bold,theme::WHITE,H::Left);
            // 健康提醒设置沿用主页的标题 + 喝水/久坐切换，切到哪边就设置哪边。
            if self.page==2 && self.health_time {c.text(31.0,25.0,"提醒时段",size::TITLE,W::Bold,theme::WHITE,H::Left);}
            else if self.page==2 {let sv=self.sit_view_t.pos.clamp(0.0,1.0);self.health_tabs(c,31.0,sv);}
            else {c.text(37.0,25.0,"专注设置",size::TITLE,W::Bold,theme::WHITE,H::Left);}
            // 「提醒时段」二级页的返回先回到健康提醒设置，再退出设置。
            if c.hit(9.0,7.0,28.0,23.0,"subpage-back").clicked(){
                if self.page==2 && self.health_time {self.health_time=false;} else {self.subpage=None;self.settings_editing=false;}
            }
            self.page_settings(c);self.nav(c);return false;
        }
        let e = self.page_e();
        let in_tx = e < PAGE_TURN_DURATION && self.page_from != self.page;
        if in_tx {
            // 旧页停在当前位置淡出，新页朝目标单向滑入。
            let old_a = Self::page_slide_alpha(self.page_from_elapsed) * (1.0 - smoothstep(0.0, 0.16, e));
            let new_a = Self::page_slide_alpha(e);
            if old_a > 0.0 && self.module_on(self.page_from) {
                let oc = c.faded(old_a).ghost();
                let old_header=c.faded(Self::page_header_alpha(self.page_from_elapsed)*(1.0-smoothstep(0.0,0.10,e))).ghost();
                self.page_title(self.page_from, &old_header);
                self.render_page(self.page_from, &oc.shifted(self.page_from_offset, 0.0), &old_header, env, &mut Vec::new());
            }
            let nc = c.faded(new_a);
            let nc = if e < 0.12 { nc.ghost() } else { nc };
            // 同一位置的标题先退后进，避免两个页面的汉字在中间帧叠成重影。
            let header=c.faded(Self::page_header_alpha(e));
            let header=if e<0.12 {header.ghost()}else{header};
            self.page_title(self.page, &header);
            self.render_page(self.page, &nc.shifted(self.page_slide_offset(e), 0.0), &header, env, cmds);
        } else {
            self.page_title(self.page, c);
            self.render_page(self.page, c, c, env, cmds);
        }
        self.nav(c);
        if self.page==0 && c.live {self.draw_music_volume(c,cmds);}
        in_tx
    }

    fn page_title(&self, page: usize, c: &Card) {
        let base = 25.0;
        #[cfg(feature="sdk-js")]
        if page>=PAGE_NAMES.len() {
            let accent=self.status_pages.get(page-PAGE_NAMES.len())
                .and_then(|(id,_)|self.plugins.status_title_accent(id));
            crate::plugin_status_ui::draw_title(
                c,self.navigation_page_name(page),accent,self.island_overlay_fill(),
            );
            return;
        }
        let (a, b, col) = match page {
            0 | CALENDAR_PAGE => return,
            1 => ("番茄", "专注", theme::RED_TOP),
            // 健康页的标题由页面自己画（「健康提醒」+ 喝水/久坐分段切换）。
            2 => return,
            // 置物架的标题就是「置物架 | 剪贴板」分栏，由页面自己画。
            3 => return,
            4 => ("待办", "清单", theme::GREEN),
            5 => ("邮箱", "", theme::CYAN),
            6 => ("AI", "额度", theme::SOFT_GREEN),
            8 if self.weather.solar_open => ("日出", "日落", theme::YELLOW),
            PERFORMANCE_PAGE => ("系统", "性能", crate::performance_ui::CPU_COLOR),
            8 => ("当地", "天气", theme::CYAN),
            // 详情页的标题带返回箭头，由页面自己画。
            BLUETOOTH_PAGE if self.wireless_focus.is_some() && self.wireless_devices.len()>1 => return,
            BLUETOOTH_PAGE => ("无线", "设备", theme::CYAN),
            _ => ("岛仔", "设置", theme::WHITE),
        };
        let wa = c.text(15.0, base, a, size::TITLE, W::Bold, theme::WHITE, H::Left);
        if !b.is_empty() {
            c.text(15.0 + wa + 1.5, base, b, size::TITLE, W::Bold, col, H::Left);
        }
    }

    fn nav(&mut self, c: &Card) {
        let all_pages:Vec<_>=self.navigation_pages().collect();
        if all_pages.len()<2 { return; }
        // 停靠方式决定箭头的锚点，切页改变面板高度时仍可原地点击。
        let anchor_y=68.0;
        let arrow_y=match self.dock() {
            model::Dock::Top=>anchor_y,
            model::Dock::Bottom=>c.to_local(c.painter.clip_rect().center_bottom()).y-(PH-anchor_y),
            model::Dock::Left|model::Dock::Right=>anchor_y+(self.page_height.pos-PH)/2.0,
        };
        for (x, direction, id) in [(21.0,-1.0,"nav-l"),(398.0,1.0,"nav-r")] {
            let arrow_card=if self.side() {c.edge_control(x-13.0,arrow_y-13.0,26.0,26.0)} else {Some(c.clone())};
            let Some(arrow_card)=arrow_card else {continue};
            let button=arrow_card.hit(x-13.0,arrow_y-13.0,26.0,26.0,id);
            let (hover,press)=arrow_card.hover_t(&button);
            #[cfg(all(windows, feature = "sdk-material"))]
            if !self.glassy() || self.glass_controls_ready() {
                let role=if direction>0.0 {crate::material::ControlRole::NavR} else {crate::material::ControlRole::NavL};
                let radius=if self.glassy() {12.0} else {9.0*(1.0-0.08*press)};
                arrow_card.material_circle_with_role(&button,x,arrow_y,radius,role);
            }
            if self.glassy() {
                paint::chevron_glass(&arrow_card,x,arrow_y,direction,hover,self.glass_control_scale(&button,press),self.glass_controls_ready());
            } else {
                paint::chevron(&arrow_card,x,arrow_y,direction,hover,press);
            }
            if button.clicked() {self.navigate_page(direction>0.0);}
        }
        let shortcuts = self.page_shortcuts_enabled();
        // 页数较多时，底栏随当前页移动，左右箭头和滚轮仍可切到每一页。
        let max_slots=if shortcuts {PAGE_NAMES.len()}else{40};
        let current=all_pages.iter().position(|&page|page==self.page).unwrap_or_default();
        let start=current.saturating_sub(max_slots/2).min(all_pages.len().saturating_sub(max_slots));
        let pages=&all_pages[start..(start+max_slots).min(all_pages.len())];
        let dot_width=|page:usize|self.dots.get(page).map_or(if page==self.page {16.0}else{4.0},|dot|dot.pos);
        let gap = if shortcuts { 3.0 } else { 4.0 };
        let dot_scale = if shortcuts { 0.75 } else { 1.0 };
        let dot_size = 4.0 * dot_scale;
        let dot_y = if shortcuts {
            self.page_height.pos - PAGE_SHORTCUT_ROW_H / 2.0
        } else {
            self.page_height.pos - 7.2
        };
        // 图标和页码点共用底部一行，按可见页面的数量整体居中。
        let dots_width = pages.iter().copied().map(dot_width).sum::<f32>() * dot_scale
            + gap * (pages.len() - 1) as f32;
        let shortcut_slot = 26.0;
        let group_gap = 14.0;
        let shortcut_width = if shortcuts { shortcut_slot * pages.len() as f32 + group_gap } else { 0.0 };
        let mut x = (PW - dots_width - shortcut_width) / 2.0;
        let mut clicked = None;
        if shortcuts {
            let row_width = dots_width + shortcut_width;
            // 只在整行已经露出时显示，避免展开过程点击穿到正文控件。
            let Some(row) = c.edge_control(x - 2.0, dot_y - 11.0, row_width + 4.0, 22.0) else { return; };
            for &page in pages {
                let center = x + shortcut_slot / 2.0;
                let button = row.hit(x, dot_y - 11.0, shortcut_slot, 22.0, ("page-shortcut", page))
                    .on_hover_text(self.navigation_page_name(page))
                    .on_hover_cursor(egui::CursorIcon::PointingHand);
                let (hover, press) = row.hover_t(&button);
                let (ink, bg) = if page == self.page {
                    let accent = self.active_page_shortcut_colors()
                        .and_then(|colors| PAGE_SHORTCUT_KEYS.get(page).and_then(|key|colors.get(*key)))
                        .map_or(theme::GRAY, |&[r, g, b]| Color32::from_rgb(r, g, b));
                    (paint::mix(accent, theme::WHITE, 0.45), paint::alpha(accent, 0.30))
                } else {
                    (paint::mix(Color32::from_gray(174), theme::WHITE, hover),
                        Color32::from_white_alpha((hover * 25.0) as u8))
                };
                row.fill(center - 10.5, dot_y - 9.0, 21.0, 18.0, 5.0, bg);
                let icon=row.scaled(center, dot_y, 1.0 - 0.08 * press);
                if page<PAGE_NAMES.len() {
                    paint::page_shortcut_icon(&icon, page, center, dot_y, ink);
                } else {
                    #[cfg(feature="sdk-js")]
                    let custom_icon = self.status_pages.get(page-PAGE_NAMES.len())
                        .and_then(|(id, _)| self.plugins.navigation_icon(id));
                    #[cfg(feature="sdk-js")]
                    let use_default_icon = if let Some(custom_icon) = custom_icon {
                        crate::plugin_canvas_ui::draw_navigation_icon(
                            &icon.painter, icon.rect(center-9.0,dot_y-9.0,18.0,18.0),
                            custom_icon.commands(), ink,
                        );
                        false
                    } else {
                        true
                    };
                    #[cfg(not(feature="sdk-js"))]
                    let use_default_icon = true;
                    if use_default_icon {
                        icon.seg((center-5.0,dot_y+4.0),(center-5.0,dot_y+1.0),1.7,ink);
                        icon.seg((center,dot_y+4.0),(center,dot_y-4.0),1.7,ink);
                        icon.seg((center+5.0,dot_y+4.0),(center+5.0,dot_y-1.0),1.7,ink);
                    }
                }
                if button.clicked() { clicked = Some(page); }
                x += shortcut_slot;
            }
            row.seg((x + group_gap / 2.0, dot_y - 6.0), (x + group_gap / 2.0, dot_y + 6.0), 1.0, Color32::from_gray(55));
            x += group_gap;
        }
        for &i in pages {
            let w = dot_width(i).max(3.0) * dot_scale;
            let on = anim::remap(w, 4.0 * dot_scale, 16.0 * dot_scale);
            let col = paint::mix(Color32::from_rgb(92, 92, 96), theme::WHITE, on);
            c.fill(x, dot_y - dot_size / 2.0, w, dot_size, dot_size / 2.0, col);
            let extra = gap / 2.0;
            let hr = c.hit(x - extra, dot_y - 6.0, w + gap, 12.0, ("dot", i));
            hr.clone().on_hover_text(self.navigation_page_name(i));
            if hr.hovered() && i != self.page {
                c.fill(x - 1.0, dot_y - dot_size / 2.0 - 1.0, w + 2.0, dot_size + 2.0, 3.0, Color32::from_white_alpha(28));
            }
            if hr.clicked() {
                clicked = Some(i);
            }
            x += w + gap;
        }
        if let Some(i) = clicked {
            let dir = if all_pages.iter().position(|&page|page==i).is_some_and(|index|index>current) {1.0}else{-1.0};
            self.go(i, dir);
        }
    }

    #[cfg(feature="sdk-js")]
    fn status_values_blocked(&self, env: &Env) -> bool {
        #[cfg(all(windows,feature="sdk-material"))]
        if self.material_capture_forced {return true;}
        let exclude=self.settings.exclude_from_capture || self.capture_exclusion_request==Some(true);
        crate::plugin_status_ui::sensitive_values_blocked(exclude,env)
    }

    fn render_page(&mut self, page: usize, c: &Card, header: &Card, env: &Env, cmds: &mut Vec<Cmd>) {
        #[cfg(feature="sdk-js")]
        if let Some((id,_))=page.checked_sub(PAGE_NAMES.len()).and_then(|index|self.status_pages.get(index)) {
            let backdrop=crate::plugin_status_ui::StatusBackdrop{
                palette:self.island_palette(),island:self.island_overlay_fill(),
            };
            let privacy_blocked=!self.open
                || self.status_values_blocked(env);
            let visible=self.open && !env.fullscreen_hidden && !env.screen_locked;
            let page_card=if visible {c.clone()} else {c.ghost()};
            let interaction=self.plugins.draw_status_controls(&page_card,id,backdrop,privacy_blocked);
            #[cfg(windows)]
            if let Some((label,url,time))=self.plugins.source_attribution(id) {
                let top=self.plugins.status_height(id);
                crate::plugin_status_ui::draw_attribution(c,id,top,label,url,time);
            }
            if c.live {
                self.plugin_form_focused=interaction.focused;
                if interaction.gained_focus {cmds.push(Cmd::Focus);}
                self.plugin_canvas_frame=Some((page,interaction));
            }
            return;
        }
        // 页面内容横向滑动，右侧操作只随切页淡入淡出，避免滑过图钉。
        let actions=header.shifted(-self.header_music_shift(),0.0);
        #[cfg(any(feature="sdk-js",feature="sdk-wasm"))]
        if page==4 && crate::sdk_spike::live_card(c){return;}
        match page {
            0 => self.page_music(c, env, cmds),
            1 => self.page_focus(c, header, env),
            2 => self.page_water(c, header, env),
            3 => self.page_shelf(c, header, env, cmds),
            4 => {self.page_todo(c, env, cmds);
                #[cfg(feature="sdk-js")]
                if self.plugins.card(c) {
                    self.settings_tab=PAGE_SETTINGS_TABS[TODO_PAGE];
                    self.open_settings();
                }
            }
            5 => self.page_mail(c, env, cmds),
            6 => self.page_quota(c,&actions),
            PERFORMANCE_PAGE => self.page_performance(c),
            8 => self.page_weather(c,&actions,env,cmds),
            BLUETOOTH_PAGE => self.page_wireless(c,env),
            CALENDAR_PAGE => self.page_calendar(c,header,env),
            _ => self.page_settings(c),
        }
    }

    fn toast_layer(&mut self, c: &Card, base_y:f32)->Option<Rect> {
        let Some((msg, at)) = self.toast.clone() else { return None };
        let e = (self.t - at) as f32;
        if e > 2.4 {
            self.toast = None;
            return None;
        }
        let a = smoothstep(0.0, 0.16, e) * (1.0 - smoothstep(2.1, 2.4, e));
        let rise = (1.0 - anim::ease_out_cubic(e / 0.3)) * 6.0;
        let msg=c.fit(&msg,size::BODY,W::Regular,310.0);
        let w = c.text_w(&msg, size::BODY, W::Regular) + 26.0;
        let tc = c.faded(a);
        let y = base_y + rise;
        tc.fill(PW / 2.0 - w / 2.0, y, w, 22.0, 11.0, Color32::from_rgb(44, 44, 48));
        tc.text(PW / 2.0, y + 15.0, &msg, size::BODY, W::Regular, theme::WHITE, H::Center);
        Some(tc.rect(PW/2.0-w/2.0,y,w,22.0))
    }

    // ---------------------------------------------------------------
    // 每帧推进：番茄钟、饮水提醒、跨天
    // ---------------------------------------------------------------
    fn tick(&mut self, env: &Env, dt: f32, elapsed: f32) {
        #[cfg(feature="d3d11-glass")]
        {
            static LIQUID_K: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            let luma = crate::gpu_glass::background_luma();
            let cur = f32::from_bits(LIQUID_K.load(std::sync::atomic::Ordering::Relaxed));
            let target = self.settings.liquid_theme.target(luma, if cur == 0.0 {1.0} else {cur});
            let k = cur + (target - cur) * (dt as f32 * 6.0).clamp(0.0, 1.0);
            LIQUID_K.store(k.to_bits(), std::sync::atomic::Ordering::Relaxed);
            crate::gpu_glass::set_theme_level(k);
            crate::gpu_glass::set_morph_scale(self.settings.liquid_morph);
            crate::gpu_glass::set_liquid_optics(self.settings.liquid_refract_px, self.settings.liquid_chroma_px);
            crate::gpu_glass::set_liquid_look(self.settings.liquid_blur_px, self.settings.liquid_band_px,
                self.settings.liquid_edge_w_px, self.settings.liquid_edge_op);
            crate::gpu_glass::set_light_follow(self.settings.liquid_light_follow);
            crate::gpu_glass::set_shadow(self.settings.liquid_shadow);
        }
        if self.toast.as_ref().is_some_and(|(_,at)|self.t-at>2.4){self.toast=None;}
        // 跳转后先用本地目标位置显示；播放器回报追上了、换歌了或超时就交还真实位置。
        let seek_done=self.music_seek_ui.as_ref().is_some_and(|(target,title)|{
            (self.media.duration>0.0 && (self.media.position-*target).abs()<1.5)
                || self.t>=self.music_seek_quiet_until || &self.media.title!=title
        });
        if seek_done {self.music_seek_ui=None;}
        if self.music_restart_at.is_some_and(|at|self.t>=at+1.5){self.music_restart_at=None;}
        if env.today != self.today {
            self.today = env.today.clone();
            self.water = model::Water::load(&env.today);
            self.last_drink=None;
            self.water_ml = self.water.drunk_ml as f32;
            self.water_ring.snap(self.water.percent() / 100.0);
        }
        #[cfg(feature="sdk-js")]
        let mut focus_finished=None;
        if self.pomo.running && !self.module_on(FOCUS_PAGE) {self.pomo.running=false;}
        if self.pomo.running {
            self.pomo.remaining -= elapsed;
            if self.pomo.remaining <= 0.0 {
                let next = match self.pomo.phase {
                    Phase::Focus => {
                        #[cfg(feature="sdk-js")]
                        {focus_finished=Some(self.pomo.configured_minutes);}
                        self.pet.react(PetState::Happy);
                        Phase::Rest
                    }
                    Phase::Rest => Phase::Focus,
                };
                self.pomo.phase = next;
                self.pomo.reset_remaining(self.phase_secs(next));
                self.pomo.running = false;
                self.alert("专注计时", format!("该{}了", next.label()), 1, true);
            }
        }
        self.ring_shown.target = (self.pomo.remaining / self.phase_secs(self.pomo.phase)).clamp(0.0, 1.0);
        self.ring_shown.step(dt);
        self.reset_spin.step(dt);
        self.todo_pop.step(dt);

        if self.module_on(WATER_PAGE) && self.water.remind && !self.health_pending[HealthReminder::Water as usize] && self.water.percent() < 100.0 && self.remind_window(env) && self.t - self.last_reminder >= self.settings.water_minutes as f64 * 60.0 {
            self.alert("喝水时间到了", format!("休息一下，喝 {} ml 水吧", self.settings.serving_ml), 2, true);
        }
        // 专注阶段不打断，等专注结束（通常正好进入休息）再提醒。
        let focusing = self.pomo.running && self.pomo.phase == Phase::Focus;
        if self.module_on(WATER_PAGE) && self.settings.sit_remind && !self.health_pending[HealthReminder::Sit as usize] && !focusing && self.remind_window(env) && self.t >= self.sit_due() {
            self.alert(SIT_TITLE, "起来走走，休息一下", 2, true);
        }
        let now = format!("{} {}", env.today, env.clock.get(..5).unwrap_or("00:00"));
        let todo_on=self.module_on(TODO_PAGE);
        #[cfg(feature="sdk-js")]
        {
            let plugin_start=std::time::Instant::now();
            let calendar_on=self.module_on(CALENDAR_PAGE);
            let quota_on=self.quota_active();
            self.plugins.tick_features(todo_on,calendar_on,quota_on,&env.today);
            if let Some(minutes)=focus_finished {
                self.plugins.focus_finished(chrono::Utc::now().timestamp(),minutes,todo_on,calendar_on,quota_on);
            }
            #[cfg(windows)]
            for notice in self.plugins.take_script_notices(self.settings.notifications,chrono::Utc::now().timestamp_millis()) {
                self.receive_plugin_notice(notice);
            }
            self.sync_status_pages();
            crate::resources::PLUGIN_FRAME_US.store(plugin_start.elapsed().as_micros() as usize,std::sync::atomic::Ordering::Relaxed);
        }
        let due: Vec<_> = self.todos.items.iter().enumerate().filter(|(_, item)| todo_on && item.reminder_due(&now)).map(|(i,item)| (i,item.text.clone())).collect();
        for (i,text) in due { self.todos.items[i].mark_reminded(&now); self.todos.save(); self.alert("待办到时间了", text, 4, true); }
        // v0.5.5 会议提醒：.ics 里的开始时间，默认提前 15 分钟（可设 5/15/30）。
        // v0.5.6 起日历（CalDAV）事件走同一套：同一场会（UID + DTSTART）只提醒一次，
        // 邮件和日历两条来源撞上了也只弹一次。
        let lead = self.settings.mail_ics as i64 * 60;
        let now_ts = chrono::Local::now().timestamp();
        let mut picks = Vec::new();
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        // 会议提醒点开后进入日程页；日程模块关闭时不再提醒（含邮件邀请）。
        let calendar_on=self.module_on(CALENDAR_PAGE);
        for entry in self.calendar.local.iter().filter(|_|calendar_on) {
            let Some(at)=entry.reminder_at() else{continue;};
            let key=calendar::dedupe_key(&entry.event);
            let cutoff=if entry.event.all_day{at+60}else{entry.event.start+60};
            if now_ts>=at && now_ts<cutoff && !self.mail.fired.contains(&key) && seen.insert(key.clone()) {
                picks.push((key,entry.event.clone(),"本机日程".into(),CALENDAR_PAGE));
            }
        }
        for (m,provider) in self.calendar_work_events(None).into_iter().filter(|_|calendar_on) {
            let left = m.start - now_ts;
            let key = calendar::dedupe_key(m);
            if !m.all_day && left <= lead && left > -60 && !self.mail.fired.contains(&key) && seen.insert(key.clone()) {
                let source=provider.map(|provider|provider.label()).unwrap_or("日历");
                picks.push((key, m.clone(), source.to_string(), CALENDAR_PAGE));
            }
        }
        if calendar_on && self.mail_active() {
            for m in self.mail.recent.iter().filter_map(|m| m.meeting.as_ref()).filter(|m|!self.mail_event_ignored(m)) {
                let left = m.start - now_ts;
                let key = calendar::dedupe_key(m);
                if !m.all_day && left <= lead && left > -60 && !self.mail.fired.contains(&key) && seen.insert(key.clone()) {
                    picks.push((key, m.clone(), "邮件邀请".to_string(), CALENDAR_PAGE));
                }
            }
        }
        for (key, meeting, source, page) in picks {
            self.mail.fired.insert(key);
            self.meeting_alert(meeting, source, page);
        }
        self.water_ring.target = self.water.percent() / 100.0;
        self.water_ring.step(dt);
        self.water_ml = anim::approach(self.water_ml, self.water.drunk_ml as f32, 9.0, dt);
        self.water_view_t.target = self.water_view as f32;
        self.water_view_t.step(dt);
        self.sit_view_t.target = if self.sit_mode { 1.0 } else { 0.0 };
        self.sit_view_t.step(dt);
        self.remind_t.target = if self.water.remind { 1.0 } else { 0.0 };
        self.remind_t.step(dt);
        self.sit_t.target = if self.settings.sit_remind { 1.0 } else { 0.0 };
        self.sit_t.step(dt);
        // 剪贴板记录同时受置物架模块和记录开关控制。
        if self.clipboard.is_enabled() != self.clipboard_active() {
            self.clipboard.set_enabled(self.clipboard_active());
        }
        self.shelf_scroll.target = self.shelf_first as f32;
        self.shelf_scroll.step(dt);
        if let Some(at) = self.confirm_clear {
            if self.t - at > 3.0 {
                self.confirm_clear = None;
            }
        }
    }

    /// 媒体进度在两次 SMTC 刷新之间按时间外推，进度条才会连续走动。
    fn update_media(&mut self, env: &Env, ctx: &egui::Context) {
        let m = &env.media;
        if !self.module_on(MUSIC_PAGE) || !m.is_music() {
            self.music_seek_ui=None;
            self.music_restart_at=None;
            self.music_volume_ui=None;
            self.music_volume_quiet_until=0.0;
            self.music_volume_open=false;
            self.media=NowPlaying::default();
            self.media_anchor=(String::new(),0.0,0.0,false);
            self.music_artwork=None;
            self.ly=LyricsState::default();
            return;
        }
        let a = &self.media_anchor;
        if a.0 != m.title || (a.1 - m.position).abs() > 0.01 || a.3 != m.playing {
            self.media_anchor = (m.title.clone(), m.position, env.t, m.playing);
        }
        let mut np = m.clone();
        if self.media.source!=m.source || m.volume.is_none(){self.music_volume_open=false;}
        if self.media.source!=m.source || self.t>=self.music_volume_quiet_until || m.volume.is_none() {
            self.music_volume_ui=m.volume;
        }
        if m.playing {
            let (_, pos, _, _) = &self.media_anchor;
            np.position = if let Some(sampled)=m.sampled_at {m.position+sampled.elapsed().as_secs_f64()}
                else {*pos};
            if m.duration>0.0 {np.position=np.position.min(m.duration);}
        }
        // 网易云的 song_id 只来自本机日志，SMTC 那一路没有；None 与 Some 之间来回不是换歌，不能每次都清空歌词重查。
        let other_song=matches!((self.media.song_id,np.song_id),(Some(old),Some(new)) if old!=new);
        if self.media.source != np.source || other_song { self.ly=LyricsState::default(); }
        self.media = np;
        // 播放器版本比已适配的旧、歌词无法同步：岛下浮出一句提示（与网易云不支持跳转的提示同一样式），同一播放器版本只提醒一次。
        #[cfg(windows)]
        if let Some(message)=crate::smtc::music_compat::take_update_notice() {self.say(message);self.toast_below=true;}
        if let Some(image)=&self.media.artwork {
            if !self.music_artwork.as_ref().is_some_and(|(old,_)|std::sync::Arc::ptr_eq(old,image)) {
                self.music_artwork=Some((image.clone(),ctx.load_texture("music-artwork",image.clone(),egui::TextureOptions::LINEAR)));
            }
        } else {self.music_artwork=None;}
    }

    fn poll_lyrics(&mut self) {
        if !self.media.is_music(){self.ly=LyricsState::default();return;}
        if !self.show_lyrics && !self.settings.island_lyrics {self.ly=LyricsState::default();return;}
        let np = &self.media;
        if np.title.is_empty(){self.ly=LyricsState::default();return;}
        let key = format!("{}|{}|{:.0}", np.title, np.artist, np.duration);
        let id_arrived=key==self.ly.key && !self.ly.loading && self.ly.song_id.is_none() && np.song_id.is_some()
            && !self.ly.lyrics.as_ref().is_some_and(|lyrics|lyrics.synced);
        if key != self.ly.key {
            let change=self.ly.song.as_ref().map_or(SongChange::Different,|song|song_change(song,np));
            let synced=self.ly.lyrics.as_ref().is_some_and(|lyrics|lyrics.synced);
            match change {
                // 时长、写法补全：已有同步歌词就沿用，不清空、不重查。
                SongChange::Same if synced=>{self.ly.key=key;self.ly.song=Some(LyricSong::of(np));}
                // 换歌：立即切换显示状态，旧歌词和迟到的旧结果都不再显示。
                SongChange::Different=>self.start_lyrics(key,None,false),
                // 同一首的补全：重新查询，期间继续显示已有歌词。
                _=>{let keep=self.ly.lyrics.take();self.start_lyrics(key,keep,false);}
            }
        } else if id_arrived || self.ly.retry_at.is_some_and(|at|self.t>=at) {
            self.start_lyrics(key,None,true);
        }
        self.spawn_lyrics_job();
        if let Some(rx)=&self.ly.native_rx {
            let mut found=None;
            while let Ok(result)=rx.try_recv(){if result.is_some(){found=result;}}
            if let Some(lyrics)=found {
                self.ly.lyrics=Some(lyrics);self.ly.loading=false;self.ly.error=None;
                // 本机歌词已到：排队中的在线查询不再需要，进行中的结果也丢弃。
                self.ly.retry_at=None;self.ly.rx=None;self.ly.native_rx=None;self.ly.pending=None;
            }
        }
        if let Some(rx) = &self.ly.rx {
            if let Ok(r) = rx.try_recv() {
                self.ly.loading = false;
                match r {
                    Ok((l,waiting_native)) => {
                        self.ly.error=None;
                        self.ly.retry_at=None;
                        if (waiting_native || lyrics::is_soda(&self.media.source) && !l.synced) && self.ly.attempts<NATIVE_RETRIES {self.ly.retry_at=Some(self.t+5.0);}
                        self.ly.lyrics = Some(l);
                    },
                    Err(error) => {
                        let native=lyrics::is_soda(&self.media.source) || self.media.source.to_ascii_lowercase().contains("kugou") && error==lyrics::FetchError::NotFound;
                        if native {if self.ly.attempts<NATIVE_RETRIES{self.ly.retry_at=Some(self.t+5.0);}}
                        else if matches!(error,lyrics::FetchError::Unavailable(_)|lyrics::FetchError::InvalidResponse(_)) && self.ly.attempts<NETWORK_RETRIES {self.ly.retry_at=Some(self.t+30.0);}
                        self.ly.error = Some(error);
                    },
                }
                self.ly.rx = None;
            }
        }
    }

    /// 为当前歌曲建立新的歌词状态：本机歌词轮询立即开始，在线查询交给 `spawn_lyrics_job`。
    /// `keep`：同一首歌信息补全时继续显示的歌词；`retry`：同一首歌的定时重试或补到歌曲 ID。
    fn start_lyrics(&mut self,key:String,keep:Option<lyrics::Lyrics>,retry:bool) {
        let np=&self.media;
        let kugou=np.source.to_ascii_lowercase().contains("kugou");
        let native_only=retry && kugou && self.ly.error==Some(lyrics::FetchError::NotFound);
        let cached=if retry && kugou {self.ly.lyrics.take()}else{None};
        let attempts=if retry {self.ly.attempts.saturating_add(1)}else{0};
        let manual=self.ly.manual_retry;
        let shown=cached.clone().or(keep);
        self.ly = LyricsState { key, loading: shown.is_none() && !native_only, lyrics:shown,
            error:native_only.then_some(lyrics::FetchError::NotFound), song_id:np.song_id, attempts,
            song:Some(LyricSong::of(np)), ..Default::default() };
        let (source, title, artist, dur) = (np.source.clone(), np.title.clone(), np.artist.clone(), np.duration);
        let song_id=np.song_id;
        if kugou || lyrics::is_soda(&source) {
            let (tx,rx)=std::sync::mpsc::sync_channel(1);self.ly.native_rx=Some(rx);
            let (source,title,artist)=(source.clone(),title.clone(),artist.clone());
            let native=crate::resources::NATIVE_LYRICS.spawn(move ||{
                // 客户端通常在开播几秒内写好歌词：先密后疏（0.5s 起，最长 5s 一次），最多等 90 秒。
                // 汽水的缓存文件可达 32MB，修改时间和大小都没变时不重复整份读取，但每 15 秒仍完整核对一次。
                let started=std::time::Instant::now();
                let mut wait=std::time::Duration::from_millis(500);
                let mut last:Option<((std::time::SystemTime,u64),std::time::Instant)>=None;
                let mut miss="未读取";
                loop {
                    let print=lyrics::native_fingerprint(&source);
                    let unchanged=print.is_some_and(|print|last.is_some_and(|(seen,at)|seen==print && at.elapsed()<std::time::Duration::from_secs(15)));
                    let found=if unchanged {None}else{
                        if let Some(print)=print {last=Some((print,std::time::Instant::now()));}
                        lyrics::read_native_detail(&source,&title,&artist,dur).map_err(|reason|miss=reason).ok()
                    };
                    // 找到歌词时等界面取走再结束（换歌或关闭歌词后通道断开会立即返回），不因通道暂满丢掉结果。
                    if found.is_some() {
                        if tx.send(found).is_ok(){lyrics::log_native(&source,&title,&artist,"命中",started.elapsed());}
                        break;
                    }
                    if let Err(std::sync::mpsc::TrySendError::Disconnected(_))=tx.try_send(None){break;}
                    if started.elapsed()>=std::time::Duration::from_secs(90){lyrics::log_native(&source,&title,&artist,&format!("放弃:{miss}"),started.elapsed());break;}
                    std::thread::sleep(wait);
                    wait=(wait*2).min(std::time::Duration::from_secs(5));
                }
            });
            // 快速切歌时本机轮询线程已满：不留空通道，之后的定时重试会再次启动。
            if native.is_none(){self.ly.native_rx=None;}
        }
        self.ly.pending=Some(LyricsJob{source,title,artist,duration:dur,song_id,cached,native_only,manual});
    }

    /// 旧请求结束后再发起排队的在线查询：歌曲信息抖动或快速切歌时不会叠加网络线程，
    /// 也只会查询最新的那首；旧请求的结果因通道已更换而直接丢弃。
    fn spawn_lyrics_job(&mut self) {
        if self.ly.pending.is_none() || self.ly_worker.as_ref().is_some_and(|worker|!worker.is_finished()) {return;}
        let Some(lease)=crate::resources::LYRICS.acquire() else {return;};
        let Some(job)=self.ly.pending.take() else {return;};
        let (tx, rx) = std::sync::mpsc::channel();
        self.ly.rx = Some(rx);
        self.ly_worker=lease.spawn(move || {
            let LyricsJob{source,title,artist,duration,song_id,cached,native_only,manual}=job;
            let result=if native_only {
                lyrics::kugou_cache::read(&title,&artist,duration).map(|lyrics|(lyrics,false)).ok_or(lyrics::FetchError::NotFound)
            }else{lyrics::fetch_for_display_with(&source, &title, &artist, duration, song_id, cached, manual)};
            let _ = tx.send(result);
        });
        if self.ly_worker.is_none(){self.ly.rx=None;self.ly.loading=false;self.ly.key.clear();}
    }

    /// 外部直接提供歌词（录制演示用）。
    pub fn set_lyrics(&mut self, title: &str, artist: &str, duration: f64, l: lyrics::Lyrics) {
        self.ly = LyricsState { key: format!("{title}|{artist}|{duration:.0}"), lyrics: Some(l), ..Default::default() };
    }

    pub fn on_drop(&mut self, files: Vec<PathBuf>) {
        if !self.settings.visible_pages[3] || files.is_empty() { return; }
        self.drag_hot = false;
        self.drag_departing = false;
        self.drag_saved = None;
        if self.page != 3 {
            self.go(3, 1.0);
        }
        let (mut added,mut duplicate,mut failed) = (0,0,0);
        for p in files {
            if !p.exists() {failed+=1;continue;}
            if self.shelf.items.contains(&p) {
                duplicate+=1;
                continue;
            }
            if self.shelf.add(&p).is_ok() {
                self.shelf_born.insert(p.clone(), self.t);
                added += 1;
            }else{failed+=1;}
        }
        let message=if added==0 && failed==0 {
            if duplicate==1{"该文件已在置物架中".into()}else{"这些文件已在置物架中".into()}
        }else if added==0 && duplicate==0 {"文件不存在或无法访问".into()}
        else {
            let mut parts=Vec::new();
            if added>0{parts.push(format!("已添加 {added} 个文件"));}
            if duplicate>0{parts.push(format!("{duplicate} 个已存在"));}
            if failed>0{parts.push(format!("{failed} 个无法添加"));}
            parts.join("，")
        };
        self.say(message);
        if added>0 {
            self.pet.react(PetState::Happy);
            self.shelf_first = self.shelf.items.len().saturating_sub(5);
        }
    }

    /// 待办输入框：真正的 egui 文本框，放在待办卡片顶部那一行。
    fn input_field(&mut self, ui: &mut egui::Ui, origin: Pos2, cmds: &mut Vec<Cmd>) {
        let rect = Rect::from_min_size(origin + Vec2::new(222.0, 37.0), Vec2::new(142.0, 18.0));
        let mut buf = std::mem::take(&mut self.buf);
        let te = egui::TextEdit::singleline(&mut buf)
            .id(egui::Id::new("todo-input"))
            .frame(false)
            .margin(egui::Margin::ZERO)
            .text_color(theme::WHITE)
            .font(theme::font(size::BODY, W::Regular))
            .hint_text(egui::RichText::new("要做点什么？回车保存").color(theme::DIM).font(theme::font(size::BODY, W::Regular)))
            .desired_width(rect.width());
        let resp = ui.put(rect, te);
        let first_focus=!self.todo_input_focused;
        self.buf=buf;
        let lost_window=ui.input(|i|i.events.iter().any(|event|matches!(event,egui::Event::WindowFocused(false))));
        if !first_focus && !ui.ctx().viewport(|viewport|viewport.ime_owns_keys) && self.adding && self.buf.trim().is_empty() && (resp.clicked_elsewhere() || lost_window) {
            resp.surrender_focus();
            self.cancel_edit();
            return;
        }
        if first_focus {
            self.todo_input_focused=true;
            resp.request_focus();
            cmds.push(Cmd::Focus);
        }
        let submit = ui.input(|i| i.key_pressed(egui::Key::Enter));
        if submit {
            let text = self.buf.trim().to_string();
            let mut added=false;
            if let Some(i) = self.editing {
                if !text.is_empty() && i < self.todos.items.len() {
                    self.todos.items[i].text = text;
                    self.todos.save();
                }
            } else if !text.is_empty() {
                self.todo_first = 0;
                self.todos.items.insert(0, model::TodoItem::new(text));
                self.todos.save();
                self.todo_pop.vel += 6.0;
                added=true;
            }
            self.cancel_edit();
            if added {self.open_task_reminder(0);}
        }
    }
}
