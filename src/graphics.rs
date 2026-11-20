//! OpenGL 初始化失败时使用独立的图形后端。
//!
//! 典型场景：笔记本接扩展坞 / DisplayLink 外接屏、远程桌面、显卡驱动缺失时，窗口所在的显示设备
//! 只有 Windows 自带的 OpenGL 1.1（GDI Generic），egui_glow 报「requires OpenGL 2.0+」。
//! 这时改用 Direct3D 12 重新启动，不需要用户拔扩展坞。

/// 兼容图形后端（低功耗显卡优先）：指定 Direct3D 12 或 Vulkan。
fn compatible(mut options:eframe::NativeOptions,backends:eframe::wgpu::Backends)->eframe::NativeOptions {
    options.renderer=eframe::Renderer::Wgpu;
    if let eframe::egui_wgpu::WgpuSetup::CreateNew(setup)=&mut options.wgpu_options.wgpu_setup {
        setup.instance_descriptor.backends=backends;
        setup.power_preference=eframe::wgpu::PowerPreference::LowPower;
        // 缓冲分配策略：实测改用 MemoryHints::Manual（小内存块池 8–64MB）。
        // dhat+VMMap A/B（同为新鲜进程、展开态悬停 15s 后测，2026-11-17）：
        //   Performance（wgpu 默认，大块池）：工作集 303MB / 私有提交 600MB
        //   MemoryUsage（精确分配）：反而比 Performance 多 ~85MB 提交——
        //     egui 每帧重建流式缓冲，精确分配产生大量 D3D 缓冲对象，驱动侧
        //     固定开销超过省下的池化空间
        //   Manual（8–64MB）：工作集 230MB / 私有提交 288MB —— 提交砍半、
        //     工作集 −24%，本负载资源集合小且稳定，块开销可忽略
        // 收起深休眠时另有工作集裁剪（见 resources::trim_working_set），
        // 任务管理器内存列从 ~230MB 掉到 ~30MB。
        // 回退开关：DAOZAI_WGPU_MEMHINTS=performance 用回 wgpu 默认。
        setup.device_descriptor=std::sync::Arc::new(|adapter| {
            let base_limits=if adapter.get_info().backend==eframe::wgpu::Backend::Gl {
                eframe::wgpu::Limits::downlevel_webgl2_defaults()
            }else{
                eframe::wgpu::Limits::default()
            };
            eframe::wgpu::DeviceDescriptor {
                label:Some("daozai wgpu device (Manual 8-64MB)"),
                required_features:eframe::wgpu::Features::default(),
                // 上游 v0.6.65 的优化一并吸收：DX12 默认按绑定上限（100 万项）预建
                // 描述符堆约 61MB，压到 65536 项只占 ~4MB，岛仔绑定数远用不到上限。
                required_limits:eframe::wgpu::Limits{
                    max_texture_dimension_2d:8192,
                    max_non_sampler_bindings:65_536,
                    ..base_limits
                },
                memory_hints:match std::env::var("DAOZAI_WGPU_MEMHINTS").as_deref().map(str::trim) {
                    Ok(v) if v.eq_ignore_ascii_case("memory_usage")=>eframe::wgpu::MemoryHints::MemoryUsage,
                    Ok(v) if v.eq_ignore_ascii_case("performance")=>eframe::wgpu::MemoryHints::Performance,
                    _=>eframe::wgpu::MemoryHints::Manual {
                        suballocated_device_memory_block_size:(8*1024*1024)..(64*1024*1024),
                    },
                },
            }
        });
    }
    options
}
/// 岛仔同时存在的纹理只有几十张。DX12 会按绑定上限预建整张描述符表：
/// 默认 100 万项约 61MB，65536 项约 4MB，仍有上千倍余量。
#[cfg(test)]
const MAX_BINDINGS:u32=65_536;
#[cfg(test)]
fn small_memory_blocks(descriptor:eframe::wgpu::DeviceDescriptor<'static>)->eframe::wgpu::DeviceDescriptor<'static>{
    let required_limits=eframe::wgpu::Limits{max_non_sampler_bindings:MAX_BINDINGS,..descriptor.required_limits.clone()};
    eframe::wgpu::DeviceDescriptor{memory_hints:eframe::wgpu::MemoryHints::MemoryUsage,required_limits,..descriptor}
}

/// 兼容渲染优先用 Direct3D 12（Windows 原生、睡眠唤醒与换屏处理最稳）；
/// 设备不支持时再退到 Vulkan。两者都不经过显卡的 OpenGL 驱动。
fn launch_compatible(options:eframe::NativeOptions,launch:&mut impl FnMut(eframe::NativeOptions)->eframe::Result<()>)->eframe::Result<()> {
    // 只有走 DX12 时才给窗口去掉 GDI 重定向表面；Vulkan/OpenGL 仍靠它显示内容。
    // 失败退回 Vulkan/OpenGL 前必须恢复环境变量，否则新窗口会没有可绘制表面。
    // 而且要先探一次 DComp：这个样式只能在建窗口时给，探不通还去掉表面，
    // 就只能整进程重开——预检把它变成“一开始就别去掉”。
    let saved=std::env::var_os("DAOZAI_DCOMP");
    if eframe::egui_wgpu::winit::direct_composition_available() {
        std::env::set_var("DAOZAI_DCOMP","1");
    } else {
        std::env::remove_var("DAOZAI_DCOMP");
    }
    let first=launch(compatible(options.clone(),eframe::wgpu::Backends::DX12));
    match saved {Some(value)=>std::env::set_var("DAOZAI_DCOMP",value),None=>std::env::remove_var("DAOZAI_DCOMP")}
    match first {
        Err(eframe::Error::Wgpu(_))=>launch(compatible(options,eframe::wgpu::Backends::VULKAN)),
        result=>result,
    }
}

/// 验收与排障：设置环境变量 `DAOZAI_RENDERER=wgpu` 直接使用兼容图形后端，
/// 在正常电脑上也能检查 OpenGL 失败后用户会看到的画面。
fn forced_compatible()->bool {
    std::env::var("DAOZAI_RENDERER").is_ok_and(|value|value.trim().eq_ignore_ascii_case("wgpu"))
}

pub fn run(options:eframe::NativeOptions,launch:impl FnMut(eframe::NativeOptions)->eframe::Result<()>)->eframe::Result<()> {
    // 上次崩在显卡 OpenGL 驱动里时，崩溃处理留下的标记会在这里转成设置。
    let _=daozai::renderer::apply_pending_fallback();
    // `DAOZAI_DCOMP` 只在 DX12 尝试期间有效，却活在进程环境里：关掉兼容渲染后重启时，
    // 新进程会把它继承下来，窗口被去掉 GDI 表面，而 OpenGL 正是靠这个表面显示 ——
    // 结果就是「不开 Direct3D 时岛体完全不出现」。启动时先清掉，需要时由 launch_compatible 再设。
    std::env::remove_var("DAOZAI_DCOMP");
    run_with(forced_compatible() || daozai::renderer::compat_requested(),options,launch)
}

fn opengl(mut options:eframe::NativeOptions)->eframe::NativeOptions { options.renderer=eframe::Renderer::Glow; options }

fn run_with(compat_first:bool,options:eframe::NativeOptions,mut launch:impl FnMut(eframe::NativeOptions)->eframe::Result<()>)->eframe::Result<()> {
    if compat_first {
        // 兼容渲染是被要求的（设置、崩溃标记或环境变量），但这台电脑可能根本没有 DX12 / Vulkan 设备
        // （远程桌面、虚拟机、只有基本显示适配器、老显卡）。这时退回 OpenGL，不让用户每次都打不开。
        daozai::renderer::set_compat_active(true);
        let error=match launch_compatible(options.clone(),&mut launch) {
            Err(error @ eframe::Error::Wgpu(_))=>error,
            result=>return result,
        };
        daozai::renderer::set_compat_active(false);
        return launch(opengl(options)).map_err(|fallback|eframe::Error::AppCreation(format!(
            "兼容图形后端无法启动：{error}\nOpenGL 也无法启动：{fallback}\n请安装电脑厂商提供的显卡驱动后重试。"
        ).into()));
    }
    let options=opengl(options);
    let fallback_options=options.clone();
    let error=match launch(options) {
        Ok(())=>return Ok(()),
        Err(error @ (eframe::Error::OpenGL(_)|eframe::Error::Glutin(_)|eframe::Error::NoGlutinConfigs(..)))=>error,
        Err(error)=>return Err(error),
    };
    daozai::renderer::set_compat_active(true);
    launch_compatible(fallback_options,&mut launch).map_err(|fallback|eframe::Error::AppCreation(format!(
        "OpenGL 初始化失败：{error}\n兼容图形后端也无法启动：{fallback}\n请安装电脑厂商提供的显卡驱动后重试。"
    ).into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opengl_20_failure_retries_without_opengl(){
        let mut attempts=0;
        let result=run_with(false,eframe::NativeOptions::default(),|options|{
            attempts+=1;
            if attempts==1 {
                assert_eq!(options.renderer,eframe::Renderer::Glow);
                return Err(eframe::Error::OpenGL("egui_glow requires OpenGL 2.0+.".to_owned().into()));
            }
            assert_eq!(options.renderer,eframe::Renderer::Wgpu);
            let eframe::egui_wgpu::WgpuSetup::CreateNew(setup)=options.wgpu_options.wgpu_setup else{panic!("应创建兼容图形设备")};
            assert!(!setup.instance_descriptor.backends.contains(eframe::wgpu::Backends::GL));
            Ok(())
        });
        assert!(result.is_ok());assert_eq!(attempts,2);
    }

    #[test]
    fn compatible_rendering_prefers_direct3d_12_then_vulkan(){
        let mut backends=Vec::new();
        let result=launch_compatible(eframe::NativeOptions::default(),&mut |options|{
            let eframe::egui_wgpu::WgpuSetup::CreateNew(setup)=options.wgpu_options.wgpu_setup else{panic!("应创建兼容图形设备")};
            backends.push(setup.instance_descriptor.backends);
            if backends.len()==1 {Err(eframe::Error::Wgpu(eframe::egui_wgpu::WgpuError::NoSuitableAdapterFound("测试".into())))} else {Ok(())}
        });
        assert!(result.is_ok());
        assert_eq!(backends,[eframe::wgpu::Backends::DX12,eframe::wgpu::Backends::VULKAN],"先 Direct3D 12，失败再 Vulkan");
    }

    #[test]
    fn compatible_rendering_uses_small_gpu_memory_blocks(){
        let base=eframe::wgpu::DeviceDescriptor{label:Some("egui wgpu device"),required_limits:eframe::wgpu::Limits{max_texture_dimension_2d:8192,..Default::default()},..Default::default()};
        let descriptor=small_memory_blocks(base);
        assert!(matches!(descriptor.memory_hints,eframe::wgpu::MemoryHints::MemoryUsage),"核显上默认分块会让进程多占约 300MB");
        assert_eq!(descriptor.required_limits.max_texture_dimension_2d,8192,"保留 egui 的其他设备要求");
        assert_eq!(descriptor.required_limits.max_non_sampler_bindings,MAX_BINDINGS,"默认 100 万项的描述符表会多占约 61MB");
    }

    #[test]
    fn application_errors_do_not_restart_the_application(){
        let mut attempts=0;
        assert!(run_with(false,eframe::NativeOptions::default(),|_|{
            attempts+=1;Err(eframe::Error::AppCreation("应用初始化失败".into()))
        }).is_err());
        assert_eq!(attempts,1);
    }

    #[test]
    fn successful_opengl_run_is_not_restarted(){
        let mut attempts=0;
        run_with(false,eframe::NativeOptions::default(),|options|{
            attempts+=1;assert_eq!(options.renderer,eframe::Renderer::Glow);Ok(())
        }).unwrap();
        assert_eq!(attempts,1);
    }

    fn no_adapter()->eframe::Error { eframe::Error::Wgpu(eframe::egui_wgpu::WgpuError::NoSuitableAdapterFound("测试".into())) }

    #[test]
    fn requested_compat_without_any_adapter_falls_back_to_opengl(){
        let mut renderers=Vec::new();
        let result=run_with(true,eframe::NativeOptions::default(),|options|{
            renderers.push(options.renderer);
            if options.renderer==eframe::Renderer::Wgpu {Err(no_adapter())} else {Ok(())}
        });
        assert!(result.is_ok());
        assert_eq!(renderers,[eframe::Renderer::Wgpu,eframe::Renderer::Wgpu,eframe::Renderer::Glow],"DX12、Vulkan 都没有设备时退回 OpenGL");
    }

    #[test]
    fn requested_compat_that_works_does_not_try_opengl(){
        let mut renderers=Vec::new();
        run_with(true,eframe::NativeOptions::default(),|options|{renderers.push(options.renderer);Ok(())}).unwrap();
        assert_eq!(renderers,[eframe::Renderer::Wgpu]);
    }

    #[test]
    fn requested_compat_app_errors_do_not_try_opengl(){
        let mut attempts=0;
        assert!(run_with(true,eframe::NativeOptions::default(),|_|{attempts+=1;Err(eframe::Error::AppCreation("应用初始化失败".into()))}).is_err());
        assert_eq!(attempts,1);
    }

    #[test]
    fn requested_compat_and_opengl_failures_are_both_reported(){
        let error=run_with(true,eframe::NativeOptions::default(),|options|{
            if options.renderer==eframe::Renderer::Wgpu {Err(no_adapter())} else {Err(eframe::Error::OpenGL("OpenGL 2.0".to_owned().into()))}
        }).unwrap_err().to_string();
        assert!(error.contains("测试") && error.contains("OpenGL 2.0"),"{error}");
    }

    #[test]
    fn both_failures_are_preserved_in_the_report(){
        let mut attempts=0;
        let error=run_with(false,eframe::NativeOptions::default(),|_|{
            attempts+=1;
            if attempts==1{Err(eframe::Error::OpenGL("OpenGL 2.0".to_owned().into()))}
            else{Err(eframe::Error::AppCreation("没有可用的图形设备".into()))}
        }).unwrap_err().to_string();
        assert!(error.contains("OpenGL 2.0") && error.contains("没有可用的图形设备"));
        assert_eq!(attempts,2);
    }
}
