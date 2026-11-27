//! 材质包 D3D11 接口预留。
//!
//! 上游将开放集市材质入口；届时在 `snapshot()` 返回材质包 DXBC 与 generation、
//! 在 `active_params()` 返回 shader 参数。当前占位：无材质包 → 用内置 HLSL 玻璃。

/// (generation, DXBC 字节, 插件 id)；None = 无外部材质包。
pub fn snapshot() -> Option<(u64, Vec<u8>, String)> { None }

/// 当前材质参数（f32 序列）；占位为空。
pub fn active_params() -> Vec<f32> { Vec::new() }
