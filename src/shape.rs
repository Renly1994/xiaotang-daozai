//! 刘海面板轮廓：顶部向外翻的「肩」+ 竖直侧边 + 底部大圆角。
//!
//! 轮廓左右对称、自上而下单调，所以按行切成梯形条填充（可处理凹肩），
//! 再沿外轮廓补一圈 1 物理像素的羽化带做抗锯齿——边缘不再有锯齿台阶。

use eframe::egui::{self, Color32, Pos2, Rect, Vec2};
use crate::model::Dock;

/// 左右停靠时，轮廓先在「贴顶」坐标系里生成：x 沿屏幕边缘（= 屏幕 y），
/// y 是离开边缘的距离（0 = 贴边）；再映射回屏幕。顶部停靠为恒等映射。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EdgeMap { dock: Dock, edge: f32 }

impl EdgeMap {
    pub const TOP: EdgeMap = EdgeMap { dock: Dock::Top, edge: 0.0 };
    /// `rect` 为屏幕坐标里的岛体外接矩形，返回映射与对应的贴顶矩形。
    pub fn of(rect: Rect, dock: Dock) -> (EdgeMap, Rect) {
        match dock {
            Dock::Top => (Self::TOP, rect),
            Dock::Left => (EdgeMap { dock, edge: rect.left() }, Rect::from_min_max(Pos2::new(rect.top(), 0.0), Pos2::new(rect.bottom(), rect.width()))),
            Dock::Right => (EdgeMap { dock, edge: rect.right() }, Rect::from_min_max(Pos2::new(rect.top(), 0.0), Pos2::new(rect.bottom(), rect.width()))),
        }
    }
    pub fn apply(self, p: Pos2) -> Pos2 {
        match self.dock {
            Dock::Top => p,
            Dock::Left => Pos2::new(self.edge + p.y, p.x),
            Dock::Right => Pos2::new(self.edge - p.y, p.x),
        }
    }
    fn code(self) -> f32 { match self.dock { Dock::Top => 0.0, Dock::Left => 1.0, Dock::Right => 2.0 } }
}

/// 阴影层相对主体的 (等距外扩, 向下偏移)。
///
/// 圆角增量必须**等于**横向扩展量 `d`，否则每层阴影的圆弧与主体圆弧不同心、会在角部
/// 互相交叉，看起来像「直线转半圆的地方多了一圈突起」。等距外扩后再整体下移 `dy`
/// 模拟顶部光源，四角圆心只随 dy 平移，不会再横向漂移。
///
/// `dy` 必须保持 0：顶部净外扩是 `d - dy`、底部是 `d + dy`。早期用 `0.35g` 时两者
/// 是 0.25g : 0.95g，顶部只剩一条硬边、底部有四五层渐淡的环，于是「上面两个角
/// 不如下面两个角平滑」。岛本来就贴着屏幕顶边，向下的落影方向感收益极小，
/// 完全对称的等距外扩才能保证四角过渡一模一样。
pub(crate) fn shadow_expansion(grow: f32) -> (f32, f32) {
    (grow * 0.6, 0.0)
}

/// 贴着 `dock` 边缘的轮廓（屏幕坐标）。
pub fn outline_docked(rect: Rect, dock: Dock, flare: f32, flare_h: f32, radius: f32) -> Vec<Pos2> {
    let (map, frame) = EdgeMap::of(rect, dock);
    outline(frame, flare, flare_h, radius).into_iter().map(|p| map.apply(p)).collect()
}

/// 贴着 `dock` 边缘的抗锯齿填充。
#[allow(clippy::too_many_arguments)]
pub fn fill_docked(painter: &egui::Painter, rect: Rect, dock: Dock, flare: f32, flare_h: f32, radius: f32, color: Color32, feather: f32) {
    let (map, frame) = EdgeMap::of(rect, dock);
    fill_mapped(painter, frame, map, flare, flare_h, radius, color, color, frame.height(), feather);
}

/// 从贴边处向外的渐变（毛玻璃柔光）。
#[allow(clippy::too_many_arguments)]
pub fn fill_gradient_docked(painter: &egui::Painter, rect: Rect, dock: Dock, flare: f32, flare_h: f32, radius: f32, top: Color32, bottom: Color32, span: f32, feather: f32) {
    let (map, frame) = EdgeMap::of(rect, dock);
    fill_mapped(painter, frame, map, flare, flare_h, radius, top, bottom, span, feather);
}

/// 贴着 `dock` 边缘的柔和落影。
pub fn shadow_docked(painter: &egui::Painter, rect: Rect, dock: Dock, flare: f32, flare_h: f32, radius: f32, strength: f32) {
    let (map, frame) = EdgeMap::of(rect, dock);
    shadow_mapped(painter, frame, map, flare, flare_h, radius, strength);
}

/// (y, 左 x, 右 x) 轮廓采样行。
pub fn sections(rect: Rect, flare: f32, flare_h: f32, radius: f32) -> Vec<(f32, f32, f32)> {
    let (x0, y0, x1, y1) = (rect.left(), rect.top(), rect.right(), rect.bottom());
    let r = radius.min(rect.height() * 0.5).min(rect.width() * 0.5).max(0.0);
    let fh = flare_h.min(rect.height() * 0.5).max(0.5);
    let mut rows = Vec::with_capacity(48);
    const N_FLARE: usize = 14;
    for i in 0..=N_FLARE {
        let t = i as f32 / N_FLARE as f32;
        // 肩部曲线：(1-t)^2.4 比三次方更饱满，过渡更柔。
        let wing = flare * (1.0 - t).powf(2.4);
        rows.push((y0 + fh * t, x0 - wing, x1 + wing));
    }
    if y1 - r > y0 + fh {
        rows.push((y1 - r, x0, x1));
    }
    // 32 段：直线转圆弧的过渡更细腻，端点切线几乎与直边重合。
    const N_CORNER: usize = 32;
    for i in 1..=N_CORNER {
        let a = i as f32 / N_CORNER as f32 * std::f32::consts::FRAC_PI_2;
        rows.push((y1 - r + r * a.sin(), x0 + r - r * a.cos(), x1 - r + r * a.cos()));
    }
    rows
}

/// 闭合外轮廓（顺时针：右侧自上而下，再左侧自下而上）。
pub fn outline(rect: Rect, flare: f32, flare_h: f32, radius: f32) -> Vec<Pos2> {
    let rows = sections(rect, flare, flare_h, radius);
    rows.iter()
        .map(|&(y, _, r)| Pos2::new(r, y))
        .chain(rows.iter().rev().map(|&(y, l, _)| Pos2::new(l, y)))
        .collect()
}

// 顶边不参与羽化；端点只取肩部切线，避免整条顶边扭曲法线。
fn edge_normal(pts: &[Pos2], i: usize) -> Vec2 {
    let normal = |a: Pos2, b: Pos2| {
        let d = (b - a).normalized();
        Vec2::new(d.y, -d.x)
    };
    if i == 0 {
        return normal(pts[0], pts[1]);
    }
    if i == pts.len() - 1 {
        return normal(pts[i - 1], pts[i]);
    }
    let prev = normal(pts[i - 1], pts[i]);
    let next = normal(pts[i], pts[i + 1]);
    // 相邻单位法线的交点补偿，保持两段的垂直羽化宽度一致。
    (prev + next) / (1.0 + prev.dot(next))
}

/// 抗锯齿填充。`feather` 是羽化宽度（逻辑点，通常 = 1 / ppp）。
pub fn fill(painter: &egui::Painter, rect: Rect, flare: f32, flare_h: f32, radius: f32, color: Color32, feather: f32) {
    fill_gradient(painter, rect, flare, flare_h, radius, color, color, rect.height(), feather);
}

/// 自上而下的渐变填充：顶边为 `top`，向下 `span` 逻辑点处过渡到 `bottom`，此后保持 `bottom`。
/// 毛玻璃皮肤用它画顶部柔光。
#[allow(clippy::too_many_arguments)]
pub fn fill_gradient(painter: &egui::Painter, rect: Rect, flare: f32, flare_h: f32, radius: f32, top: Color32, bottom: Color32, span: f32, feather: f32) {
    fill_mapped(painter, rect, EdgeMap::TOP, flare, flare_h, radius, top, bottom, span, feather);
}

#[allow(clippy::too_many_arguments)]
fn fill_mapped(painter: &egui::Painter, rect: Rect, map: EdgeMap, flare: f32, flare_h: f32, radius: f32, top: Color32, bottom: Color32, span: f32, feather: f32) {
    let span=if top==bottom {0.0} else {span};
    let key=([rect.min.x,rect.min.y,rect.max.x,rect.max.y,flare,flare_h,radius,feather,span,map.code(),map.edge].map(f32::to_bits),top,bottom);
    let mesh=painter.ctx().memory_mut(|memory|memory.caches.cache::<FillCache>().get(key));
    painter.add(egui::Shape::Mesh(mesh));
}

type FillKey=([u32;11],Color32,Color32);
type FillCache=egui::cache::FrameCache<std::sync::Arc<egui::Mesh>,FillComputer>;
#[derive(Default)]
struct FillComputer;
impl egui::cache::ComputerMut<FillKey,std::sync::Arc<egui::Mesh>> for FillComputer {
    fn compute(&mut self,(bits,top,bottom):FillKey)->std::sync::Arc<egui::Mesh>{
        let [x0,y0,x1,y1,flare,flare_h,radius,feather,span,dock,edge]=bits.map(f32::from_bits);
        let mut mesh=gradient_mesh(Rect::from_min_max(Pos2::new(x0,y0),Pos2::new(x1,y1)),flare,flare_h,radius,top,bottom,span,feather);
        if dock!=0.0 {
            let map=EdgeMap{dock:if dock==1.0{Dock::Left}else{Dock::Right},edge};
            for v in &mut mesh.vertices {v.pos=map.apply(v.pos);}
        }
        std::sync::Arc::new(mesh)
    }
}

/// 预乘颜色的线性插值。
fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let ch = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgba_premultiplied(ch(a.r(), b.r()), ch(a.g(), b.g()), ch(a.b(), b.b()), ch(a.a(), b.a()))
}

// 仅保留当前帧仍使用的轮廓；尺寸、透明度或 DPI 改变时重新生成，动画不复用旧形状。
#[cfg(test)]
fn fill_mesh(rect:Rect,flare:f32,flare_h:f32,radius:f32,color:Color32,feather:f32)->egui::Mesh {
    gradient_mesh(rect,flare,flare_h,radius,color,color,0.0,feather)
}

#[allow(clippy::too_many_arguments)]
fn gradient_mesh(rect:Rect,flare:f32,flare_h:f32,radius:f32,top:Color32,bottom:Color32,span:f32,feather:f32)->egui::Mesh {
    let at=|y:f32| if span<=0.0 {top} else {lerp_color(top,bottom,(y-rect.top())/span)};
    let rows = sections(rect, flare, flare_h, radius);
    let mut mesh = egui::Mesh::default();
    mesh.vertices.reserve((rows.len()*3-2)*4);
    mesh.indices.reserve((rows.len()*3-2)*6);
    // 1) 实心梯形条
    for w in rows.windows(2) {
        let (ya, la, ra) = w[0];
        let (yb, lb, rb) = w[1];
        let i = mesh.vertices.len() as u32;
        mesh.colored_vertex(Pos2::new(la, ya), at(ya));
        mesh.colored_vertex(Pos2::new(ra, ya), at(ya));
        mesh.colored_vertex(Pos2::new(rb, yb), at(yb));
        mesh.colored_vertex(Pos2::new(lb, yb), at(yb));
        mesh.add_triangle(i, i + 1, i + 2);
        mesh.add_triangle(i, i + 2, i + 3);
    }
    // 2) 外轮廓羽化带（不含顶边：贴着屏幕顶）
    let pts: Vec<Pos2> = rows
        .iter()
        .map(|&(y, _, r)| Pos2::new(r, y))
        .chain(rows.iter().rev().map(|&(y, l, _)| Pos2::new(l, y)))
        .collect();
    let n = pts.len();
    let clear = Color32::TRANSPARENT;
    for i in 0..n - 1 {
        let (a, b) = (pts[i], pts[i + 1]);
        // 顶边两端（右上角 → 左上角那条闭合边）不羽化
        if a.y <= rect.top() + 0.01 && b.y <= rect.top() + 0.01 {
            continue;
        }
        let (na, nb) = (edge_normal(&pts, i), edge_normal(&pts, i + 1));
        let base = mesh.vertices.len() as u32;
        mesh.colored_vertex(a, at(a.y));
        mesh.colored_vertex(b, at(b.y));
        mesh.colored_vertex(b + nb * feather, clear);
        mesh.colored_vertex(a + na * feather, clear);
        mesh.add_triangle(base, base + 1, base + 2);
        mesh.add_triangle(base, base + 2, base + 3);
    }
    mesh
}

/// 柔和的落影：多层半透明轮廓叠加，近似高斯模糊（不依赖 GPU 模糊）。
pub fn shadow(painter: &egui::Painter, rect: Rect, flare: f32, flare_h: f32, radius: f32, strength: f32) {
    shadow_mapped(painter, rect, EdgeMap::TOP, flare, flare_h, radius, strength);
}

fn shadow_mapped(painter: &egui::Painter, rect: Rect, map: EdgeMap, flare: f32, flare_h: f32, radius: f32, strength: f32) {
    if strength <= 0.01 {
        return;
    }
    for (grow, a) in [(10.0f32, 0.035f32), (6.5, 0.05), (3.5, 0.07), (1.5, 0.09)] {
        let (d, dy) = shadow_expansion(grow);
        let r = Rect::from_min_max(
            Pos2::new(rect.left() - d, rect.top() - (d - dy)),
            Pos2::new(rect.right() + d, rect.bottom() + d + dy),
        );
        let c = Color32::from_black_alpha((255.0 * a * strength) as u8);
        fill_mapped(painter, r, map, flare, flare_h, radius + d, c, c, r.height(), grow * 0.9);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inside(points: &[Pos2], p: Pos2) -> bool {
        let mut inside = false;
        let mut prev = *points.last().unwrap();
        for &next in points {
            if (next.y > p.y) != (prev.y > p.y)
                && p.x < (prev.x - next.x) * (p.y - next.y) / (prev.y - next.y) + next.x
            {
                inside = !inside;
            }
            prev = next;
        }
        inside
    }

    #[test]
    fn shoulder_feather_keeps_one_pixel_perpendicular_to_edge() {
        for (width, height, flare, flare_h, radius) in [
            (143.0, 25.0, 5.0, 8.0, 12.5),
            (419.0, 140.0, 17.6, 16.0, 33.0),
        ] {
            let pts = outline(Rect::from_min_size(Pos2::ZERO, Vec2::new(width, height)), flare, flare_h, radius);
            for ppp in [1.0, 1.25, 1.5, 2.0] {
                for i in 0..pts.len() - 1 {
                    let d = (pts[i + 1] - pts[i]).normalized();
                    let normal = Vec2::new(d.y, -d.x);
                    for j in [i, i + 1] {
                        let offset = edge_normal(&pts, j) / ppp;
                        assert!((offset.dot(normal) * ppp - 1.0).abs() < 0.001,
                            "羽化必须沿每段轮廓保持一物理像素：边 {i}，顶点 {j}，缩放 {ppp}");
                    }
                }
            }
        }
    }

    #[test]
    fn wings_extend_outside_body_only_at_top() {
        let rect = Rect::from_min_size(Pos2::new(110.0, 0.0), Vec2::new(419.0, 140.0));
        let pts = outline(rect, 17.6, 16.0, 33.0);
        assert!(pts.iter().any(|p| p.x < rect.left() - 17.0));
        assert!(inside(&pts, Pos2::new(rect.left() + 0.2, 70.0)));
        assert!(!inside(&pts, Pos2::new(rect.left() - 0.5, 70.0)));
        // 底部圆角外侧是空的
        assert!(!inside(&pts, Pos2::new(rect.left() + 2.0, rect.bottom() - 2.0)));
    }
}

#[cfg(test)]
mod dock_tests {
    use super::*;
    #[test]
    fn side_outlines_hug_the_screen_edge_with_the_wings_along_it() {
        let rect = Rect::from_min_size(Pos2::new(0.0, 100.0), Vec2::new(419.0, 140.0));
        let left = outline_docked(rect, Dock::Left, 17.6, 16.0, 33.0);
        let b = Rect::from_points(&left);
        assert!((b.left() - 0.0).abs() < 0.01 && (b.right() - 419.0).abs() < 0.01, "{b:?}");
        assert!(b.top() < 100.0 - 17.0 && b.bottom() > 240.0 + 17.0, "肩部沿屏幕边缘上下展开：{b:?}");
        // 肩部只在贴边处
        assert!(left.iter().filter(|p| p.y < 99.0).all(|p| p.x < 16.5));
        let right_rect = Rect::from_min_size(Pos2::new(81.0, 100.0), Vec2::new(419.0, 140.0));
        let right = outline_docked(right_rect, Dock::Right, 17.6, 16.0, 33.0);
        let b = Rect::from_points(&right);
        assert!((b.right() - 500.0).abs() < 0.01 && (b.left() - 81.0).abs() < 0.01, "{b:?}");
        assert!(right.iter().filter(|p| p.y < 99.0).all(|p| p.x > 500.0 - 16.5));
        assert_eq!(outline_docked(rect, Dock::Top, 5.0, 8.0, 12.0), outline(rect, 5.0, 8.0, 12.0));
    }
}


#[cfg(test)]
mod cache_tests {
    use super::*;
    fn frame(ctx:&egui::Context,width:f32,feather:f32)->std::sync::Arc<egui::Mesh>{
        let out=ctx.run(egui::RawInput::default(),|ctx|{
            fill(&ctx.layer_painter(egui::LayerId::background()),Rect::from_min_size(Pos2::ZERO,Vec2::new(width,30.0)),5.0,8.0,12.5,Color32::BLACK,feather);
        });
        out.shapes.into_iter().find_map(|s|if let egui::Shape::Mesh(m)=s.shape{Some(m)}else{None}).unwrap()
    }
    #[test]
    fn unchanged_outline_reuses_geometry_across_frames(){
        let ctx=egui::Context::default();let first=frame(&ctx,300.0,1.0);let second=frame(&ctx,300.0,1.0);
        assert!(std::sync::Arc::ptr_eq(&first,&second),"稳定轮廓不能每帧重新分配网格");
        let changed=frame(&ctx,320.0,1.0);assert_ne!(first.vertices,changed.vertices);
        let scaled=frame(&ctx,320.0,0.5);assert_ne!(changed.vertices,scaled.vertices);
    }

    #[test]
    fn animated_outlines_do_not_accumulate_old_meshes(){
        let ctx=egui::Context::default();
        for i in 0..300 {
            let mesh=frame(&ctx,300.0+i as f32/10.0,1.0);
            let expected=fill_mesh(Rect::from_min_size(Pos2::ZERO,Vec2::new(300.0+i as f32/10.0,30.0)),5.0,8.0,12.5,Color32::BLACK,1.0);
            assert_eq!(mesh.vertices,expected.vertices);assert_eq!(mesh.indices,expected.indices);
            let count=ctx.memory_mut(|m|egui::cache::CacheTrait::len(m.caches.cache::<FillCache>()));
            assert!(count<=2,"动画不能累积历史轮廓：{count}");
        }
    }
}

const ROW_STEP: f32 = 6.0;
const COL_STEP: f32 = 9.0;
const COLS_MIN: usize = 10;
const COLS_MAX: usize = 64;

/// 把行表补密：相邻两行之间按需插入等距行，左右边界线性插值。
fn densify(rows: Vec<(f32, f32, f32)>) -> Vec<(f32, f32, f32)> {
    let mut out = Vec::with_capacity(rows.len() * 3);
    for w in rows.windows(2) {
        let (ya, la, ra) = w[0];
        let (yb, lb, rb) = w[1];
        out.push(w[0]);
        let n = ((yb - ya) / ROW_STEP).ceil() as usize;
        for i in 1..n {
            let t = i as f32 / n as f32;
            out.push((ya + (yb - ya) * t, la + (lb - la) * t, ra + (rb - ra) * t));
        }
    }
    if let Some(last) = rows.last() {
        out.push(*last);
    }
    out
}

pub(crate) fn shade_mesh(
    rect: Rect,
    rows: Vec<(f32,f32,f32)>,
    feather: f32,
    shade: &dyn Fn(f32, f32) -> Color32,
) -> egui::Mesh {
    let rows = densify(rows);
    let cols = ((rect.width() / COL_STEP).ceil() as usize).clamp(COLS_MIN, COLS_MAX);
    let mut mesh = egui::Mesh::default();
    mesh.vertices.reserve(rows.len() * cols * 4);
    mesh.indices.reserve(rows.len() * cols * 6);
    // 1) 二维网格：每行横向切 cols 段，四个角各自取色，光斑才不会跨行被抹平
    for w in rows.windows(2) {
        let (ya, la, ra) = w[0];
        let (yb, lb, rb) = w[1];
        for i in 0..cols {
            let t0 = i as f32 / cols as f32;
            let t1 = (i + 1) as f32 / cols as f32;
            let ax0 = la + (ra - la) * t0;
            let ax1 = la + (ra - la) * t1;
            let bx0 = lb + (rb - lb) * t0;
            let bx1 = lb + (rb - lb) * t1;
            let id = mesh.vertices.len() as u32;
            mesh.colored_vertex(Pos2::new(ax0, ya), shade(ax0, ya));
            mesh.colored_vertex(Pos2::new(ax1, ya), shade(ax1, ya));
            mesh.colored_vertex(Pos2::new(bx1, yb), shade(bx1, yb));
            mesh.colored_vertex(Pos2::new(bx0, yb), shade(bx0, yb));
            mesh.add_triangle(id, id + 1, id + 2);
            mesh.add_triangle(id, id + 2, id + 3);
        }
    }
    // 2) 外轮廓羽化带（不含顶边）：外沿保持同色但完全透明
    let pts: Vec<Pos2> = rows
        .iter()
        .map(|&(y, _, r)| Pos2::new(r, y))
        .chain(rows.iter().rev().map(|&(y, l, _)| Pos2::new(l, y)))
        .collect();
    let n = pts.len();
    for i in 0..n - 1 {
        let (a, b) = (pts[i], pts[i + 1]);
        if a.y <= rect.top() + 0.01 && b.y <= rect.top() + 0.01 {
            continue;
        }
        let (na, nb) = (edge_normal(&pts, i), edge_normal(&pts, i + 1));
        let clear = |p: Pos2| {
            let c = shade(p.x, p.y);
            Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), 0)
        };
        let base = mesh.vertices.len() as u32;
        mesh.colored_vertex(a, shade(a.x, a.y));
        mesh.colored_vertex(b, shade(b.x, b.y));
        mesh.colored_vertex(b + nb * feather, clear(b + nb * feather));
        mesh.colored_vertex(a + na * feather, clear(a + na * feather));
        mesh.add_triangle(base, base + 1, base + 2);
        mesh.add_triangle(base, base + 2, base + 3);
    }
    mesh
}

#[cfg(test)]
mod corner_tests {
    use super::*;

    #[test]
    fn 阴影层四角圆弧与主体同心等距() {
        let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(320.0, 46.0));
        let radius = 16.0;
        let body_rt = Pos2::new(rect.right() - radius, rect.top() + radius);
        let body_lb = Pos2::new(rect.left() + radius, rect.bottom() - radius);
        for grow in [10.0f32, 6.5, 3.5, 1.5] {
            let (d, dy) = shadow_expansion(grow);
            let r = Rect::from_min_max(rect.min - Vec2::new(d, d - dy), rect.max + Vec2::new(d, d + dy));
            let rr = radius + d;
            let shadow_rt = Pos2::new(r.right() - rr, r.top() + rr);
            let shadow_lb = Pos2::new(r.left() + rr, r.bottom() - rr);
            // 等距外扩不改圆心，整体下移只让圆心跟着下移 dy；圆弧不横向漂移就不会与主体交叉。
            assert!((rr - radius - d).abs() < 1e-4, "圆角增量必须等于横向扩展量");
            // 顶部净外扩 / 底部净外扩：太不对称就会「上两角硬、下两角柔」。
            let top = d - dy;
            let bottom = d + dy;
            assert!(top > 0.0, "顶部阴影不能缩进主体");
            assert!((top - bottom).abs() < 1e-6, "上下阴影必须严格对称，否则上两角比下两角生硬：{top}/{bottom}");
            assert!((shadow_rt.x - body_rt.x).abs() < 1e-4, "右上角圆弧不能横向漂移");
            assert!((shadow_rt.y - body_rt.y - dy).abs() < 1e-4);
            assert!((shadow_lb.x - body_lb.x).abs() < 1e-4, "左下角圆弧不能横向漂移");
            assert!((shadow_lb.y - body_lb.y - dy).abs() < 1e-4);

            // 旧参数（圆角 +0.8g、横向 +0.6g、上下不对称）确实让角部圆弧错位，
            // 这正是「直线转半圆处有突起」的来源。
            let old_r = Rect::from_min_max(
                rect.min - Vec2::new(grow * 0.6, grow * 0.3),
                rect.max + Vec2::new(grow * 0.6, grow),
            );
            let old_c = Pos2::new(old_r.right() - (radius + grow * 0.8), old_r.top() + radius + grow * 0.8);
            assert!((old_c.x - body_rt.x).abs() > 0.1 * grow, "旧实现圆心漂移量应当可测");
        }
    }

    #[test]
    fn 圆弧两端与直边相切() {
        let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(320.0, 140.0));
        let r = 33.0;
        let rows = sections(rect, 0.0, 16.0, r);
        let corner: Vec<_> = rows.iter().copied().filter(|t| t.0 >= rect.bottom() - r - 1e-4).collect();
        assert!(corner.len() > 20, "1/4 圆弧要有足够采样");

        // 竖直侧边 -> 圆弧起点：切线必须接近竖直，否则直线接半圆时会有折角。
        let (ya, la, _) = corner[0];
        let (yb, lb, _) = corner[1];
        assert!((lb - la).abs() / (yb - ya) < 0.03, "圆弧起点切线不够竖直");

        // 圆弧终点 -> 水平底边：切线必须接近水平。
        let (y1, l1, _) = corner[corner.len() - 2];
        let (y2, l2, _) = corner[corner.len() - 1];
        assert!((y2 - y1) / (l2 - l1) < 0.03, "圆弧终点切线不够水平");

        // 羽化法线不能在轮廓拐点炸开（否则角部会拉出尖刺）。
        let pts = outline(rect, 0.0, 16.0, r);
        for i in 0..pts.len() {
            assert!(edge_normal(&pts, i).length() < 2.0, "角部羽化法线异常：{i}");
        }
    }
}

#[cfg(test)]
mod fluid_path_bench {
    use super::*;
    use crate::fluid::Fluid;
    use std::time::Instant;

    /// 复刻 `paint_body` 里悬浮云胶囊的行表镜像。
    fn pill_rows(rect: Rect, radius: f32) -> Vec<(f32, f32, f32)> {
        let mut rows = sections(rect, 0.0, 16.0, radius);
        let mid = rect.center().y;
        rows.retain(|r| r.0 > mid && r.0 >= rect.bottom() - radius);
        let top = rows.iter().rev().map(|&(y, l, r)| (2.0 * mid - y, l, r)).collect::<Vec<_>>();
        rows.splice(0..0, top);
        rows
    }

    #[test]
    fn 纹理路径比梯形网格省得多() {
        let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(419.0, 215.0));
        let rows = pill_rows(rect, 26.0);
        let fluid = Fluid::default();
        let (w, h) = (rect.width(), rect.height());
        const N: usize = 40;

        let t = Instant::now();
        let mut vertices = 0usize;
        let mut tris = 0usize;
        for _ in 0..N {
            let mesh = shade_mesh(rect, rows.clone(), 1.0, &|x, y| fluid.compose(x, y, w, h, 1.0));
            vertices = mesh.vertices.len();
            tris = mesh.indices.len() / 3;
            std::hint::black_box(mesh);
        }
        let old_ms = t.elapsed().as_secs_f64() * 1000.0 / N as f64;

        let t = Instant::now();
        for _ in 0..N {
            let mut image = egui::ColorImage::new([160, 80], Color32::TRANSPARENT);
            for row in 0..80usize {
                let py = h * (row as f32 + 0.5) / 80.0;
                for col in 0..160usize {
                    image.pixels[row * 160 + col] =
                        fluid.compose(w * (col as f32 + 0.5) / 160.0, py, w, h, 1.0);
                }
            }
            std::hint::black_box(image);
        }
        let new_ms = t.elapsed().as_secs_f64() * 1000.0 / N as f64;

        println!("旧 shade_mesh: {vertices} 顶点 / {tris} 三角形 / {old_ms:.3} ms 每帧");
        println!("新颜色纹理: {} 像素 / {new_ms:.3} ms 每帧", 160 * 80);
        println!("顶点数据量: 旧 {:.0} KB -> 新 1 个 RectShape", vertices as f64 * 24.0 / 1024.0);
        assert!(vertices > 1000, "旧路径确实每帧生成大量顶点");
    }
}
