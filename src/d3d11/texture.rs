// This file contains implementations inspired by or derived from the following
// sources:
// - https://github.com/ohchase/egui-directx/blob/master/egui-directx11/src/texture.rs
//
// Here I would express my gratitude for their contributions to the Rust
// community. Their work served as a valuable reference and inspiration for this
// project.
//
// Nekomaru, March 2024

use std::{collections::HashMap, mem, slice};

use egui::{Color32, ImageData, TextureFilter, TextureId, TextureOptions, TextureWrapMode, TexturesDelta};

use windows::{
    Win32::Graphics::{Direct3D11::*, Dxgi::Common::*},
    core::Result,
};

struct ManagedTexture {
    tex: ID3D11Texture2D,
    srv: ID3D11ShaderResourceView,
    pixels: Vec<Color32>,
    width: usize,
    options: TextureOptions,
}

enum Texture {
    /// A texture managed by egui (created from ImageData)
    Managed(ManagedTexture),
    /// A user-provided texture (registered from an existing shader resource view)
    User {
        srv: ID3D11ShaderResourceView,
        options: TextureOptions,
    },
}
impl Texture {
    pub fn is_managed(&self) -> bool {
        matches!(self, Texture::Managed(_))
    }

    pub fn is_user(&self) -> bool {
        matches!(self, Texture::User { .. })
    }

    fn options(&self) -> Option<TextureOptions> {
        match self {
            Texture::Managed(m) => Some(m.options),
            Texture::User { options, .. } => Some(*options),
        }
    }
}

pub struct TexturePool {
    device: ID3D11Device,
    pool: HashMap<TextureId, Texture>,
    next_user_texture_id: u64,
}

impl TexturePool {
    pub fn new(device: &ID3D11Device) -> Self {
        Self {
            device: device.clone(),
            pool: HashMap::new(),
            next_user_texture_id: 0,
        }
    }

    pub fn get_srv(&self, tid: TextureId) -> Option<ID3D11ShaderResourceView> {
        self.pool.get(&tid).map(|t| match t {
            Texture::Managed(managed) => managed.srv.clone(),
            Texture::User { srv, .. } => srv.clone(),
        })
    }

    /// 该纹理请求的采样选项；未注册时用 egui 的默认（线性 + ClampToEdge）。
    pub fn options(&self, tid: TextureId) -> TextureOptions {
        self.pool
            .get(&tid)
            .and_then(Texture::options)
            .unwrap_or(TextureOptions::LINEAR)
    }

    /// Register a user-provided shader resource view and get a TextureId for it.
    /// This TextureId can be used in egui to reference this texture.
    ///
    /// The returned TextureId will be unique and won't conflict with egui's managed textures.
    pub fn register_user_texture(
        &mut self,
        srv: ID3D11ShaderResourceView,
    ) -> TextureId {
        let id = TextureId::User(self.next_user_texture_id);
        self.next_user_texture_id += 1;
        self.pool.insert(
            id,
            Texture::User {
                srv,
                options: TextureOptions::LINEAR,
            },
        );
        id
    }

    /// Unregister a user texture by its TextureId.
    /// Returns true if the texture was found and removed, false otherwise.
    pub fn unregister_user_texture(&mut self, tid: TextureId) -> bool {
        if self.pool.get(&tid).is_some_and(|t| t.is_user()) {
            self.pool.remove(&tid);
            true
        } else {
            false
        }
    }

    pub fn update(
        &mut self,
        ctx: &ID3D11DeviceContext,
        delta: TexturesDelta,
    ) -> Result<()> {
        // egui 0.31 TexturesDelta 无 Drop，可直接消费字段。
        for (tid, delta) in delta.set {
                if delta.is_whole()
                    && delta.image.width() > 0
                    && delta.image.height() > 0
                {
                    self.pool.insert(
                        tid,
                        Self::create_managed_texture(
                            &self.device,
                            delta.image.clone(),
                            delta.options,
                        )?,
                    );
                    // the old texture is returned and dropped here, freeing
                    // all its gpu resource.
                } else if let Some(tex) =
                    self.pool.get_mut(&tid).filter(|t| t.is_managed())
                {
                    Self::update_partial(
                        ctx,
                        tex,
                        delta.image.clone(),
                        delta.pos.unwrap(),
                    )?;
                } else {
                    log::warn!(
                        "egui wants to update a non-existing texture {tid:?}. this request will be ignored."
                    );
                }
        }
        for tid in delta.free {
            if self.pool.get(&tid).is_some_and(|t| t.is_managed()) {
                self.pool.remove(&tid);
            }
        }
        Ok(())
    }

    fn update_partial(
        ctx: &ID3D11DeviceContext,
        old: &mut Texture,
        image: ImageData,
        [nx, ny]: [usize; 2],
    ) -> Result<()> {
        let Texture::Managed(old) = old else {
            log::warn!(
                "attempted to partially update a user texture, which is not supported"
            );
            return Ok(());
        };

        let subr = unsafe {
            let mut output = D3D11_MAPPED_SUBRESOURCE::default();
            ctx.Map(
                &old.tex,
                0,
                D3D11_MAP_WRITE_DISCARD,
                0,
                Some(&mut output),
            )?;
            output
        };
        let data = unsafe {
            let slice = slice::from_raw_parts_mut(
                subr.pData as *mut Color32,
                old.pixels.len(),
            );
            slice.as_mut_ptr().copy_from_nonoverlapping(
                old.pixels.as_ptr(),
                old.pixels.len(),
            );
            slice
        };
        match image {
            ImageData::Color(f) => {
                for y in 0..f.height() {
                    for x in 0..f.width() {
                        let whole = (ny + y) * old.width + nx + x;
                        let frac = y * f.width() + x;
                        old.pixels[whole] = f.pixels[frac];
                        data[whole] = f.pixels[frac];
                    }
                }
            },
            ImageData::Font(f) => {
                for y in 0..f.height() {
                    for x in 0..f.width() {
                        let whole = (ny + y) * old.width + nx + x;
                        let frac = y * f.width() + x;
                        let v = f.pixels[frac];
                        let c = Color32::from_white_alpha((v * 255.0).round() as u8);
                        old.pixels[whole] = c;
                        data[whole] = c;
                    }
                }
            },
        }
        unsafe { ctx.Unmap(&old.tex, 0) };
        Ok(())
    }

    fn create_managed_texture(
        device: &ID3D11Device,
        data: ImageData,
        options: TextureOptions,
    ) -> Result<Texture> {
        let width = data.width();

        let pixels = match &data {
            ImageData::Color(c) => c.pixels.clone(),
            ImageData::Font(f) => f.pixels.iter()
                .map(|&v| Color32::from_white_alpha((v * 255.0).round() as u8))
                .collect(),
        };

        let desc = D3D11_TEXTURE2D_DESC {
            Width: data.width() as _,
            Height: data.height() as _,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_R8G8B8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DYNAMIC,
            BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as _,
            CPUAccessFlags: D3D11_CPU_ACCESS_WRITE.0 as _,
            ..Default::default()
        };

        let subresource_data = D3D11_SUBRESOURCE_DATA {
            pSysMem: pixels.as_ptr() as _,
            SysMemPitch: (width * mem::size_of::<Color32>()) as u32,
            SysMemSlicePitch: 0,
        };

        let mut tex = None;
        unsafe {
            device.CreateTexture2D(
                &desc,
                Some(&subresource_data),
                Some(&mut tex),
            )
        }?;
        let tex = tex.unwrap();

        let mut srv = None;
        unsafe { device.CreateShaderResourceView(&tex, None, Some(&mut srv)) }?;
        let srv = srv.unwrap();

        Ok(Texture::Managed(ManagedTexture {
            tex,
            srv,
            width,
            pixels,
            options,
        }))
    }
}

/// 采样器缓存：egui 每个纹理会带 `TextureOptions`（过滤方式 + wrap 模式），
/// 必须按它选采样器，否则字体图集/图标会被全局采样器的边界行为污染。
#[derive(Default)]
pub struct SamplerCache {
    map: std::collections::HashMap<SamplerKey, ID3D11SamplerState>,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
struct SamplerKey {
    min_linear: bool,
    mag_linear: bool,
    wrap: i32,
}

impl SamplerKey {
    fn new(options: TextureOptions) -> Self {
        let wrap = match options.wrap_mode {
            TextureWrapMode::ClampToEdge => D3D11_TEXTURE_ADDRESS_CLAMP.0,
            TextureWrapMode::Repeat => D3D11_TEXTURE_ADDRESS_WRAP.0,
            TextureWrapMode::MirroredRepeat => D3D11_TEXTURE_ADDRESS_MIRROR.0,
        };
        Self {
            min_linear: matches!(options.minification, TextureFilter::Linear),
            mag_linear: matches!(options.magnification, TextureFilter::Linear),
            wrap,
        }
    }
}

impl SamplerCache {
    pub fn get(
        &mut self,
        device: &ID3D11Device,
        options: TextureOptions,
    ) -> Option<ID3D11SamplerState> {
        let key = SamplerKey::new(options);
        if let Some(s) = self.map.get(&key) {
            return Some(s.clone());
        }
        let filter = match (key.min_linear, key.mag_linear) {
            (true, true) => D3D11_FILTER_MIN_MAG_MIP_LINEAR,
            (false, false) => D3D11_FILTER_MIN_MAG_MIP_POINT,
            (true, false) => D3D11_FILTER_MIN_LINEAR_MAG_MIP_POINT,
            (false, true) => D3D11_FILTER_MIN_POINT_MAG_MIP_LINEAR,
        };
        let desc = D3D11_SAMPLER_DESC {
            Filter: filter,
            AddressU: D3D11_TEXTURE_ADDRESS_MODE(key.wrap),
            AddressV: D3D11_TEXTURE_ADDRESS_MODE(key.wrap),
            AddressW: D3D11_TEXTURE_ADDRESS_MODE(key.wrap),
            ComparisonFunc: D3D11_COMPARISON_ALWAYS,
            // ClampToEdge 语义下边界色不会被采到；保险起见给白色，和字体图集空白一致。
            BorderColor: [1., 1., 1., 1.],
            ..D3D11_SAMPLER_DESC::default()
        };
        let mut state = None;
        unsafe { device.CreateSamplerState(&desc, Some(&mut state)).ok()? };
        let state = state?;
        self.map.insert(key, state.clone());
        Some(state)
    }
}

#[cfg(test)]
mod sampler_tests {
    use super::{SamplerKey, TextureOptions};
    use egui::{TextureFilter, TextureWrapMode};
    use windows::Win32::Graphics::Direct3D11::*;

    /// egui 默认的 LINEAR 必须是 ClampToEdge + 双线性——这是修细线的关键。
    #[test]
    fn linear_options_map_to_clamp_and_bilinear() {
        let k = SamplerKey::new(TextureOptions::LINEAR);
        assert!(k.min_linear && k.mag_linear);
        assert_eq!(k.wrap, D3D11_TEXTURE_ADDRESS_CLAMP.0);
    }

    #[test]
    fn nearest_options_map_to_point_filter() {
        let k = SamplerKey::new(TextureOptions::NEAREST);
        assert!(!k.min_linear && !k.mag_linear);
    }

    #[test]
    fn wrap_modes_map_distinctly() {
        let clamp = SamplerKey::new(TextureOptions {
            wrap_mode: TextureWrapMode::ClampToEdge,
            ..TextureOptions::LINEAR
        });
        let repeat = SamplerKey::new(TextureOptions {
            wrap_mode: TextureWrapMode::Repeat,
            ..TextureOptions::LINEAR
        });
        let mirror = SamplerKey::new(TextureOptions {
            wrap_mode: TextureWrapMode::MirroredRepeat,
            ..TextureOptions::LINEAR
        });
        assert_ne!(clamp.wrap, repeat.wrap);
        assert_ne!(repeat.wrap, mirror.wrap);
        assert_eq!(repeat.wrap, D3D11_TEXTURE_ADDRESS_WRAP.0);
        assert_eq!(mirror.wrap, D3D11_TEXTURE_ADDRESS_MIRROR.0);
    }

    /// 只改一个维度也必须产生不同的 key，否则会错误复用采样器。
    #[test]
    fn mixed_filters_are_distinct_keys() {
        let mm = SamplerKey::new(TextureOptions {
            magnification: TextureFilter::Linear,
            minification: TextureFilter::Linear,
            wrap_mode: TextureWrapMode::ClampToEdge,
            mipmap_mode: None,
        });
        let pm = SamplerKey::new(TextureOptions {
            magnification: TextureFilter::Linear,
            minification: TextureFilter::Nearest,
            wrap_mode: TextureWrapMode::ClampToEdge,
            mipmap_mode: None,
        });
        assert_ne!(mm, pm);
    }
}
