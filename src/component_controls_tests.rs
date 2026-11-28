use super::resident_navigation::{fixture, frame};
use super::*;
use crate::app::{
    resident_navigation::{registered, Destination},
    *,
};
use crate::{model::Dock, performance::BarStyle};

#[test]
fn component_backend_eye_and_pin_are_independent_and_persist() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut h = H::new();
    h.app.settings.visible_pages.fill(true);
    h.app.settings.hidden_settings_pages.fill(false);
    for page in 0..PAGE_NAMES.len() {
        h.app.set_resident_preference(page, true);
        h.app.set_page_settings_hidden(page, true);
        assert!(h.app.module_on(page) && h.app.resident_on(page));
        h.app.settings.save();
        assert!(crate::model::Settings::load().hidden_settings_pages[page]);
        h.app.set_page_settings_hidden(page, false);
        assert!(h.app.module_on(page) && !h.app.settings.hidden_settings_pages[page]);
        h.app.set_page_visible(page, false);
        assert!(h.app.settings.hidden_settings_pages[page] && !h.app.resident_on(page));
        assert!(h.app.resident_preference(page), "关闭后台保留常驻偏好");
        h.app.set_page_settings_hidden(page, false);
        assert!(
            h.app.settings.hidden_settings_pages[page],
            "关闭时眼睛不能恢复入口"
        );
        h.app.set_page_visible(page, true);
        assert!(!h.app.settings.hidden_settings_pages[page] && h.app.resident_on(page));
        h.app.set_resident_preference(page, false);
        assert!(h.app.module_on(page) && !h.app.settings.hidden_settings_pages[page]);
        h.app.settings.save();
        h.app.settings = crate::model::Settings::load();
        assert!(!h.app.resident_preference(page));
    }
}

#[test]
fn component_management_eye_clicks_never_change_backend_or_pin() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut h = H::new();
    h.app.settings.visible_pages.fill(true);
    h.app.settings.hidden_settings_pages.fill(false);
    h.app.settings_tab = crate::extras::SETTINGS_TAB_PAGES;
    h.app.open_settings();
    h.ctx.set_embed_viewports(false);
    for _ in 0..3 {
        draw_page_management(&mut h, vec![]);
    }
    let sidebar = |h: &H, name: &str| page_management_text_rects(h, name).any(|r| r.right() < 180.);
    for (page, name) in PAGE_NAMES.iter().enumerate() {
        let switch = page_management_switch(&h, name, true).0;
        let eye = Pos2::new(switch.right() + 22., switch.center().y);
        let pin = h.app.resident_preference(page);
        click_page_management(&mut h, eye);
        draw_page_management(&mut h, vec![]);
        assert!(
            h.app.module_on(page) && h.app.settings.hidden_settings_pages[page],
            "{name}: eye hides only entry"
        );
        assert!(
            !sidebar(&h, name),
            "{name}: independent sidebar entry should hide"
        );
        assert_eq!(h.app.resident_preference(page), pin);
        click_page_management(&mut h, eye);
        draw_page_management(&mut h, vec![]);
        assert!(!h.app.settings.hidden_settings_pages[page] && sidebar(&h, name));
        click_page_management(&mut h, switch.center());
        draw_page_management(&mut h, vec![]);
        assert!(!h.app.module_on(page) && !sidebar(&h, name));
        click_page_management(&mut h, eye);
        draw_page_management(&mut h, vec![]);
        assert!(h.app.settings.hidden_settings_pages[page] && !h.app.module_on(page));
        click_page_management(&mut h, switch.center());
        draw_page_management(&mut h, vec![]);
        assert!(
            h.app.module_on(page) && sidebar(&h, name),
            "{name}: reopening restores entry"
        );
    }
}

#[test]
fn component_every_idle_page_has_a_matching_hover_target_in_every_dock() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    for dock in [Dock::Top, Dock::Bottom, Dock::Left, Dock::Right] {
        let mut h = H::new();
        h.app.settings.dock = dock;
        h.app.settings.pet = false;
        h.app.settings.hover_page_delay_ms = 200;
        h.app.settings.performance_bar = BarStyle::Gauges;
        h.app.settings.visible_pages.fill(true);
        h.app.settings.hidden_settings_pages.fill(false);
        h.media = Default::default();
        h.power = (None, false);
        h.bt.clear();
        h.app.weather.snapshot = None;
        for page in 0..PAGE_NAMES.len() {
            h.app.set_resident_preference(page, true);
        }
        for _ in 0..90 {
            frame(&mut h, dock, None, vec![]);
        }
        let destinations = registered(&h.ctx);
        for page in 0..PAGE_NAMES.len() {
            let found = destinations.iter().any(|(_, d)| match d {
                Destination::Page(p) => *p == page,
                Destination::Performance(_) => page == PERFORMANCE_PAGE,
                Destination::Wireless(_) => page == BLUETOOTH_PAGE,
                Destination::Quota(_) => page == QUOTA_PAGE,
                _ => false,
            });
            assert!(
                found,
                "{dock:?}/{}: idle resident icon missing, targets={destinations:?}",
                PAGE_NAMES[page]
            );
        }
        for page in [
            MUSIC_PAGE,
            FOCUS_PAGE,
            WATER_PAGE,
            SHELF_PAGE,
            TODO_PAGE,
            MAIL_PAGE,
            WEATHER_PAGE,
            BLUETOOTH_PAGE,
            BATTERY_PAGE,
        ] {
            let point = registered(&h.ctx)
                .into_iter()
                .find(|(_, d)| *d == Destination::Page(page))
                .unwrap()
                .0
                .center();
            for _ in 0..32 {
                frame(
                    &mut h,
                    dock,
                    Some(point),
                    vec![egui::Event::PointerMoved(point)],
                );
            }
            assert!(
                h.app.open && h.app.page() == page,
                "{dock:?}: hovered {} but opened {}",
                PAGE_NAMES[page],
                h.app.page()
            );
            for _ in 0..90 {
                frame(&mut h, dock, None, vec![egui::Event::PointerGone]);
            }
        }
    }
}

#[cfg(feature = "sdk-js")]
#[test]
fn component_plugin_without_pinned_surface_gets_generic_residency_and_stable_order() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    for dock in [Dock::Top, Dock::Bottom, Dock::Left, Dock::Right] {
        let mut h = fixture(dock, BarStyle::Gauges);
        let id = "example.world-clock";
        h.app
            .plugins
            .install(std::path::Path::new(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/spike/examples/world-clock/plugin.json"
            )))
            .unwrap();
        h.app.plugins.enable(id, true).unwrap();
        assert!(!h.app.plugins.has_pinned_surface(id));
        h.app.set_plugin_resident(id, true);
        h.app.settings.island_order = vec![
            format!("plugin:{id}"),
            crate::app::component_controls::page_key(MUSIC_PAGE),
        ];
        h.app.settings.save();
        assert_eq!(crate::model::Settings::load().resident_plugins, [id]);
        for _ in 0..100 {
            frame(&mut h, dock, None, vec![]);
        }
        let point = registered(&h.ctx)
            .into_iter()
            .find(|(_, d)| *d == Destination::Plugin(id.into()))
            .expect("new plugin gets a resident target without island.pinned")
            .0
            .center();
        for _ in 0..32 {
            frame(
                &mut h,
                dock,
                Some(point),
                vec![egui::Event::PointerMoved(point)],
            );
        }
        assert_eq!(h.app.navigation_page_name(h.app.page()), "世界时钟");
        assert!(h.app.open);
        h.app
            .plugins
            .set_status_page_settings_hidden(id, true)
            .unwrap();
        assert!(h.app.plugin_resident(id) && h.app.plugins.status_pages().any(|(p, _)| p == id));
        h.app.plugins.set_status_page_enabled(id, false).unwrap();
        assert!(h.app.plugin_resident(id) && h.app.plugins.status_pages().all(|(p, _)| p != id));
        h.app.open_plugin_settings(id);
        assert_eq!(
            h.app.settings_plugin.as_deref(),
            Some(id),
            "paused plugin settings still accessible from management"
        );
        h.app.plugins.set_status_page_enabled(id, true).unwrap();
        assert!(!h.app.plugins.status_page_settings_hidden(id));
        assert_eq!(
            h.app.component_keys().first(),
            Some(&format!("plugin:{id}"))
        );
    }
}

#[test]
fn component_management_renders_light_dark_and_narrow_widths() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    for (name, theme, width) in [
        ("dark", crate::settings_theme::SettingsTheme::Dark, 900.),
        ("light", crate::settings_theme::SettingsTheme::Light, 900.),
        ("narrow", crate::settings_theme::SettingsTheme::Dark, 690.),
    ] {
        let mut h = H::new();
        h.app.settings_tab = crate::extras::SETTINGS_TAB_PAGES;
        h.app.settings.settings_theme = theme;
        h.app.settings.visible_pages.fill(true);
        h.app.settings.hidden_settings_pages.fill(false);
        h.app.open_settings();
        h.ctx.set_embed_viewports(false);
        #[cfg(feature = "sdk-js")]
        {
            h.app
                .plugins
                .install(std::path::Path::new(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/spike/examples/world-clock/plugin.json"
                )))
                .unwrap();
            h.app.plugins.enable("example.world-clock", true).unwrap();
        }
        h.app.set_page_visible(FOCUS_PAGE, false);
        h.app.set_page_settings_hidden(MUSIC_PAGE, true);
        let height = 1150.;
        let mut raster = crate::preview_raster::Raster::new(width as usize, height as usize);
        for _ in 0..4 {
            let output = std::sync::Arc::new(Mutex::new(None));
            let capture = output.clone();
            h.t += 0.02;
            let time = h.t;
            egui::Context::set_immediate_viewport_renderer(move |ctx, mut viewport| {
                let id = viewport.ids.this;
                *capture.lock().unwrap() = Some(ctx.run(
                    egui::RawInput {
                        viewport_id: id,
                        screen_rect: Some(Rect::from_min_size(
                            Pos2::ZERO,
                            egui::vec2(width, height),
                        )),
                        time: Some(time),
                        viewports: std::iter::once((id, egui::ViewportInfo::default())).collect(),
                        ..Default::default()
                    },
                    |ctx| (viewport.viewport_ui_cb)(ctx),
                ));
            });
            let _ = h
                .ctx
                .run(Default::default(), |ctx| h.app.settings_ui(ctx, false));
            let out = output.lock().unwrap().take().unwrap();
            raster.update_textures(&out.textures_delta);
            h.shapes = out.shapes;
        }
        for page in 0..PAGE_NAMES.len() {
            let rect = page_management_switch(&h, PAGE_NAMES[page], true).0;
            assert!(
                rect.right() + 100. < width,
                "{} controls overflow {width}",
                PAGE_NAMES[page]
            );
        }
        assert!(!h.shapes.iter().any(|s|matches!(&s.shape,egui::Shape::Text(t) if ["关闭并隐藏","隐藏入口","显示入口"].contains(&t.galley.text()))));
        if let Some(path) = std::env::var_os("DAOZAI_COMPONENT_PREVIEW") {
            raster.draw(&h.ctx.tessellate(h.shapes, 1.), 1.);
            let mut rgba = Vec::new();
            raster.to_rgba8(&mut rgba);
            image::save_buffer(
                std::path::Path::new(&path).join(format!("components-{name}.png")),
                &rgba,
                width as u32,
                height as u32,
                image::ColorType::Rgba8,
            )
            .unwrap();
        }
    }
}

#[test]
fn component_settings_links_open_the_corresponding_independent_settings() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut h = H::new();
    h.app.open_settings();
    h.ctx.set_embed_viewports(false);
    for (page, own, other) in [
        (FOCUS_PAGE, "番茄钟与休息计时", "喝水与久坐提醒"),
        (WATER_PAGE, "喝水与久坐提醒", "番茄钟与休息计时"),
        (TODO_PAGE, "任务清单与到期提醒", "临时收纳文件与剪贴板"),
        (SHELF_PAGE, "临时收纳文件与剪贴板", "任务清单与到期提醒"),
    ] {
        h.app.settings_tab = crate::extras::SETTINGS_TAB_PAGES;
        for _ in 0..2 {
            draw_page_management(&mut h, vec![]);
        }
        let title = page_management_row_title(&h, PAGE_NAMES[page], true);
        let link = page_management_text_rects(&h, "设置 ›")
            .find(|r| r.left() > title.right() && (r.center().y - title.center().y).abs() < 24.)
            .unwrap()
            .center();
        click_page_management(&mut h, link);
        draw_page_management(&mut h, vec![]);
        assert_eq!(h.app.settings_tab, PAGE_SETTINGS_TABS[page]);
        assert!(page_management_text_rects(&h, own).any(|r| r.left() > 180.));
        assert!(!page_management_text_rects(&h, other).any(|r| r.left() > 180.));
    }
}
