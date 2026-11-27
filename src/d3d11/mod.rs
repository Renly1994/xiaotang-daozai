//! D3D11 自绘渲染栈（从 egui-directx11 移植到 windows 0.58）。
//! 单 device：WGC 抓屏 → HLSL 玻璃 → egui widgets → Present → DComp Commit。
#[cfg(windows)]
pub mod renderer;
#[cfg(windows)]
mod texture;
#[cfg(windows)]
pub mod wgc_capture;
#[cfg(windows)]
pub mod runtime;
