//! 岛仔：Windows 顶部「灵动岛」。
//!
//! - `app` / `pages`：与平台无关的界面与动效（外壳与录制器共用）
//! - `anim` / `paint` / `shape` / `theme`：弹簧、矢量绘制、面板轮廓、配色字体
//! - `model` / `lyrics` / `smtc` / `icons`：数据、歌词、系统媒体、文件图标
//! - `native` / `tray`：仅 Windows 的窗口区域、命中穿透与托盘

// egui 的 `Stroke::new(width: impl Into<f32>, …)` 收泛型参数，`Stroke::new(1.0, …)`
// 里的字面量只能 fallback 到 f32 才能选实现——这正是我们要的类型。上游 egui 自己
// 也有二十多处同样写法，等它改签名前先关掉这条提示，免得真问题被淹掉。
#![allow(float_literal_f32_fallback)]
// 缓存键、回调签名、绘制参数里确实有「多层元组」，它们就是从数据本身长出来的形状：
// `(Arc<str>, Arc<str>, u32, Arc<str>)` 是一个邮件缓存的真实复合键，起别名只是把复杂度
// 换个地方藏，读代码时还得跳过去看定义。逐处 allow 也只会增加噪声，统一在这里关掉。
#![allow(clippy::type_complexity)]
// 本项目习惯把短语句压在一行（`if a{..} if b{..}`），前后两个 if 是并列而不是漏了
// else；clippy 在压缩写法上分辨不出来，六处全是误报，逐处加花括号只是把警告换成换行。
#![allow(clippy::possible_missing_else)]
// 这 195 条全部来自第三方宏展开：`imap-proto` 依赖的 `nom 5.1.3` 宏里带多余分号，
// 展开点落在我们的 `mail.rs`/`mail_manage.rs` 里，改不了。等 nom 升到 7 会自动消失。
#![allow(semicolon_in_expressions_from_non_local_macros)]

pub mod display_layout;
mod mail_privacy;
mod mail_view;
pub mod poll_clock;
pub mod resources;
pub mod anim;
pub mod oled_shift;
pub mod app;
pub mod messenger;
pub mod child_process;
mod settings_window;
pub mod bt;
pub mod calendar;
mod calendar_ui;
mod local_calendar;
mod calendar_editor;
pub mod glass;
pub mod skin_paint;
pub mod icons;
pub mod input;
pub mod lyrics;
pub mod mail;
pub mod mail_format;
/// 上游的 HTML→文本行（`Line`/`Kind`），「阅读全文」窗口用。
/// 我们的 HTML→块（`Doc`/`Block`），岛内详情卡与「完整排版」用。
pub mod mail_doc;
mod mail_body_ui;
pub mod mail_image;
mod mail_render;
mod mail_tuning;
mod mail_html_view;
#[cfg(windows)]
mod mail_webview;
mod mail_reader;
pub mod mail_manage;
mod mail_panel;
mod mail_manage_ui;
pub mod clipboard;
pub mod side_dock;
mod task_picker;
pub mod model;
pub mod renderer;
pub mod otp;
pub mod quota;
mod quota_ui;
pub mod weather;
mod weather_ui;
mod pages;
mod extras;
mod pet;
mod pet_expressions;
mod bar_layout;
mod resident_settings;
mod avoidance;
mod focus_bar;
#[cfg(windows)]
pub mod backdrop;
#[cfg(windows)]
pub mod composition;
#[cfg(windows)]
pub mod system;
pub mod paint;
pub mod shape;
mod fluid;
pub mod smtc;
pub mod music_volume;
pub mod theme;
pub mod settings_theme;
mod updates;
#[cfg(windows)]
pub use updates::record_install_size;

#[cfg(windows)]
pub mod native;
#[cfg(windows)]
pub mod wechat;
#[cfg(windows)]
pub mod tray;

#[cfg(test)]
mod ui_tests;
#[cfg(test)]
#[path = "bin/daozai-record/raster.rs"]
mod preview_raster;
