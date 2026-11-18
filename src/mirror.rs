//! 多屏镜像：把岛仔当前画面同步到其余显示器，镜像窗口只显示、鼠标穿透。
//!
//! 交互仍在主窗口（设置里选中的屏幕）上；镜像用独立的分层窗口，
//! 位置算法与 `native::Window::place_on_display` 保持一致，缩放、偏移、停靠和 OLED 位移都同步。

#[cfg(windows)]
use windows::Win32::Foundation::POINT;

#[cfg(windows)]
pub struct Mirrors {
    layers: Vec<crate::layered::Layered>,
    key: String,
}

#[cfg(windows)]
impl Default for Mirrors {
    fn default() -> Self { Self::new() }
}

#[cfg(windows)]
impl Mirrors {
    pub fn new() -> Self { Self { layers: Vec::new(), key: String::new() } }

    /// 把一帧同步到所有非主屏；返回本帧是否有窗口内容变化。
    #[allow(clippy::too_many_arguments)]
    pub fn sync(
        &mut self,
        displays: &[crate::display_layout::Display],
        selected_id: &str,
        settings: &crate::model::Settings,
        scale: f32,
        offset: f32,
        shift: (i32, i32),
        fullscreen: Option<crate::display_layout::Bounds>,
        // 已经裁好的岛体画面尺寸（像素）。
        size: [usize; 2],
        frame: &[u8],
    ) -> bool {
        use crate::display_layout::{hide_for_fullscreen, placement_bounds, selected, side_origin, island_origin_offset, shift_slack};

        let (cw, ch) = (size[0], size[1]);
        if cw == 0 || ch == 0 || frame.len() != cw * ch * 4 { return false; }

        let selected_id = selected(displays, selected_id).map(|d| d.id.clone()).unwrap_or_default();
        let targets = crate::display_layout::mirror_targets(displays, &selected_id);
        if targets.is_empty() {
            for layer in &mut self.layers { layer.hide(); }
            self.layers.clear();
            self.key.clear();
            return false;
        }

        let mut key = String::new();
        let mut origins = Vec::with_capacity(targets.len());
        for display in &targets {
            let bounds = placement_bounds(display, settings.avoids_top_bar());
            let half = if settings.dock.is_side() { crate::side_dock::SIDE_HALF_EXTENT } else { crate::display_layout::ISLAND_HALF_EXTENT };
            let (back, forth) = shift_slack(bounds, scale, half, offset, settings.dock);
            let (dx, dy) = (shift.0.clamp(-back, forth), shift.1.clamp(-back, forth));
            let island=(cw as f32/scale,ch as f32/scale);
            let origin=if settings.dock.is_side() {
                side_origin(bounds,settings.dock,island,half,scale,offset)
            } else {
                island_origin_offset(bounds,island.0,scale,offset)
            };
            let Some((x,y))=origin else { origins.push(None); continue; };
            let (x,y)=if settings.dock.is_side() {(x,y+dy)}else{(x+dx,y)};
            origins.push(Some((x,y)));
            key.push_str(&format!("{}:{x},{y};", display.id));
        }
        key.push_str(&format!("s={size:?};i={selected_id}"));
        if key != self.key {
            self.key = key;
            for layer in &mut self.layers { layer.invalidate(); }
        }

        while self.layers.len() < targets.len() {
            match crate::layered::Layered::new() {
                Ok(layer) => self.layers.push(layer),
                Err(_) => break,
            }
        }
        if self.layers.len() > targets.len() {
            for layer in self.layers.iter_mut().skip(targets.len()) { layer.hide(); }
            self.layers.truncate(targets.len());
        }

        let mut changed = false;
        for (index, (layer, display)) in self.layers.iter_mut().zip(targets).enumerate() {
            let hidden = hide_for_fullscreen(settings.hide_fullscreen, fullscreen, Some(display.bounds));
            if hidden || origins[index].is_none() {
                layer.hide();
                continue;
            }
            let (x, y) = origins[index].unwrap();
            match layer.present_at(POINT { x, y }, size, frame) {
                Ok(value) => changed |= value,
                Err(_) => layer.hide(),
            }
        }
        changed
    }
}

#[cfg(not(windows))]
pub struct Mirrors;

#[cfg(not(windows))]
impl Mirrors {
    pub fn new() -> Self { Self }
    pub fn sync(
        &mut self,
        _displays: &[crate::display_layout::Display],
        _selected_id: &str,
        _settings: &crate::model::Settings,
        _scale: f32,
        _offset: f32,
        _shift: (i32, i32),
        _fullscreen: Option<crate::display_layout::Bounds>,
        _size: [usize; 2],
        _frame: &[u8],
    ) -> bool { false }
}

#[cfg(not(windows))]
impl Default for Mirrors {
    fn default() -> Self { Self::new() }
}
