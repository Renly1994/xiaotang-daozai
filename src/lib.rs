//! 岛仔：Windows 顶部「灵动岛」。
//!
//! - `app` / `pages`：与平台无关的界面与动效（外壳与录制器共用）
//! - `anim` / `paint` / `shape` / `theme`：弹簧、矢量绘制、面板轮廓、配色字体
//! - `model` / `lyrics` / `smtc` / `icons`：数据、歌词、系统媒体、文件图标
//! - `native` / `tray`：仅 Windows 的窗口区域、命中穿透与托盘

#[cfg(any(feature="sdk-js",feature="sdk-wasm"))]
#[path="../spike/mod.rs"]
pub mod sdk_spike;
pub mod display_layout;
pub mod diagnostics_tool;
/// 材质包接口预留：d3d11-glass + sdk-js 下提供 material_d3d11 占位实现。
#[cfg(feature="sdk-js")]
pub mod local_plugins;
pub mod rtl_text;
mod mail_privacy;
#[cfg(windows)]
mod plugin_notice;
#[cfg(windows)]
mod plugin_notice_settings;
#[cfg(windows)]
mod plugin_notice_ui;
pub mod poll_clock;
pub mod resources;
pub mod anim;
pub mod oled_shift;
pub mod app;
pub mod messenger;
pub mod child_process;
mod settings_window;
pub mod bt;
mod bluetooth_ui;
pub mod calendar;
mod calendar_ui;
mod local_calendar;
mod lunar_calendar;
mod calendar_editor;
pub mod glass;
pub mod skin_paint;
pub mod icons;
pub mod input;
pub mod lyrics;
pub mod mail;
pub mod mail_format;
mod mail_reader;
pub mod mail_manage;
mod mail_manage_ui;
pub mod clipboard;
pub mod side_dock;
mod task_picker;
pub mod model;
pub mod renderer;
#[cfg(all(windows, feature = "sdk-material"))]
pub mod material;
pub mod otp;
pub mod quota;
#[cfg(feature = "sdk-js")]
mod quota_plugin;
#[cfg(feature = "sdk-js")]
mod plugin_canvas;
#[cfg(feature = "sdk-js")]
mod plugin_canvas_ui;
#[cfg(feature = "sdk-js")]
mod plugin_setting_catalog;
#[cfg(feature = "sdk-js")]
mod plugin_status;
#[cfg(feature = "sdk-js")]
mod plugin_status_ui;
mod quota_ui;
pub mod weather;
mod weather_ui;
mod weather_source_ui;
mod qweather;
mod pages;
mod extras;
mod pet;
mod pet_expressions;
mod bar_layout;
pub mod performance;
mod performance_ui;
mod resident_settings;
mod resident_date;
mod avoidance;
mod focus_bar;
#[cfg(windows)]
pub mod backdrop;
#[cfg(windows)]
pub mod composition;
#[cfg(windows)]
pub mod layered;
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
mod supporters;
#[cfg(windows)]
pub use updates::record_install_size;

#[cfg(windows)]
pub mod native;
// 可选 Windows 原生 D3D11 渲染后端（默认关闭，上游 eframe/glow/wgpu 路径不变）。
#[cfg(all(windows, feature = "d3d11-glass"))]
pub mod gpu_glass_stub;
#[cfg(all(windows, feature = "d3d11-glass"))]
#[path = "gpu_glass_stub.rs"]
pub mod gpu_glass;
#[cfg(all(windows, feature = "d3d11-glass"))]
pub mod wgc;
#[cfg(all(windows, feature = "d3d11-glass"))]
pub mod d3d11;
#[cfg(all(windows, feature = "d3d11-glass"))]
pub mod wallpaper;
#[cfg(windows)]
mod window_bounds;
#[cfg(windows)]
mod crop_front_buffer;
#[cfg(windows)]
pub mod wechat;
#[cfg(windows)]
pub mod tray;

#[cfg(test)]
mod ui_tests;
#[cfg(test)]
#[path = "bin/daozai-record/raster.rs"]
mod preview_raster;
