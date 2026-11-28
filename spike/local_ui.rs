#[cfg(windows)]
use super::market_state::MarketStatus;
use super::*;
use crate::{
    extras::{
        page_visibility_action, settings_apply_style, settings_card, settings_control_row,
        settings_hint, settings_label, SETTINGS_SWITCH_W,
    },
    paint::{self, Card, H},
    settings_theme::color,
    theme::{self, W},
};
use eframe::egui::{self, Color32};
#[cfg(windows)]
use std::sync::Arc;

#[cfg(windows)]
#[path = "market_browse_ui.rs"]
mod market_browse_ui;
#[cfg(windows)]
#[path = "plugin_author_ui.rs"]
mod plugin_author_ui;
#[cfg(windows)]
#[path = "plugin_guide_ui.rs"]
mod plugin_guide_ui;
#[path = "plugin_feedback_ui.rs"]
mod plugin_feedback_ui;
use plugin_feedback_ui::{plugin_feedback, request_plugin_feedback};
#[path = "script_settings_ui.rs"]
mod script_settings_ui;

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum SettingsNavigation {
    Tab(usize),
    Plugin(String),
    OpenStatus(String),
}

const COVERS: &str = "settings-plugin-covers";
type CoverCache = std::collections::VecDeque<(egui::Id, Option<egui::TextureHandle>)>;

#[cfg(windows)]
#[derive(Clone)]
struct SourceDialog {
    plugin: String,
    url: String,
    secret: String,
    error: String,
}

#[cfg(windows)]
const SOURCE_DIALOG: &str = "plugin-source-dialog";

fn forget_covers(ui: &egui::Ui) {
    ui.data_mut(|data| data.remove::<CoverCache>(egui::Id::new(COVERS)));
}

/// 插件封面，按 16:9 居中裁切。`muted` 用于已停用的插件：去色后淡化到 45%，
/// 不同风格的封面停用后都会往卡片底色靠，看起来是同一种褪掉的样子。
fn plugin_cover(
    ui: &mut egui::Ui,
    key: egui::Id,
    width: f32,
    radius: egui::CornerRadius,
    placeholder: &str,
    muted: bool,
    load: impl FnOnce() -> Option<std::result::Result<Vec<u8>, ()>>,
) -> bool {
    let key = if muted { key.with("muted") } else { key };
    let size = egui::vec2(width, width * 9.0 / 16.0);
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let cache_id = egui::Id::new(COVERS);
    let cached = ui.data_mut(|data| {
        let cache = data.get_temp_mut_or_default::<CoverCache>(cache_id);
        let index = cache.iter().position(|(id, _)| *id == key)?;
        let entry = cache.remove(index)?;
        let texture = entry.1.clone();
        cache.push_front(entry);
        Some(texture)
    });
    if !ui.is_rect_visible(rect) {
        return cached.is_some_and(|texture| texture.is_some());
    }
    let frame = ui.ctx().cumulative_pass_nr();
    let queued = cached.is_none()
        && !ui.data_mut(|data| {
            let budget = data.get_temp_mut_or_default::<(u64, u8)>(cache_id.with("decode"));
            if budget.0 != frame {
                *budget = (frame, 0);
            }
            if budget.1 == 2 {
                return false;
            }
            budget.1 += 1;
            true
        });
    let mut pending = queued;
    let texture = if queued {
        ui.ctx().request_repaint();
        None
    } else {
        cached.unwrap_or_else(|| {
            let Some(result) = load() else {
                pending = true;
                return None;
            };
            let texture = result.ok().and_then(|png| {
                let image =
                    image::load_from_memory_with_format(&png, image::ImageFormat::Png).ok()?;
                let (w, h) = (image.width(), image.height());
                let crop_w = w.min((h * 16 / 9).max(1));
                let crop_h = h.min((w * 9 / 16).max(1));
                let mut pixels = image
                    .crop_imm((w - crop_w) / 2, (h - crop_h) / 2, crop_w, crop_h)
                    .thumbnail(768, 432)
                    .to_rgba8();
                if muted {
                    for pixel in pixels.pixels_mut() {
                        let [r, g, b, _] = pixel.0;
                        let gray = (0.299 * f32::from(r) + 0.587 * f32::from(g) + 0.114 * f32::from(b))
                            .round() as u8;
                        pixel.0[..3].fill(gray);
                    }
                }
                let size = [pixels.width() as usize, pixels.height() as usize];
                let image = egui::ColorImage::from_rgba_unmultiplied(size, pixels.as_raw());
                Some(
                    ui.ctx()
                        .load_texture("plugin-cover", image, egui::TextureOptions::LINEAR),
                )
            });
            ui.data_mut(|data| {
                let cache = data.get_temp_mut_or_default::<CoverCache>(cache_id);
                cache.push_front((key, texture.clone()));
                cache.truncate(32);
            });
            texture
        })
    };
    if texture.is_none() || !muted {
        ui.painter().rect_filled(
            rect,
            radius,
            color(
                ui.ctx(),
                Color32::from_rgb(45, 45, 50),
                Color32::from_rgb(233, 236, 241),
            ),
        );
    }
    if let Some(texture) = texture {
        let image = egui::Image::new(&texture).corner_radius(radius);
        let image = if muted {
            image.tint(Color32::from_white_alpha(115))
        } else {
            image
        };
        image.paint_at(ui, rect);
        true
    } else {
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            if pending {
                "正在加载封面…"
            } else {
                placeholder
            },
            egui::FontId::proportional(if width < 100.0 { 10.0 } else { 11.5 }),
            color(ui.ctx(), Color32::from_gray(170), Color32::from_gray(92)),
        );
        false
    }
}

fn plugin_accent(ui: &egui::Ui) -> Color32 {
    color(
        ui.ctx(),
        Color32::from_rgb(140, 127, 208),
        Color32::from_rgb(91, 80, 140),
    )
}

fn muted_color(ui: &egui::Ui) -> Color32 {
    color(ui.ctx(), Color32::from_gray(160), Color32::from_rgb(92, 101, 117))
}

fn line_color(ui: &egui::Ui) -> Color32 {
    color(
        ui.ctx(),
        Color32::from_rgb(58, 58, 63),
        Color32::from_rgb(224, 227, 234),
    )
}

fn card_color(ui: &egui::Ui) -> Color32 {
    color(ui.ctx(), Color32::from_rgb(35, 35, 38), Color32::WHITE)
}

fn warn_color(ui: &egui::Ui) -> Color32 {
    color(
        ui.ctx(),
        Color32::from_rgb(238, 181, 91),
        Color32::from_rgb(139, 88, 12),
    )
}

#[cfg(windows)]
fn ok_color(ui: &egui::Ui) -> Color32 {
    color(
        ui.ctx(),
        Color32::from_rgb(127, 214, 155),
        Color32::from_rgb(29, 122, 60),
    )
}

/// 插件提供的设置项，不论落在哪个设置页（待办、日程、外观材质……），标题旁都用它标出来源。
/// 新增插件设置入口时也调用这里，不单独画。
pub(crate) fn plugin_tag(ui: &mut egui::Ui) {
    small_tag(ui, "插件");
}

/// 名称旁的小标签，例如「官方」「验收样例」「插件」。高度贴着文字，不撑高标题行。
fn small_tag(ui: &mut egui::Ui, label: &str) {
    let ctx = ui.ctx().clone();
    let fg = color(&ctx, Color32::from_rgb(215, 201, 252), Color32::from_rgb(91, 80, 140));
    let bg = color(&ctx, Color32::from_rgb(57, 49, 81), Color32::from_rgb(237, 232, 249));
    let galley = ui.fonts(|fonts| {
        fonts.layout_no_wrap(label.to_owned(), egui::FontId::proportional(10.5), fg)
    });
    let size = galley.size() + egui::vec2(12.0, 2.0);
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    ui.painter().rect_filled(rect, 4.0, bg);
    let pos = rect.min + egui::vec2(6.0, 1.0);
    ui.painter().galley(pos, galley.clone(), fg);
    ui.painter().galley(pos + egui::vec2(0.3, 0.0), galley, fg);
}

/// 最多显示 `rows` 行的说明文字，固定占满这些行的高度，同一排卡片底部对齐。
#[cfg(windows)]
fn clamped_text(ui: &mut egui::Ui, text: &str, size: f32, rows: usize, tint: Color32) {
    let width = ui.available_width();
    let mut job = egui::text::LayoutJob::simple(
        text.to_owned(),
        egui::FontId::proportional(size),
        tint,
        width,
    );
    job.wrap.max_rows = rows;
    job.wrap.break_anywhere = true;
    job.wrap.overflow_character = Some('…');
    let galley = ui.fonts(|fonts| fonts.layout_job(job));
    let height = ui.fonts(|fonts| fonts.row_height(&egui::FontId::proportional(size))) * rows as f32;
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
    let clipped = galley.rows.len() >= rows && galley.text() != text;
    ui.painter().galley(rect.min, galley, tint);
    if clipped {
        response.on_hover_text(text);
    }
}

/// 两段或多段的切换条。返回被点中的序号。`dots` 为真的一段右上角带一个紫点。
fn segmented(
    ui: &mut egui::Ui,
    id: &str,
    options: &[String],
    selected: usize,
    dots: &[bool],
    size: f32,
) -> Option<usize> {
    segmented_counts(ui, id, options, &[], selected, dots, size)
}

/// 切换条，`counts` 里的数字用小一号的弱化颜色跟在名字后面。
fn segmented_counts(
    ui: &mut egui::Ui,
    id: &str,
    options: &[String],
    counts: &[Option<usize>],
    selected: usize,
    dots: &[bool],
    size: f32,
) -> Option<usize> {
    let ctx = ui.ctx().clone();
    let mut chosen = None;
    egui::Frame::new()
        .fill(color(&ctx, Color32::from_rgb(26, 26, 30), Color32::from_rgb(240, 241, 245)))
        .stroke(egui::Stroke::new(1.0, color(&ctx, Color32::from_rgb(46, 46, 52), Color32::from_rgb(230, 232, 238))))
        .corner_radius(9.0)
        .inner_margin(egui::Margin::same(3))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                // 放在右对齐的标题行时，egui 会从右往左排；倒着画，保证第一项始终在左边。
                let mut order: Vec<_> = options.iter().enumerate().collect();
                if ui.layout().prefer_right_to_left() {
                    order.reverse();
                }
                for (index, option) in order {
                    let on = index == selected;
                    let font = egui::FontId::proportional(size);
                    let text_color = if on {
                        color(&ctx, Color32::WHITE, Color32::from_rgb(29, 34, 48))
                    } else {
                        muted_color(ui)
                    };
                    let galley = ui.fonts(|fonts| fonts.layout_no_wrap(option.clone(), font, text_color));
                    let count_color = muted_color(ui);
                    let count = counts.get(index).copied().flatten().map(|count| {
                        ui.fonts(|fonts| {
                            fonts.layout_no_wrap(
                                count.to_string(),
                                egui::FontId::proportional(size - 2.0),
                                count_color,
                            )
                        })
                    });
                    let dot = dots.get(index).copied().unwrap_or(false);
                    let count_w = count.as_ref().map_or(0.0, |count| count.size().x + 5.0);
                    let extra = if dot { 10.0 } else { 0.0 } + count_w;
                    let pad = if size < 13.0 { 10.0 } else { 14.0 };
                    let height = if size < 13.0 { 22.0 } else { 26.0 };
                    let desired = egui::vec2(galley.size().x + pad * 2.0 + extra, height);
                    let (rect, _) = ui.allocate_exact_size(desired, egui::Sense::hover());
                    let response = ui.interact(rect, egui::Id::new((id, index)), egui::Sense::click());
                    response.widget_info(|| {
                        egui::WidgetInfo::selected(egui::WidgetType::Button, true, on, option)
                    });
                    if on {
                        ui.painter().rect_filled(
                            rect.translate(egui::vec2(0.0, 1.0)),
                            7.0,
                            Color32::from_black_alpha(18),
                        );
                        ui.painter().rect_filled(
                            rect,
                            7.0,
                            color(&ctx, Color32::from_rgb(56, 56, 63), Color32::WHITE),
                        );
                    } else if response.hovered() {
                        ui.painter().rect_filled(
                            rect,
                            7.0,
                            color(&ctx, Color32::from_rgb(38, 38, 44), Color32::from_rgb(232, 234, 239)),
                        );
                    }
                    if response.has_focus() {
                        ui.painter().rect_stroke(
                            rect,
                            7.0,
                            egui::Stroke::new(1.0, plugin_accent(ui)),
                            egui::StrokeKind::Inside,
                        );
                    }
                    let text_pos = egui::pos2(rect.left() + pad, rect.center().y - galley.size().y / 2.0);
                    if on {
                        // 选中项加粗：同一行文字叠一像素绘制，宽度不跳动。
                        ui.painter().galley(text_pos + egui::vec2(0.35, 0.0), galley.clone(), text_color);
                    }
                    let text_w = galley.size().x;
                    ui.painter().galley(text_pos, galley, text_color);
                    if let Some(count) = count {
                        let pos = egui::pos2(
                            text_pos.x + text_w + 5.0,
                            rect.center().y - count.size().y / 2.0,
                        );
                        ui.painter().galley(pos, count, muted_color(ui));
                    }
                    if dot {
                        ui.painter().circle_filled(
                            egui::pos2(rect.right() - pad - 3.0, rect.center().y),
                            3.5,
                            plugin_accent(ui),
                        );
                    }
                    if response.clicked() {
                        chosen = Some(index);
                    }
                }
            });
        });
    chosen.filter(|&index| index != selected)
}

/// 筛选用的圆角小标签。
#[cfg(windows)]
fn chip(ui: &mut egui::Ui, text: &str, on: bool) -> egui::Response {
    let ctx = ui.ctx().clone();
    let fill = if on { plugin_accent(ui) } else { card_color(ui) };
    let text_color = if on { Color32::WHITE } else { muted_color(ui) };
    let stroke = if on { plugin_accent(ui) } else { line_color(ui) };
    let response = ui.add(
        egui::Button::new(egui::RichText::new(text).size(12.0).color(text_color))
            .fill(fill)
            .stroke(egui::Stroke::new(1.0, stroke))
            .corner_radius(11.0)
            .min_size(egui::vec2(0.0, 22.0)),
    );
    let _ = ctx;
    response
}

const PLUGIN_SWITCH_WIDTH: f32 = 38.0;

/// 插件开关统一用紫色表示开启、灰色表示关闭。
fn plugin_switch(ui: &mut egui::Ui, id: &str, on: bool, label: &str) -> bool {
    plugin_switch_sized(ui, id, on, label, egui::vec2(PLUGIN_SWITCH_WIDTH, 22.0))
}

fn plugin_switch_sized(ui: &mut egui::Ui, id: &str, on: bool, label: &str, size: egui::Vec2) -> bool {
    let (_, rect) = ui.allocate_space(size);
    let radius = size.y / 2.0;
    let response = ui.interact(
        rect,
        egui::Id::new(("settings-plugin-switch", id)),
        egui::Sense::click(),
    );
    response
        .widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, on, label));
    let t = ui
        .ctx()
        .animate_bool_with_time(response.id.with("t"), on, 0.15);
    let off = color(
        ui.ctx(),
        Color32::from_gray(86),
        Color32::from_rgb(203, 207, 214),
    );
    ui.painter()
        .rect_filled(rect, radius, paint::mix(off, plugin_accent(ui), t));
    if response.has_focus() {
        ui.painter().rect_stroke(
            rect.expand(2.0),
            radius + 2.0,
            egui::Stroke::new(1.0, plugin_accent(ui)),
            egui::StrokeKind::Outside,
        );
    }
    let x = egui::lerp((rect.left() + radius)..=(rect.right() - radius), t);
    ui.painter()
        .circle_filled(egui::pos2(x, rect.center().y), radius - 3.0, Color32::WHITE);
    response.on_hover_text(label).clicked()
}

fn plugin_setting_switch_row(
    ui: &mut egui::Ui,
    id: &str,
    title: &str,
    description: &str,
    on: bool,
    tagged: bool,
) -> bool {
    ui.horizontal(|ui| {
        let text_width =
            (ui.available_width() - PLUGIN_SWITCH_WIDTH - ui.spacing().item_spacing.x).max(0.0);
        ui.vertical(|ui| {
            ui.set_max_width(text_width);
            ui.horizontal(|ui| {
                ui.scope(|ui| {
                    if tagged {
                        ui.set_max_width((ui.available_width() - 44.0).max(0.0));
                    }
                    ui.add(egui::Label::new(title).truncate()).on_hover_text(title);
                });
                if tagged { plugin_tag(ui); }
            });
            settings_hint(ui, description);
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            plugin_switch(ui, id, on, title)
        })
        .inner
    })
    .inner
}

/// 已安装列表里的「路径」：显示设置归属，点一下就是前往设置。
fn path_button(ui: &mut egui::Ui, text: &str, warn: bool, tip: &str) -> bool {
    let base = if warn { warn_color(ui) } else { muted_color(ui) };
    let id = ui.id().with(("plugin-path", text));
    let hovered = ui.ctx().animate_bool_with_time(id, ui.ctx().read_response(id).is_some_and(|r| r.hovered()), 0.1);
    let tint = paint::mix(base, plugin_accent(ui), if warn { 0.0 } else { hovered });
    let galley = ui.fonts(|fonts| {
        fonts.layout_no_wrap(text.to_owned(), egui::FontId::proportional(12.0), tint)
    });
    let (rect, _) = ui.allocate_exact_size(galley.size(), egui::Sense::hover());
    let response = ui
        .interact(rect, id, egui::Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Link, true, text));
    ui.painter().galley(rect.min, galley, tint);
    if response.has_focus() {
        ui.painter().rect_stroke(
            rect.expand(2.0),
            3.0,
            egui::Stroke::new(1.0, plugin_accent(ui)),
            egui::StrokeKind::Outside,
        );
    }
    response.on_hover_text(tip).clicked()
}

#[cfg(windows)]
fn market_button(ui: &egui::Ui, text: &str) -> egui::Button<'static> {
    egui::Button::new(egui::RichText::new(text).color(Color32::WHITE))
        .fill(plugin_accent(ui))
        .corner_radius(6.0)
}

/// 弹窗里的大号选项：粗体标题加一行后果说明，整块可点。
fn option_card(ui: &mut egui::Ui, title: &str, detail: &str, danger: bool) -> bool {
    let ctx = ui.ctx().clone();
    let id = ui.id().with(("plugin-option", title));
    let hovered = ctx.read_response(id).is_some_and(|r| r.hovered());
    let frame = egui::Frame::new()
        .fill(card_color(ui))
        .stroke(egui::Stroke::new(
            1.0,
            if hovered { plugin_accent(ui) } else { line_color(ui) },
        ))
        .corner_radius(9.0)
        .inner_margin(egui::Margin::symmetric(12, 9))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 2.0;
            let title_color = if danger {
                color(&ctx, Color32::from_rgb(240, 138, 128), Color32::from_rgb(180, 55, 44))
            } else {
                ui.visuals().strong_text_color()
            };
            ui.add(
                egui::Label::new(egui::RichText::new(title).strong().color(title_color))
                    .selectable(false),
            );
            ui.add(
                egui::Label::new(egui::RichText::new(detail).size(12.0).color(muted_color(ui)))
                    .selectable(false)
                    .wrap(),
            );
        });
    let response = ui
        .interact(frame.response.rect, id, egui::Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, title));
    response.clicked()
}

#[cfg(windows)]
#[derive(Clone, Copy, PartialEq, Eq)]
enum PluginPage {
    Market,
    Mine,
    TestMarket,
}

/// 「开发者工具」入口：代码图标加正常字色，悬停变紫并出现下划线，一看就能点。
#[cfg(windows)]
fn developer_link(ui: &mut egui::Ui) -> bool {
    let text = "开发者工具";
    let id = egui::Id::new("plugin-developer-link");
    let hovered = ui.ctx().read_response(id).is_some_and(|r| r.hovered());
    let tint = if hovered {
        plugin_accent(ui)
    } else {
        ui.visuals().strong_text_color()
    };
    let galley = ui.fonts(|fonts| {
        fonts.layout_no_wrap(text.to_owned(), egui::FontId::proportional(12.5), tint)
    });
    let size = egui::vec2(18.0 + galley.size().x, galley.size().y.max(18.0));
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let response = ui
        .interact(rect, id, egui::Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, text));
    let c = egui::pos2(rect.left() + 6.0, rect.center().y);
    let stroke = egui::Stroke::new(1.4, tint);
    let painter = ui.painter();
    painter.line_segment([c + egui::vec2(-1.0, -4.0), c + egui::vec2(-5.0, 0.0)], stroke);
    painter.line_segment([c + egui::vec2(-5.0, 0.0), c + egui::vec2(-1.0, 4.0)], stroke);
    painter.line_segment([c + egui::vec2(3.0, -4.0), c + egui::vec2(7.0, 0.0)], stroke);
    painter.line_segment([c + egui::vec2(7.0, 0.0), c + egui::vec2(3.0, 4.0)], stroke);
    let text_pos = egui::pos2(rect.left() + 18.0, rect.center().y - galley.size().y / 2.0);
    let text_w = galley.size().x;
    painter.galley(text_pos, galley, tint);
    if hovered {
        let y = text_pos.y + 17.0;
        painter.hline(text_pos.x..=text_pos.x + text_w, y, egui::Stroke::new(1.0, tint));
    }
    if response.has_focus() {
        painter.rect_stroke(rect.expand(2.0), 4.0, egui::Stroke::new(1.0, plugin_accent(ui)), egui::StrokeKind::Outside);
    }
    response.clicked()
}

/// 设置归属在界面上的名字，顺序与设置侧栏一致。
const OWNERS: [(&str, &str); 8] = [
    ("appearance", "外观"),
    ("pages", "页面管理"),
    ("status", "独立功能"),
    ("calendar", "日程"),
    ("tasks", "待办与置物"),
    ("quota", "AI 额度"),
    ("wireless", "无线设备"),
    ("mail", "邮箱"),
];

fn owner_label(owner: &str) -> Option<&'static str> {
    OWNERS
        .iter()
        .find(|(key, _)| *key == owner)
        .map(|(_, label)| *label)
}

fn owner_settings_tab(owner: &str) -> usize {
    match owner {
        "calendar" => crate::app::PAGE_SETTINGS_TABS[crate::app::CALENDAR_PAGE],
        "quota" => crate::app::PAGE_SETTINGS_TABS[crate::app::QUOTA_PAGE],
        "mail" => crate::app::PAGE_SETTINGS_TABS[crate::app::MAIL_PAGE],
        "pages" | "status" => crate::extras::SETTINGS_TAB_PAGES,
        "wireless" => crate::extras::SETTINGS_TAB_WIRELESS,
        "appearance" => crate::extras::SETTINGS_TAB_APPEARANCE,
        _ => crate::app::PAGE_SETTINGS_TABS[crate::app::TODO_PAGE],
    }
}

fn standalone_settings_page(manifest: &Manifest) -> bool {
    manifest.is_status_page()
}

// 官方面板按实际用途分组。其他清单字段保持原来的显示顺序。
fn settings_section(id: &str, key: &str) -> (u8, &'static str) {
    match (id, key) {
        ("daozai.gold", "unit" | "range" | "up_color") => (1, "报价"),
        ("daozai.gold", "alert_usd" | "alert_cny") => (2, "价格提醒"),
        ("daozai.live-pay", "salary" | "days" | "start" | "end") => (1, "薪资与班次"),
        ("daozai.live-pay", "breaks") => (2, "休息时段"),
        ("daozai.world-clock", "cities") => (1, "城市"),
        ("daozai.days", "days") => (1, "我的日子"),
        ("daozai.days", "show_holidays") => (2, "显示"),
        ("daozai.notes", "pin_mode")
        | ("daozai.live-pay", "pinned_show")
        | ("daozai.world-clock", "pinned_city")
        | ("daozai.days", "pinned_day") => (3, "常驻内容"),
        _ => (0, "设置"),
    }
}

/// 已安装分组的小标题，例如「外观 · 每类外观同时选用一个」。
fn group_header(ui: &mut egui::Ui, title: &str, note: &str) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        ui.label(egui::RichText::new(title).size(12.0).strong().color(muted_color(ui)));
        if !note.is_empty() {
            ui.label(egui::RichText::new(note).size(11.5).color(muted_color(ui)));
        }
    });
    ui.add_space(2.0);
}

/// 一组插件共用一张卡片，行与行之间用细线分开。
fn plugin_list(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(card_color(ui))
        .stroke(egui::Stroke::new(1.0, line_color(ui)))
        .corner_radius(10.0)
        .inner_margin(egui::Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            add(ui);
        });
}

fn row_separator(ui: &mut egui::Ui) {
    ui.add_space(4.0);
    let y = ui.cursor().top();
    let line = color(ui.ctx(), Color32::from_rgb(46, 46, 52), Color32::from_rgb(236, 238, 243));
    ui.painter()
        .hline(ui.max_rect().x_range(), y, egui::Stroke::new(1.0, line));
    ui.add_space(5.0);
}

#[cfg(all(windows, feature = "sdk-material"))]
const CAPTURE_PERMISSION: &str = "读取岛体背后一小块屏幕画面，只在本机用于绘制材质，不保存、不上传；插件本身没有网络和文件权限。";

#[cfg(all(windows, feature = "sdk-material"))]
fn material_name(ui: &egui::Ui, name: &str, selected: bool, active: bool) -> egui::RichText {
    let title = egui::RichText::new(name);
    if !selected {
        return title;
    }
    let ctx = ui.ctx();
    let color = if active {
        color(
            ctx,
            Color32::from_rgb(127, 214, 155),
            Color32::from_rgb(29, 122, 60),
        )
    } else if crate::bar_layout::animations_enabled() {
        let time = ui.input(|input| input.time);
        let phase = (0.5 + 0.5 * (time * std::f64::consts::TAU * 0.8).sin()) as f32;
        ctx.request_repaint_after(Duration::from_millis(80));
        crate::paint::mix(
            color(
                ctx,
                Color32::from_rgb(216, 162, 86),
                Color32::from_rgb(139, 88, 12),
            ),
            color(
                ctx,
                Color32::from_rgb(250, 204, 135),
                Color32::from_rgb(165, 111, 27),
            ),
            phase,
        )
    } else {
        color(
            ctx,
            Color32::from_rgb(238, 181, 91),
            Color32::from_rgb(139, 88, 12),
        )
    };
    title.color(color)
}

#[cfg(all(windows, feature = "sdk-material"))]
fn material_changed(ui: &egui::Ui, result: Result<()>) -> bool {
    let id = egui::Id::new("settings-material-error");
    match result {
        Ok(()) => {
            ui.data_mut(|data| data.remove::<String>(id));
            ui.ctx().request_repaint_of(egui::ViewportId::ROOT);
            true
        }
        Err(error) => {
            ui.data_mut(|data| data.insert_temp(id, error.to_string()));
            false
        }
    }
}

#[cfg(all(windows, feature = "sdk-material"))]
fn material_error(ui: &mut egui::Ui) {
    if let Some(error) =
        ui.data(|data| data.get_temp::<String>(egui::Id::new("settings-material-error")))
    {
        crate::extras::settings_hint(ui, &error);
    }
}

impl LocalPlugins {
    pub(crate) fn page_shortcut_settings(&self, ui: &mut egui::Ui, show: &mut bool, hide_arrows: &mut bool) -> bool {
        if self.active_page_shortcut_colors().is_none() {
            return false;
        }
        let display_changed = plugin_setting_switch_row(
            ui,
            "page-shortcuts-display",
            "快捷切换图标",
            "在岛仔展开后的底部与页码点同排显示",
            *show,
            true,
        );
        if display_changed {
            *show = !*show;
        }
        ui.separator();
        let arrows_changed = plugin_setting_switch_row(
            ui,
            "page-shortcuts-hide-arrows",
            "隐藏左右翻页箭头",
            "显示快捷切换图标时生效",
            *hide_arrows,
            true,
        );
        if arrows_changed {
            *hide_arrows = !*hide_arrows;
        }
        if display_changed || arrows_changed {
            ui.ctx().request_repaint_of(egui::ViewportId::ROOT);
        }
        true
    }

    #[cfg(windows)]
    pub(crate) fn wireless_settings(
        &mut self,
        ui: &mut egui::Ui,
        enhancement: &crate::bt::halo_component::Component,
        wireless_on: bool,
    ) {
        let Some(record) = self
            .records
            .get(WIRELESS_PLUGIN_ID)
            .filter(|record| record.from_market)
        else {
            return;
        };
        let Some(package) = record.package.as_ref() else {
            return;
        };
        let enabled = record.enabled;
        let version = format!("{} v{}", package.manifest.name, package.manifest.version);
        let error_id = ui.id().with("wireless-compat-enable-error");
        let mut toggle = false;
        let mut enhancement_toggle = false;
        let mut enhancement_update = false;
        let component = enhancement.status();
        let can_start = enabled && wireless_on;

        crate::extras::settings_label(ui, "设备兼容");
        settings_card(ui, |ui| {
            toggle = plugin_setting_switch_row(
                ui, WIRELESS_PLUGIN_ID, "启用兼容包", &version, enabled, true,
            );
            ui.add_enabled_ui(can_start || component.enabled, |ui| {
                enhancement_toggle = plugin_setting_switch_row(
                    ui, "wireless-battery-component", "电量增强",
                    if component.busy { "正在准备 HaloBattery 采集组件…" }
                    else if component.installed { "HaloBattery 采集组件已就绪" }
                    else { "首次开启时单独下载 HaloBattery 采集组件" },
                    component.enabled, true,
                );
            });
            if let Some(error) = &component.error {
                settings_hint(ui, error);
            }
            if can_start && component.enabled && !component.busy {
                enhancement_update = ui.small_button(
                    if component.installed { "检查组件更新" } else { "重试下载" }
                ).clicked();
            }
            egui::CollapsingHeader::new(egui::RichText::new("兼容接收器").size(12.5))
                .id_salt("wireless-compat-receivers")
                .show(ui, |ui| {
                    let Ok(table) = package.wireless_table() else {
                        settings_hint(ui, "无法读取兼容接收器名单");
                        return;
                    };
                    let receivers = table.receivers();
                    settings_hint(
                        ui,
                        &format!("此包补充识别 {} 个 2.4G USB 接收器，可按 USB ID 核对。", receivers.len()),
                    );
                    egui::Grid::new("wireless-compat-receiver-list")
                        .num_columns(2)
                        .min_row_height(18.0)
                        .max_col_width((ui.available_width() - 90.0).max(100.0))
                        .spacing([16.0, 6.0])
                        .show(ui, |ui| {
                            for ((vid, pid), name) in receivers {
                                ui.add(egui::Label::new(egui::RichText::new(name).size(12.5)).wrap());
                                settings_hint(ui, &format!("{vid:04X}:{pid:04X}"));
                                ui.end_row();
                            }
                        });
                });
            ui.hyperlink_to("反馈设备适配", WIRELESS_FEEDBACK_URL);
            if let Some(error) = ui.data(|data| data.get_temp::<Option<String>>(error_id).flatten()) {
                settings_hint(ui, &error);
            }
        });
        if toggle {
            let error = self
                .enable(WIRELESS_PLUGIN_ID, !enabled)
                .err()
                .map(|error| error.to_string());
            if error.is_none() {
                ui.ctx().request_repaint_of(egui::ViewportId::ROOT);
            }
            ui.data_mut(|data| data.insert_temp(error_id, error));
            ui.ctx().request_repaint();
        }
        if enhancement_toggle || enhancement_update {
            let ctx = ui.ctx().clone();
            let wake = move || ctx.request_repaint();
            if enhancement_toggle {
                if let Err(error) = enhancement.set_enabled(!component.enabled, wake) {
                    ui.data_mut(|data| data.insert_temp(error_id, Some(error)));
                }
            } else {
                enhancement.check_update(wake);
            }
            ui.ctx().request_repaint();
        }
    }

    pub fn card(&mut self, c: &Card) -> bool {
        let mut cards = self.cards.iter();
        let count = cards.clone().count();
        if count == 0 {
            return false;
        }
        self.card_index %= count;
        let (id, body) = cards.nth(self.card_index).unwrap();
        c.fill(42., 130., 335., 40., 9., theme::CARD);
        let manifest = &self.records[id].package.as_ref().unwrap().manifest;
        let has_settings = manifest.has_settings() || manifest.has_surface(Surface::IslandPinned);
        c.text(
            52.,
            145.,
            &c.fit(&manifest.name, 11., W::Regular, 220.),
            11.,
            W::Regular,
            theme::CYAN,
            H::Left,
        );
        c.text(
            52.,
            163.,
            &c.fit(
                body,
                14.,
                W::Semibold,
                if has_settings { 290. } else { 305. },
            ),
            14.,
            W::Semibold,
            theme::WHITE,
            H::Left,
        );
        if count > 1 {
            c.text(
                if has_settings { 343. } else { 367. },
                145.,
                &format!("{}/{} ›", self.card_index + 1, count),
                11.,
                W::Regular,
                theme::GRAY,
                H::Right,
            );
            if c.hit(
                42.,
                130.,
                if has_settings { 307. } else { 335. },
                40.,
                "local-plugin-next",
            )
            .on_hover_text("下一张卡片")
            .clicked()
            {
                self.card_index = (self.card_index + 1) % count;
            }
        }
        if has_settings {
            let response = c
                .hit(349., 130., 28., 40., "local-plugin-settings")
                .on_hover_text("打开插件设置");
            let (hover, press) = c.hover_t(&response);
            c.circle(
                363.,
                145.,
                11.,
                egui::Color32::from_white_alpha((hover * 24.) as u8),
            );
            paint::gear(
                &c.scaled(363., 145., 0.8 * (1. - press * 0.08)),
                363.,
                145.,
                paint::mix(theme::GRAY, theme::WHITE, hover),
            );
            if response.clicked() {
                self.focus = Some(id.clone());
                return true;
            }
        }
        false
    }

    fn visible_ids(&self) -> Vec<String> {
        self.records
            .iter()
            .filter(|(_, r)| {
                r.package.as_ref().is_some_and(|p| {
                    cfg!(all(windows, feature = "sdk-material")) || !p.manifest.is_material()
                })
            })
            .map(|(id, _)| id.clone())
            .collect()
    }

    #[cfg(windows)]
    fn current_page(&self, ui: &egui::Ui, nothing_installed: bool) -> PluginPage {
        ui.data(|data| data.get_temp::<PluginPage>(egui::Id::new("settings-plugins-market")))
            .unwrap_or(if nothing_installed && self.invalid.is_empty() {
                PluginPage::Market
            } else {
                PluginPage::Mine
            })
    }

    /// 标题行右侧的「集市 / 已安装」切换条。已安装后面跟弱化的数量，有可更新的插件时带紫点。
    pub fn header_tabs(&mut self, ui: &mut egui::Ui) {
        #[cfg(windows)]
        {
            let ids = self.visible_ids();
            let key = egui::Id::new("settings-plugins-market");
            let previous = self.current_page(ui, ids.is_empty());
            let updates = ids.iter().filter(|id| self.market_update(id).is_some()).count();
            let options = ["集市".to_owned(), "已安装".to_owned()];
            let counts = [None, Some(ids.len() + self.invalid.len())];
            let selected = usize::from(previous == PluginPage::Mine);
            if let Some(index) = segmented_counts(
                ui,
                "settings-plugin-tabs",
                &options,
                &counts,
                selected,
                &[false, updates > 0],
                13.0,
            ) {
                let page = if index == 0 { PluginPage::Market } else { PluginPage::Mine };
                market_browse_ui::close_detail(ui);
                if page == PluginPage::Market {
                    forget_covers(ui);
                    self.refresh_official_catalog();
                }
                ui.data_mut(|data| data.insert_temp(key, page));
            }
        }
        #[cfg(not(windows))]
        let _ = ui;
    }

    /// 设置 → 插件。顶部「集市 / 已安装」两段切换；已安装按设置归属分组，
    /// 每行只有路径（前往设置）、开关和「···」三样。
    pub(crate) fn manage(
        &mut self,
        ui: &mut egui::Ui,
        tasks_on: bool,
        calendar_on: bool,
        quota_on: bool,
        #[cfg(all(windows, feature = "sdk-material"))] active_material: Option<&str>,
    ) -> Option<SettingsNavigation> {
        let mut jump = None;
        let ids = self.visible_ids();
        #[cfg(windows)]
        let page_id = egui::Id::new("settings-plugins-market");
        #[cfg(windows)]
        let page = {
            let was_busy = self.market_busy();
            self.poll_market();
            if self.market_pending() {
                ui.ctx().request_repaint_after(Duration::from_millis(32));
            }
            if was_busy && !self.market_busy() {
                ui.ctx().request_repaint_of(egui::ViewportId::ROOT);
            }
            let page = self.current_page(ui, ids.is_empty());
            ui.data_mut(|data| data.insert_temp(page_id, page));
            if page == PluginPage::Mine && self.records.values().any(|record| record.official_market) {
                self.ensure_official_catalog();
            }
            page
        };
        if !self.notice.is_empty() {
            settings_hint(ui, &self.notice);
            ui.add_space(4.0);
        }
        #[cfg(windows)]
        if page != PluginPage::Mine {
            match self.market(ui, page == PluginPage::Market) {
                market_browse_ui::MarketNav::Stay => {}
                market_browse_ui::MarketNav::Mine => {
                    ui.data_mut(|data| data.insert_temp(page_id, PluginPage::Mine));
                    ui.ctx().request_repaint();
                }
                market_browse_ui::MarketNav::Source { official } => {
                    forget_covers(ui);
                    self.notice.clear();
                    let target = if official { PluginPage::Market } else { PluginPage::TestMarket };
                    ui.data_mut(|data| data.insert_temp(page_id, target));
                    ui.ctx().request_repaint();
                }
            }
            if self.market_pending() {
                ui.ctx().request_repaint_after(Duration::from_millis(32));
            }
            // 教程里的「从模板创建」「打开开发者工具」在集市页直接弹出，盖在教程上面。
            self.guide_drawer(ui);
            if self.developer_dialog(ui) {
                ui.data_mut(|data| data.insert_temp(page_id, PluginPage::TestMarket));
            }
            plugin_feedback(ui);
            return None;
        }
        if ids.is_empty() && self.invalid.is_empty() {
            settings_card(ui, |ui| {
                ui.strong("还没有安装插件");
                settings_hint(ui, "装好后可以在这里管理。");
                #[cfg(windows)]
                if ui.add(market_button(ui, "去插件集市")).clicked() {
                    ui.data_mut(|data| data.insert_temp(page_id, PluginPage::Market));
                }
            });
        }
        // 按插件清单写明的设置归属分组，顺序与设置侧栏一致；不认识的归属单独成组，不并进别的组。
        let owner_of = |id: &String| {
            self.records[id]
                .package
                .as_ref()
                .map(|package| package.manifest.owner.clone())
                .unwrap_or_default()
        };
        let mut groups: Vec<(String, Vec<String>)> = OWNERS
            .iter()
            .map(|(key, _)| ((*key).to_owned(), Vec::new()))
            .collect();
        for id in ids {
            let owner = owner_of(&id);
            match groups.iter_mut().find(|(key, _)| *key == owner) {
                Some((_, group)) => group.push(id),
                None => groups.push((owner, vec![id])),
            }
        }
        for (group_owner, group) in groups {
            if group.is_empty() {
                continue;
            }
            let title = owner_label(&group_owner).unwrap_or(group_owner.as_str());
            let note = if group_owner == "appearance" { "每类外观同时选用一个" } else { "" };
            group_header(ui, title, note);
            plugin_list(ui, |ui| {
                for (index, id) in group.iter().enumerate() {
                    if index > 0 {
                        row_separator(ui);
                    }
                    ui.push_id(id, |ui| {
                        self.installed_row(
                            ui,
                            id,
                            tasks_on,
                            calendar_on,
                            quota_on,
                            #[cfg(all(windows, feature = "sdk-material"))]
                            active_material,
                            &mut jump,
                        );
                    });
                }
            });
            ui.add_space(14.0);
        }
        if !self.invalid.is_empty() {
            group_header(ui, "需要处理", "");
            let invalid: Vec<_> = self.invalid.iter().cloned().collect();
            plugin_list(ui, |ui| {
                for (index, id) in invalid.iter().enumerate() {
                    if index > 0 {
                        row_separator(ui);
                    }
                    ui.push_id(("invalid", id), |ui| self.invalid_row(ui, id));
                }
            });
            ui.add_space(14.0);
        }
        self.uninstall_dialog(ui);
        #[cfg(windows)]
        {
            ui.add_space(2.0);
            let line = line_color(ui);
            let top = ui.cursor().top();
            ui.painter().hline(
                ui.max_rect().x_range(),
                top,
                egui::Stroke::new(1.0, line),
            );
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                if developer_link(ui) {
                    ui.data_mut(|data| data.insert_temp(egui::Id::new("plugin-developer-tools"), true));
                }
                ui.label(
                    egui::RichText::new("创建、校验与试装插件")
                        .size(11.5)
                        .color(muted_color(ui)),
                );
            });
            if self.developer_dialog(ui) {
                ui.data_mut(|data| data.insert_temp(page_id, PluginPage::TestMarket));
            }
        }
        #[cfg(windows)]
        self.detail_drawer(ui, true);
        #[cfg(windows)]
        self.source_dialog(ui);
        #[cfg(all(windows, feature = "sdk-material"))]
        {
            if self.confirm_material_permission(ui) {
                jump = Some(SettingsNavigation::Tab(crate::extras::SETTINGS_TAB_APPEARANCE));
            }
            material_error(ui);
        }
        plugin_feedback(ui);
        jump
    }

    /// 已安装列表的一行：缩略图、名称和版本、路径；右侧是更新、开关和「···」。
    fn installed_row(
        &mut self,
        ui: &mut egui::Ui,
        id: &str,
        tasks_on: bool,
        calendar_on: bool,
        quota_on: bool,
        #[cfg(all(windows, feature = "sdk-material"))] active_material: Option<&str>,
        jump: &mut Option<SettingsNavigation>,
    ) {
        let r = &self.records[id];
        let package = r.package.as_ref().unwrap();
        let manifest = &package.manifest;
        let material = manifest.is_material();
        let appearance = manifest.appearance_kind();
        let owner_key = manifest.owner.clone();
        let retired = manifest.is_retired_lunar_preview();
        #[cfg(windows)]
        let revoked = r.market_revoked;
        #[cfg(not(windows))]
        let revoked = false;
        let module_on = match owner_key.as_str() {
            "calendar" => calendar_on,
            "quota" => quota_on,
            "status" => !r.status_page_disabled,
            "pages" | "wireless" | "appearance" | "mail" => true,
            _ => tasks_on,
        };
        let has_settings = manifest.has_settings() || manifest.has_surface(Surface::IslandPinned);
        #[cfg(windows)]
        let has_source = manifest.source_kind().is_some();
        let key = egui::Id::new(("installed", &r.origin, id, &manifest.version));
        let cover = package.cover.clone();
        let runtime_error = self.errors.get(id).cloned();
        let (name, author, version) = (
            manifest.name.clone(),
            manifest.author.clone(),
            manifest.version.clone(),
        );
        #[cfg(all(windows, feature = "sdk-material"))]
        let selected = self.selected_material_id() == Some(id);
        #[cfg(not(all(windows, feature = "sdk-material")))]
        let selected = false;
        let on = if material { selected } else { r.enabled };
        #[cfg(windows)]
        let (listed, official_mark, preview_sample, update) = {
            let catalog = self.market_catalog(true);
            let entry = catalog
                .as_ref()
                .and_then(|catalog| catalog.entries.iter().find(|entry| entry.id == id));
            let presentation = match entry {
                Some(entry) => entry.presentation.as_ref(),
                None => r.presentation.as_ref(),
            };
            let trusted = presentation.filter(|_| r.official_market);
            (
                entry.is_some(),
                trusted.is_some_and(|meta| meta.official),
                trusted.is_some_and(|meta| meta.sample),
                self.market_update(id),
            )
        };
        #[cfg(windows)]
        let author = market_browse_ui::display_author(&author, official_mark).to_owned();
        let standalone = standalone_settings_page(manifest);
        let paused = !material && !standalone && !module_on;
        // 路径和跳转都按插件清单写明的设置归属，不写死某一页。
        let owner_key = owner_key.as_str();
        let place = owner_label(owner_key).unwrap_or(owner_key);
        let (path, tip) = if material {
            ("设置 › 外观 › 材质 ›".to_owned(), "前往外观设置".to_owned())
        } else if let Some(kind) = appearance {
            (format!("设置 › 外观 › {} ›", kind.label()), "前往外观设置".to_owned())
        } else if standalone {
            (format!("设置 › {name} ›"), "前往插件设置".to_owned())
        } else if owner_key == "status" {
            (
                format!("页面管理 › {place} ›"),
                "管理这个插件的详情页和数据读取".to_owned(),
            )
        } else if paused {
            (
                format!("页面管理 › {place} ›"),
                format!("「{place}」在页面管理里关着，打开后插件才会运行"),
            )
        } else {
            (format!("设置 › {place} ›"), "前往设置".to_owned())
        };
        let row_h = if runtime_error.is_some() { 58.0 } else { 42.0 };
        let row_w = ui.available_width();
        ui.allocate_ui_with_layout(
            egui::vec2(row_w, row_h),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
            ui.set_min_height(row_h);
            ui.spacing_mut().item_spacing.x = 12.0;
            plugin_cover(ui, key, 64.0, egui::CornerRadius::same(6), "暂无封面", !on, || {
                Some(cover.as_ref().map(|cover| cover.bytes().to_vec()).ok_or(()))
            });
            #[cfg(windows)]
            let actions_w = if update.is_some() { 152.0 } else { 96.0 };
            #[cfg(not(windows))]
            let actions_w = 96.0;
            let info_w = (ui.available_width() - actions_w).max(80.0);
            ui.allocate_ui_with_layout(
                egui::vec2(info_w, row_h),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.set_width(info_w);
                    ui.spacing_mut().item_spacing.y = 3.0;
                    let text_h = if runtime_error.is_some() { 52.0 } else { 36.0 };
                    ui.add_space(((row_h - text_h) / 2.0).max(0.0));
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        let title = egui::RichText::new(&name).size(13.5).strong();
                        #[cfg(all(windows, feature = "sdk-material"))]
                        let title = if material {
                            material_name(ui, &name, selected, active_material == Some(id))
                                .size(13.5)
                                .strong()
                        } else {
                            title
                        };
                        ui.add(egui::Label::new(title).truncate())
                            .on_hover_text(format!("{name}\n{author} · v{version}"));
                        let version_color = if revoked { warn_color(ui) } else { muted_color(ui) };
                        let version_label = ui.label(
                            egui::RichText::new(format!("v{version}"))
                                .size(11.0)
                                .color(version_color),
                        );
                        if revoked {
                            version_label.on_hover_text("此版本已撤回");
                        }
                        #[cfg(windows)]
                        market_browse_ui::show_badge(ui, official_mark, preview_sample);
                    });
                    if retired && !revoked {
                        settings_hint(ui, "已撤回 · 等待更新");
                    } else if !revoked && owner_key == "pages" && !on {
                        settings_hint(ui, "启用后可在页面管理设置");
                    } else if !revoked && appearance.is_some() && !on {
                        settings_hint(ui, "启用后可在外观设置中选用");
                    } else if !revoked {
                        let show_path = |ui: &mut egui::Ui| {
                            if owner_key == "status" && !on {
                                settings_hint(ui, "启用后显示功能页");
                                false
                            } else {
                                path_button(ui, &path, paused, &tip)
                            }
                        };
                        #[cfg(windows)]
                        let navigate = if has_source {
                            ui.horizontal(|ui| {
                                let label = if self.source_grant(id).is_some() { "数据源已授权 ›" } else { "授权数据源 ›" };
                                if path_button(ui, label, false, "管理这个插件可读取的数据源") {
                                    self.open_source_dialog(ui, id);
                                }
                                (has_settings || owner_key == "status") && show_path(ui)
                            }).inner
                        } else {
                            show_path(ui)
                        };
                        #[cfg(not(windows))]
                        let navigate = show_path(ui);
                        #[cfg(windows)]
                        if let Some((label, url, time)) = self.source_attribution(id) {
                            ui.horizontal_wrapped(|ui| {
                                let label = format!("数据来源 {label}");
                                if let Some(url) = url {
                                    ui.hyperlink_to(label, url);
                                } else {
                                    settings_hint(ui, &label);
                                }
                                if let Some(time) = time {
                                    settings_hint(ui, &format!("最近取数 {}", time.format("%m-%d %H:%M")));
                                }
                            });
                        }
                        if navigate {
                            if material {
                                *jump = Some(SettingsNavigation::Tab(crate::extras::SETTINGS_TAB_APPEARANCE));
                            } else if paused {
                                *jump = Some(SettingsNavigation::Tab(crate::extras::SETTINGS_TAB_PAGES));
                            } else if standalone || (on && has_settings) {
                                *jump = Some(SettingsNavigation::Plugin(id.to_owned()));
                            } else {
                                *jump = Some(SettingsNavigation::Tab(owner_settings_tab(owner_key)));
                            }
                        }
                    }
                    if let Some(e) = &runtime_error {
                        settings_hint(ui, &format!("刷新失败 · {e}"));
                    }
                },
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                #[cfg(windows)]
                ui.menu_button(egui::RichText::new("···").color(muted_color(ui)), |ui| {
                    let storage = self.records.get(id).and_then(|record| record.package.as_ref())
                        .is_some_and(|package| package.manifest.has_capability("storage"));
                    if storage && ui.button("导出数据").clicked() {
                        match crate::system::pick_folder_for("选择插件备份文件夹") {
                            Ok(Some(folder)) => self.notice = match self.export_storage(id, &folder) {
                                Ok(path) => format!("数据已导出到 {}", path.display()),
                                Err(error) => error.to_string(),
                            },
                            Ok(None) => {}
                            Err(_) => self.notice = "无法选择备份文件夹".into(),
                        }
                        ui.close_menu();
                    }
                    if storage && ui.button("导入数据").on_hover_text("导入会替换此插件的私有数据").clicked() {
                        match crate::system::pick_files_for("选择要导入的插件数据备份", false) {
                            Ok(files) => if let Some(path) = files.first() {
                                self.notice = self.import_storage(id, path).map_or_else(
                                    |error| error.to_string(), |_| "插件数据已导入".into());
                            },
                            Err(_) => self.notice = "无法选择备份文件".into(),
                        }
                        ui.close_menu();
                    }
                    if has_source && ui.button("管理数据源").clicked() {
                        self.open_source_dialog(ui, id);
                        ui.close_menu();
                    }
                    if (listed || runtime_error.is_some()) && ui.button("查看详情").clicked() {
                        market_browse_ui::select_detail(ui, id);
                        ui.close_menu();
                    }
                    if ui.button("反馈问题").clicked() {
                        request_plugin_feedback(ui, id, &name, &version);
                        ui.close_menu();
                    }
                    if ui.button("卸载…").clicked() {
                        self.pending = Some(id.to_owned());
                        ui.close_menu();
                    }
                });
                #[cfg(not(windows))]
                if ui.add(egui::Button::new("卸载…").frame(false)).clicked() {
                    self.pending = Some(id.to_owned());
                }
                if !revoked && plugin_switch(ui, id, on, if on { "停用插件" } else { "启用插件" }) {
                    if material {
                        #[cfg(all(windows, feature = "sdk-material"))]
                        if self.request_material(ui, (!selected).then_some(id)) && !selected {
                            *jump = Some(SettingsNavigation::Tab(crate::extras::SETTINGS_TAB_APPEARANCE));
                        }
                    } else {
                        self.notice = self
                            .enable(id, !on)
                            .err()
                            .map(|e| e.to_string())
                            .unwrap_or_default();
                        if self.notice.is_empty() {
                            ui.ctx().request_repaint_of(egui::ViewportId::ROOT);
                        }
                    }
                }
                #[cfg(windows)]
                if let Some((official, latest)) = &update {
                    let response = ui
                        .add_enabled(!self.market_busy(), market_button(ui, "更新"))
                        .on_hover_text(format!("更新到 v{latest}，设置和开关保持原样"));
                    if response.clicked() {
                        self.notice = self
                            .market_action(*official, id)
                            .err()
                            .map(|e| e.to_string())
                            .unwrap_or_default();
                    }
                }
            });
            },
        );
    }

    #[cfg(windows)]
    fn open_source_dialog(&self, ui: &egui::Ui, id: &str) {
        let url = match self.source_grant(id) {
            Some(SourceGrant::Loopback(url) | SourceGrant::Https { url }) => url.clone(),
            _ => self.records[id].network().and_then(|network| network.url.clone()).unwrap_or_default(),
        };
        ui.data_mut(|data| {
            data.insert_temp(egui::Id::new(SOURCE_DIALOG), SourceDialog {
                plugin: id.to_owned(),
                url,
                secret: String::new(),
                error: String::new(),
            });
        });
    }

    #[cfg(windows)]
    fn source_dialog(&mut self, ui: &mut egui::Ui) {
        use crate::extras::{settings_modal_frame, settings_modal_title};
        let key = egui::Id::new(SOURCE_DIALOG);
        let Some(mut dialog) = ui.data(|data| data.get_temp::<SourceDialog>(key)) else { return; };
        let id = &dialog.plugin;
        let Some(kind) = self.source_kind(id) else {
            ui.data_mut(|data| data.remove::<SourceDialog>(key));
            return;
        };
        let name = &self.records[id].package.as_ref().unwrap().manifest.name;
        let grant = self.source_grant(id);
        let network = self.records[id].network().cloned();
        let ctx = ui.ctx().clone();
        let mut pick = false;
        let mut approve = false;
        let mut revoke = false;
        let modal = egui::Modal::new(key.with("window"))
            .frame(settings_modal_frame(&ctx))
            .backdrop_color(Color32::from_black_alpha(110))
            .show(&ctx, |ui| {
                ui.set_width(395.0);
                settings_apply_style(ui);
                let mut close = settings_modal_title(ui, "只读数据源");
                ui.add(egui::Label::new(name).truncate());
                settings_hint(ui, "仅在插件和所属页面均开启时刷新。");
                if let Some(grant) = grant {
                    ui.label("已授权读取");
                    let target = match grant {
                        SourceGrant::File(path) => path.to_string_lossy(),
                        SourceGrant::Loopback(url) | SourceGrant::Https { url } => url.as_str().into(),
                        SourceGrant::HttpsSources { hosts, .. } => hosts.join("、").into(),
                    };
                    ui.add(egui::Label::new(target.as_ref()).truncate()).on_hover_text(target.as_ref());
                }
                match kind {
                    SourceKind::File => {
                        pick = ui.button("选择并授权读取文件…").clicked();
                        settings_hint(ui, "只读你选中的一个本地 UTF-8 文件。");
                    }
                    SourceKind::Loopback => {
                        ui.add(egui::TextEdit::singleline(&mut dialog.url)
                            .char_limit(2048).desired_width(385.0)
                            .hint_text("http://127.0.0.1:8080/status"));
                        approve = ui.add_enabled(!dialog.url.trim().is_empty(),
                            egui::Button::new("授权读取此地址")).clicked();
                        settings_hint(ui, "仅向填写的本机地址发送 GET 请求。");
                    }
                    SourceKind::Https => {
                        let network = network.as_ref().unwrap();
                        let multiple = network.sources.is_some();
                        if !multiple {
                            ui.add(egui::TextEdit::singleline(&mut dialog.url)
                                .char_limit(2048).desired_width(385.0).hint_text("https://"));
                        }
                        settings_hint(ui, &format!("允许的域名 · {}", network.hosts.join("、")));
                        if let Some(credential) = &network.credential {
                            ui.label(&credential.label);
                            ui.add(egui::TextEdit::singleline(&mut dialog.secret)
                                .password(true).char_limit(2048).desired_width(385.0));
                        }
                        approve = ui.add_enabled((multiple || !dialog.url.trim().is_empty())
                            && (network.credential.is_none() || !dialog.secret.is_empty()),
                            egui::Button::new(if multiple { "授权读取这些来源" } else { "授权读取此地址" })).clicked();
                    }
                }
                if !dialog.error.is_empty() {
                    ui.colored_label(warn_color(ui), &dialog.error);
                }
                ui.horizontal(|ui| {
                    if grant.is_some() {
                        revoke = ui.button("撤销授权").clicked();
                    }
                    close |= ui.button("完成").clicked();
                });
                close
            });
        let changed = if pick {
            match crate::system::pick_files_for("选择要授权读取的本地文件", false) {
                Ok(paths) => paths.first().map(|path| source_read::grant_file(path).map(Some)),
                Err(error) => Some(Err(error.to_string())),
            }
        } else if approve {
            Some(match &network {
                Some(network) => network.grant(&dialog.url).map(Some).map_err(|err| err.to_string()),
                None => source_read::grant_url(&dialog.url).map(Some),
            })
        } else if revoke {
            Some(Ok(None))
        } else {
            None
        };
        if let Some(changed) = changed {
            let draft = self.drafts.get(id).cloned();
            dialog.error = changed.and_then(|grant| {
                if approve && kind == SourceKind::Https {
                    self.set_source_grant_with_secret(id, grant, Some(&dialog.secret))
                } else {
                    self.set_source_grant(id, grant)
                }.map_err(|err| err.to_string())
            }).err().unwrap_or_default();
            if dialog.error.is_empty() {
                if let Some(draft) = draft {
                    self.drafts.insert(id.clone(), draft);
                }
                dialog.secret.clear();
                if revoke {
                    dialog.url = network.and_then(|network| network.url).unwrap_or_default();
                }
            }
            ctx.request_repaint_of(egui::ViewportId::ROOT);
        }
        if modal.inner || modal.should_close() {
            ui.data_mut(|data| data.remove::<SourceDialog>(key));
        } else {
            ui.data_mut(|data| data.insert_temp(key, dialog));
        }
    }

    /// 重启检查没通过的记录：插件已暂停运行，可以从集市重装或移除记录。
    fn invalid_row(&mut self, ui: &mut egui::Ui, id: &str) {
        let row_w = ui.available_width();
        ui.allocate_ui_with_layout(
            egui::vec2(row_w, 42.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
            ui.set_min_height(42.0);
            ui.spacing_mut().item_spacing.x = 12.0;
            plugin_cover(
                ui,
                egui::Id::new(("invalid", id)),
                64.0,
                egui::CornerRadius::same(6),
                "无法读取",
                false,
                || Some(Err(())),
            );
            let info_w = (ui.available_width() - 170.0).max(80.0);
            ui.allocate_ui_with_layout(
                egui::vec2(info_w, 40.0),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.set_width(info_w);
                    ui.spacing_mut().item_spacing.y = 3.0;
                    ui.add(egui::Label::new(egui::RichText::new(id).size(13.5).strong()).truncate());
                    settings_hint(ui, "安装记录没有通过检查，已暂停运行");
                },
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add(egui::Button::new("移除记录").frame(false)).clicked() {
                    self.notice = self
                        .remove_invalid(id)
                        .err()
                        .map(|e| e.to_string())
                        .unwrap_or_default();
                }
                #[cfg(windows)]
                {
                    let listed = self
                        .market_catalog(true)
                        .is_some_and(|catalog| catalog.entries.iter().any(|entry| entry.id == id));
                    if listed
                        && ui
                            .add_enabled(!self.market_busy(), market_button(ui, "重新安装"))
                            .on_hover_text("从插件集市重新安装，原来的设置不会保留")
                            .clicked()
                    {
                        self.notice = self
                            .market_action(true, id)
                            .err()
                            .map(|e| e.to_string())
                            .unwrap_or_default();
                    }
                }
            });
            },
        );
    }

    /// 卸载确认：二选一，每项写明后果。
    fn uninstall_dialog(&mut self, ui: &mut egui::Ui) {
        use crate::extras::{settings_modal_frame, settings_modal_title};
        let Some(id) = self.pending.clone() else {
            return;
        };
        let Some(name) = self
            .records
            .get(&id)
            .and_then(|record| record.package.as_ref())
            .map(|package| package.manifest.name.clone())
        else {
            self.pending = None;
            return;
        };
        let ctx = ui.ctx().clone();
        let mut choice = None;
        let mut close = false;
        let modal = egui::Modal::new(egui::Id::new("plugin-uninstall"))
            .frame(settings_modal_frame(&ctx))
            .backdrop_color(Color32::from_black_alpha(110))
            .show(&ctx, |ui| {
                ui.set_width(360.0);
                settings_apply_style(ui);
                close = settings_modal_title(ui, &format!("卸载「{name}」"));
                settings_hint(ui, "停止运行并移除插件文件。你的设置和数据怎么处理？");
                ui.add_space(4.0);
                if option_card(ui, "保留数据", "以后重新安装，设置和数据都还在", false) {
                    choice = Some(true);
                }
                if option_card(ui, "删除数据", "一并删除插件保存的设置和数据，不能恢复", true) {
                    choice = Some(false);
                }
                ui.add_space(4.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    close |= ui.button("取消").clicked();
                });
            });
        if let Some(keep) = choice {
            self.notice = self
                .uninstall(&id, keep)
                .err()
                .map(|e| e.to_string())
                .unwrap_or_default();
            if self.notice.is_empty() {
                forget_covers(ui);
                ui.ctx().request_repaint_of(egui::ViewportId::ROOT);
            }
            self.pending = None;
        } else if close || modal.should_close() {
            self.pending = None;
        }
    }

    /// 开发者工具弹窗。导入测试集市目录成功后返回真，界面切到本地测试集市。
    #[cfg(windows)]
    fn developer_dialog(&mut self, ui: &mut egui::Ui) -> bool {
        use crate::extras::{settings_modal_frame, settings_modal_title};
        let key = egui::Id::new("plugin-developer-tools");
        if !ui.data(|data| data.get_temp::<bool>(key)).unwrap_or(false) {
            return false;
        }
        let author_key = key.with("author");
        let mut author = ui.data_mut(|data| {
            data.remove_temp::<plugin_author_ui::AuthorTools>(author_key)
                .unwrap_or_default()
        });
        let ctx = ui.ctx().clone();
        let mut close = false;
        let mut local = false;
        let mut market = false;
        let mut author_action = None;
        let mut watching = self.watch_files_enabled();
        let logs = self.devtools.logs.snapshot();
        let modal = egui::Modal::new(key.with("modal"))
            .frame(settings_modal_frame(&ctx))
            .backdrop_color(Color32::from_black_alpha(110))
            .show(&ctx, |ui| {
                ui.set_width(360.0);
                settings_apply_style(ui);
                if author.creating() {
                    close = settings_modal_title(ui, "从模板创建");
                    author_action = author.form(ui);
                    return;
                }
                close = settings_modal_title(ui, "开发者工具");
                settings_hint(ui, "给插件作者自测用。本地安装的插件不经过集市审核。");
                ui.add_space(4.0);
                author_action = author.menu(ui, &mut watching, &logs);
                ui.add_space(4.0);
                local = option_card(
                    ui,
                    "安装本地插件…",
                    "选择开发目录里的 plugin.json",
                    false,
                );
                market = option_card(
                    ui,
                    "导入测试集市目录…",
                    "选择 catalog.json，导入后集市右上角可以切到「本地测试」",
                    false,
                );
                ui.add_space(4.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    close |= ui.button("关闭").clicked();
                });
            });
        let mut opened = false;
        if watching != self.watch_files_enabled() {
            self.set_watch_files(watching);
            ctx.request_repaint_of(egui::ViewportId::ROOT);
        }
        if let Some(action) = author_action {
            author.run(action);
            if let Some(dir) = author.created_dir() {
                let dir = dir.to_path_buf();
                ui.data_mut(|data| data.insert_temp(plugin_guide_ui::template_dir_id(), dir));
            }
            ctx.request_repaint();
        }
        if local {
            match crate::system::pick_files_for("选择本地插件的 plugin.json", false) {
                Ok(paths) => {
                    if let Some(path) = paths.first() {
                        self.notice = match self.install(path) {
                            Ok(()) => {
                                forget_covers(ui);
                                "已安装。打开开关后开始运行。".into()
                            }
                            Err(e) => e.to_string(),
                        };
                    }
                }
                Err(e) => self.notice = e.to_string(),
            }
        }
        if market {
            match crate::system::pick_files_for("选择测试集市的 catalog.json", false) {
                Ok(paths) => {
                    if let Some(path) = paths.first() {
                        match self.set_market_catalog(path) {
                            Ok(()) => {
                                forget_covers(ui);
                                self.notice.clear();
                                opened = true;
                            }
                            Err(e) => self.notice = e.to_string(),
                        }
                    }
                }
                Err(e) => self.notice = e.to_string(),
            }
        }
        if local || market || close || modal.should_close() {
            ui.data_mut(|data| data.remove::<bool>(key));
        } else {
            ui.data_mut(|data| data.insert_temp(author_key, author));
        }
        opened
    }

    #[cfg(all(windows, feature = "sdk-material"))]
    fn request_material(&mut self, ui: &egui::Ui, id: Option<&str>) -> bool {
        if let Some(id) = id.filter(|id| !self.material_permission_granted(id)) {
            ui.data_mut(|data| {
                data.insert_temp(egui::Id::new("settings-material-permission"), id.to_owned())
            });
            return false;
        }
        material_changed(ui, self.activate_material(id, false))
    }

    #[cfg(all(windows, feature = "sdk-material"))]
    fn confirm_material_permission(&mut self, ui: &egui::Ui) -> bool {
        use crate::extras::{
            settings_apply_style, settings_hint, settings_modal_frame, settings_modal_title,
        };
        let key = egui::Id::new("settings-material-permission");
        let Some(id) = ui.data(|data| data.get_temp::<String>(key)) else {
            return false;
        };
        let Some(package) = self.records.get(&id).and_then(Record::material_package) else {
            ui.data_mut(|data| data.remove::<String>(key));
            return false;
        };
        let ctx = ui.ctx();
        let mut close = false;
        let mut allow = false;
        let modal = egui::Modal::new(key.with("modal"))
            .frame(settings_modal_frame(ctx))
            .backdrop_color(egui::Color32::from_black_alpha(110))
            .show(ctx, |ui| {
                ui.set_width(360.0);
                settings_apply_style(ui);
                close = settings_modal_title(ui, "允许材质读取屏幕画面？");
                ui.add(
                    egui::Label::new(egui::RichText::new(&package.manifest.name).strong()).wrap(),
                );
                settings_hint(ui, CAPTURE_PERMISSION);
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    close |= ui.button("取消").clicked();
                    allow = ui.button("允许并启用").clicked();
                });
            });
        if allow || close || modal.should_close() {
            ui.data_mut(|data| data.remove::<String>(key));
        }
        allow && material_changed(ui, self.activate_material(Some(&id), true))
    }

    #[cfg(all(windows, feature = "sdk-material"))]
    pub(crate) fn material_settings(
        &mut self,
        ui: &mut egui::Ui,
        active: Option<&str>,
        compatibility_enabled: bool,
    ) -> bool {
        if !self.records.values().any(|record| record.material_package().is_some()) {
            self.confirm_material_permission(ui);
            return false;
        }
        ui.separator();
        let selected = self.selected_material_id().map(str::to_owned);
        let mut choice = selected.clone();
        let title = selected.as_deref().map_or_else(
            || egui::RichText::new("无"),
            |id| {
                material_name(
                    ui,
                    &self.records[id].package.as_ref().unwrap().manifest.name,
                    true,
                    active == Some(id),
                )
            },
        );
        settings_control_row(ui, "材质", 68.0, |ui| {
            let combo = egui::ComboBox::from_id_salt("appearance-material")
                .selected_text(title)
                .width(ui.available_width().min(220.0))
                .truncate()
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut choice, None, "无");
                    for (id, record) in &self.records {
                        if let Some(package) = record.material_package() {
                            ui.selectable_value(
                                &mut choice,
                                Some(id.clone()),
                                &package.manifest.name,
                            );
                        }
                    }
                })
                .response;
            if selected.is_some() {
                plugin_tag(ui);
            }
            combo
        });
        if choice != selected {
            self.request_material(ui, choice.as_deref());
        }
        self.confirm_material_permission(ui);
        material_error(ui);

        let Some(id) = self.selected_material_id().map(str::to_owned) else {
            return false;
        };
        let mut restart = false;
        if !compatibility_enabled && active != Some(id.as_str()) {
            crate::extras::settings_hint(
                ui,
                "材质需要兼容渲染（Direct3D 12），开启后重启岛仔生效。",
            );
            restart = ui.button("开启并重启").clicked();
        }
        let record = &self.records[&id];
        let fields = &record.material_package().unwrap().manifest.settings;
        let mut changes = Vec::new();
        for field in fields {
            let (Some(min), Some(max), Some(step)) = (field.min, field.max, field.step) else {
                continue;
            };
            let precision = field.precision.map_or_else(
                || {
                    step.to_string()
                        .split_once('.')
                        .map_or(0, |(_, fraction)| fraction.len())
                },
                usize::from,
            );
            let mut value = record
                .values
                .get(&field.key)
                .unwrap_or(&field.default)
                .parse::<f32>()
                .unwrap_or(min);
            let original = f64::from(value);
            let changed = ui
                .push_id((&id, &field.key), |ui| {
                    ui.horizontal(|ui| {
                        let label_width = (ui.available_width() - 180.0).max(56.0);
                        ui.add_sized(
                            [label_width, 24.0],
                            egui::Label::new(&field.label).truncate(),
                        )
                        .on_hover_text(&field.label);
                        ui.spacing_mut().slider_width =
                            (ui.available_width() - 65.0).clamp(54.0, 135.0);
                        ui.add(
                            egui::Slider::new(&mut value, min..=max)
                                .clamping(egui::SliderClamping::Edits)
                                .step_by(f64::from(step))
                                .suffix(field.unit.as_deref().unwrap_or_default())
                                .custom_formatter(move |number, _| format!("{number:.precision$}"))
                                .custom_parser(move |input| {
                                    if input == format!("{original:.precision$}") {
                                        Some(original)
                                    } else {
                                        input
                                            .chars()
                                            .filter(|c| !c.is_whitespace())
                                            .map(|c| if c == '−' { '-' } else { c })
                                            .collect::<String>()
                                            .parse()
                                            .ok()
                                    }
                                }),
                        )
                        .changed()
                    })
                    .inner
                })
                .inner;
            if changed {
                changes.push((field.key.clone(), value.clamp(min, max)));
            }
        }
        for (key, value) in changes {
            material_changed(ui, self.set_material_parameter(&id, &key, value));
        }
        restart
    }

    pub(crate) fn standalone_settings_pages(&self) -> impl Iterator<Item = (&str, &str)> {
        self.records.iter().filter_map(|(id, record)| {
            let manifest = &record.package.as_ref()?.manifest;
            standalone_settings_page(manifest)
                .then_some((id.as_str(), manifest.name.as_str()))
        })
    }

    pub(crate) fn settings_destination(&mut self, id: &str) -> Option<usize> {
        let record = self.records.get(id)?;
        let manifest = &record.package.as_ref()?.manifest;
        self.focus = (record.enabled
            && (manifest.has_settings() || manifest.has_surface(Surface::IslandPinned)))
            .then(|| id.to_owned());
        (record.enabled || manifest.owner != "status").then(|| owner_settings_tab(&manifest.owner))
    }

    pub(crate) fn standalone_settings(&mut self, ui: &mut egui::Ui, id: &str) {
        let Some(record) = self.records.get(id) else {
            return;
        };
        let Some(manifest) = record.package.as_ref().map(|package| &package.manifest) else {
            return;
        };
        let has_settings=manifest.has_settings();
        if standalone_settings_page(manifest) {
            let on = record.enabled && !record.status_page_disabled;
            let switch_id = format!("{id}:status-page");
            let title = format!("启用{}", manifest.name);
            let mut switched = false;
            settings_card(ui, |ui| {
                switched = plugin_setting_switch_row(
                    ui,
                    &switch_id,
                    &title,
                    "关闭后隐藏岛上功能页并暂停后台",
                    on,
                    false,
                );
            });
            if switched {
                let draft = self.drafts.remove(id);
                if let Err(error) = self.set_status_page_enabled(id, !on) {
                    self.notice = error.to_string();
                }
                if let Some(draft) = draft {
                    self.drafts.insert(id.to_owned(), draft);
                }
                ui.ctx().request_repaint_of(egui::ViewportId::ROOT);
            }
        } else if record.status_page_disabled {
            settings_hint(ui, "岛上功能页已关闭，可在「页面管理」开启");
        }
        if !has_settings {settings_hint(ui,"此插件没有额外设置，可在页面管理中调整常驻与入口。");}
        ui.push_id(id, |ui| self.settings_form(ui, id));
        if !self.notice.is_empty() {
            settings_hint(ui, &self.notice);
        }
        #[cfg(windows)]
        self.source_dialog(ui);
    }

    pub(crate) fn status_page_settings(&mut self,ui:&mut egui::Ui,id:&str)->Option<SettingsNavigation> {
        let mut pinned=self.pinned_enabled(id);
        let route=self.status_page_controls(ui,id,&mut pinned);
        if self.has_pinned_surface(id)&&pinned!=self.pinned_enabled(id){if let Err(error)=self.set_pinned(id,pinned){self.notice=error.to_string();}}
        route
    }

    pub(crate) fn status_page_controls(&mut self,ui:&mut egui::Ui,id:&str,pinned:&mut bool)->Option<SettingsNavigation> {
        let (_,name,enabled,page_on)=self.status_page_preferences().find(|(plugin,_,_,_)|*plugin==id)?;
        let name=name.to_owned();let on=enabled&&page_on;let hidden=self.status_page_settings_hidden(id);
        let mut switched=false;let mut eye=false;let mut settings=false;
        ui.horizontal(|ui|{
            let text_width=(ui.available_width()-58.-SETTINGS_SWITCH_W-56.-3.*ui.spacing().item_spacing.x).max(0.);
            ui.vertical(|ui|{ui.set_width(text_width);ui.horizontal(|ui|{
                ui.add(egui::Label::new(egui::RichText::new(&name).size(13.)).truncate());plugin_tag(ui);
            });ui.add(egui::Label::new(egui::RichText::new("独立插件功能页").size(11.5).color(ui.visuals().weak_text_color())).truncate());});
            if crate::app::component_controls::icon_control(ui,true,*pinned,on).clicked(){*pinned=!*pinned;}
            switched=plugin_switch_sized(ui,id,on,&name,egui::vec2(SETTINGS_SWITCH_W,20.));
            eye=page_visibility_action(ui,on,hidden,true);
            settings=ui.add_sized([58.,26.],egui::Button::new(egui::RichText::new("设置 ›").size(12.)).frame(false)).clicked();
        });
        let result=if switched {self.set_status_page_enabled(id,!on)}else if eye {self.set_status_page_settings_hidden(id,!hidden)}else{Ok(())};
        if let Err(error)=result {settings_hint(ui,&error.to_string());}
        if switched||eye {ui.ctx().request_repaint_of(egui::ViewportId::ROOT);}
        settings.then(||SettingsNavigation::Plugin(id.to_owned()))
    }

    /// 插件设置显示在所属设置页，标题旁带「插件」小标。从「已安装」点路径过来时，
    /// 滚到这张卡并用描边闪一下。
    pub fn settings(&mut self, ui: &mut egui::Ui, owner: &str, module_on: bool) {
        if !module_on {
            return;
        }
        let ids: Vec<_> = self
            .records
            .iter()
            .filter(|(_, r)| {
                r.enabled
                    && r.package.as_ref().is_some_and(|p| {
                        p.manifest.owner == owner
                            && (p.manifest.has_settings() || p.manifest.has_surface(Surface::IslandPinned))
                    })
            })
            .map(|(id, _)| id.clone())
            .collect();
        if ids.is_empty() {
            return;
        }
        let now = ui.input(|input| input.time);
        let flash_id = egui::Id::new("plugin-settings-flash");
        for id in ids {
            ui.push_id(&id, |ui| {
                let card = ui.scope(|ui| {
                    ui.add_space(6.0);
                    let header = ui.horizontal(|ui| {
                        ui.strong(&self.records[&id].package.as_ref().unwrap().manifest.name);
                        plugin_tag(ui);
                    }).response;
                    if self.focus.as_deref() == Some(&id) {
                        header.scroll_to_me(Some(egui::Align::Min));
                        self.focus = None;
                        ui.data_mut(|data| data.insert_temp(flash_id, (id.clone(), now)));
                    }
                    self.settings_form(ui, &id);
                });
                let flash = ui
                    .data(|data| data.get_temp::<(String, f64)>(flash_id))
                    .filter(|(flashed, _)| *flashed == id);
                if let Some((_, start)) = flash {
                    let age = (now - start) as f32;
                    let strength = if age < 0.7 { 1.0 } else { (1.0 - (age - 0.7) / 1.1).max(0.0) };
                    if strength > 0.0 {
                        let accent = plugin_accent(ui);
                        ui.painter().rect_stroke(
                            card.response.rect,
                            10.0,
                            egui::Stroke::new(2.0, accent.gamma_multiply(strength)),
                            egui::StrokeKind::Outside,
                        );
                        ui.ctx().request_repaint();
                    } else {
                        ui.data_mut(|data| data.remove::<(String, f64)>(flash_id));
                    }
                }
            });
        }
        if !self.notice.is_empty() {
            settings_hint(ui, &self.notice);
        }
        #[cfg(windows)]
        self.source_dialog(ui);
    }

    fn settings_form(&mut self, ui: &mut egui::Ui, id: &str) {
        let record = &self.records[id];
        let manifest = &record.package.as_ref().unwrap().manifest;
        let has_settings = manifest.has_settings();
        let supports_pinned = self.pinned_name(id).is_some();
        let standalone = standalone_settings_page(manifest);
        let grouped = id == "daozai.days" || standalone;
        let mut sections = std::collections::BTreeMap::<_, Vec<_>>::new();
        for field in &manifest.settings {
            sections.entry(settings_section(id, &field.key)).or_default().push(field);
        }
        let drafts = self.drafts.entry(id.to_owned()).or_insert_with(|| record.values.clone());
        let mut submit = false;
        #[cfg(windows)]
        let mut authorize = false;
        for ((_, heading), fields) in sections {
            if grouped {
                settings_label(ui, heading);
            }
            settings_card(ui, |ui| {
                for (index, field) in fields.into_iter().enumerate() {
                    if grouped && index > 0 {
                        ui.separator();
                    }
                    let value = drafts.entry(field.key.clone()).or_insert_with(|| field.default.clone());
                    submit |= script_settings_ui::field_row(ui, id, field, value);
                }
                match (id, heading) {
                    ("daozai.gold", "价格提醒") => settings_hint(ui, "提醒价填 0 时关闭该提醒"),
                    ("daozai.live-pay", "薪资与班次") => settings_hint(ui, "月薪填 0 表示未设置"),
                    _ => {}
                }
            });
        }
        #[cfg(windows)]
        if record.source_kind().is_some() {
            settings_label(ui, "数据来源");
            settings_card(ui, |ui| {
                let label = manifest.attribution.as_ref().map_or("只读数据源", |source| source.label.as_str());
                ui.horizontal(|ui| {
                    if let Some(url) = manifest.attribution.as_ref().and_then(|source| source.url.as_deref()) {
                        ui.hyperlink_to(label, url);
                    } else {
                        ui.label(label);
                    }
                    if record.source_grant().is_some() {
                        settings_hint(ui, "已授权");
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        authorize = ui.button(if record.source_grant().is_some() {
                            "管理授权"
                        } else {
                            "授权数据源"
                        }).clicked();
                    });
                });
                if let Some(min) = record.network().and_then(|network| network.min_refresh_seconds) {
                    ui.separator();
                    let draft = drafts.entry(source_https::REFRESH_FIELD.into())
                        .or_insert_with(|| record.refresh_interval().as_secs().to_string());
                    let mut seconds = draft.parse::<u32>().unwrap_or(min);
                    settings_control_row(ui, "刷新间隔", 80., |ui| {
                        ui.add_sized([105.0, 27.0], egui::DragValue::new(&mut seconds)
                            .range(min..=source_https::MAX_REFRESH_SECONDS).suffix(" 秒"))
                    });
                    *draft = seconds.to_string();
                }
            });
        }
        #[cfg(windows)]
        if authorize {
            self.open_source_dialog(ui, id);
        }
        if supports_pinned {
            settings_label(ui, "岛上常驻");
            settings_card(ui, |ui| self.pinned_toggle(ui, id, !standalone));
        }
        if has_settings {
            ui.add_space(4.0);
            let notice_id = ui.make_persistent_id("plugin-save-result");
            let save = ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let save = ui.add(egui::Button::new(egui::RichText::new("保存设置").color(Color32::WHITE))
                        .fill(plugin_accent(ui)).min_size(egui::vec2(86.0, 30.0)));
                    if let Some(notice) = ui.data(|data| data.get_temp::<String>(notice_id)) {
                        settings_hint(ui, &notice);
                    }
                    save.clicked()
                }).inner
            }).inner;
            if save || submit {
                let values = self.drafts[id].clone();
                let notice = match self.save(id, values) {
                    Ok(()) => {
                        ui.ctx().request_repaint_of(egui::ViewportId::ROOT);
                        "设置已保存".to_owned()
                    }
                    Err(error) => error.to_string(),
                };
                ui.data_mut(|data| data.insert_temp(notice_id, notice));
            }
        }
    }

    fn pinned_toggle(&mut self, ui: &mut egui::Ui, id: &str, tagged: bool) {
        let enabled = self.pinned.order.iter().any(|item| item == id);
        if plugin_setting_switch_row(ui, &format!("{id}:pinned"), "在岛上常驻显示", "岛收起时显示", enabled, tagged) {
            if let Err(error) = self.set_pinned(id, !enabled) { self.notice = error.to_string(); }
            ui.ctx().request_repaint_of(egui::ViewportId::ROOT);
        }
    }

    pub(crate) fn pinned_order_settings(&mut self, ui: &mut egui::Ui) {
        if self.pinned.order.is_empty() { return; }
        ui.add_space(8.0);
        ui.horizontal(|ui| { ui.strong("岛上常驻插件"); plugin_tag(ui); });
        let order = self.pinned.order.clone();
        for (index, id) in order.iter().enumerate() {
            ui.push_id(id, |ui| ui.horizontal(|ui| {
                ui.label(self.pinned_name(id).unwrap_or(id));
                let earlier = ui.add_enabled(index > 0, egui::Button::new("上移")).clicked();
                let later = ui.add_enabled(index + 1 < order.len(), egui::Button::new("下移")).clicked();
                if earlier || later {
                    let target = if earlier { index - 1 } else { index + 1 };
                    if let Err(error) = self.move_pinned(index, target) { self.notice = error.to_string(); }
                    ui.ctx().request_repaint_of(egui::ViewportId::ROOT);
                }
            }));
        }
    }
}
