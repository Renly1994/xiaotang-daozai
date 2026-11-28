use super::*;
use crate::{
    app::{
        resident_navigation::{registered, Destination},
        *,
    },
    model::Dock,
    performance::{BarStyle, View},
};

fn frame(h: &mut H, dock: Dock, cursor: Option<Pos2>, events: Vec<egui::Event>) {
    h.t += 1.0 / 60.0;
    let (w, height) = if dock.is_side() {
        (crate::side_dock::SIDE_W, crate::side_dock::SIDE_H)
    } else {
        (WIN_W, WIN_H)
    };
    let win = Rect::from_min_size(Pos2::ZERO, Vec2::new(w, height));
    let env = Env {
        t: h.t,
        clock: "09:41:00".into(),
        today: "2026-11-28".into(),
        date: "11月28日 周六".into(),
        cursor,
        media: h.media.clone(),
        battery: h.power.0,
        charging: h.power.1,
        bt: h.bt.clone(),
        bt_ready: true,
        ..Default::default()
    };
    h.shapes = h
        .ctx
        .run(
            egui::RawInput {
                screen_rect: Some(win),
                time: Some(h.t),
                events,
                focused: true,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default()
                    .frame(egui::Frame::new())
                    .show(ctx, |ui| {
                        h.app.frame(ui, win, &env);
                    });
            },
        )
        .shapes;
}

fn fixture(dock: Dock, style: BarStyle) -> H {
    let mut h = H::new();
    h.app.settings.dock = dock;
    h.app.settings.pet = false;
    h.app.settings.hover_page_delay_ms = 200;
    h.app.settings.visible_pages[PERFORMANCE_PAGE] = true;
    h.app.settings.performance_bar = style;
    h.app.settings.performance_metrics = 0x1ff;
    h.app.weather.snapshot = Some(crate::weather::Snapshot {
        place: "杭州".into(),
        temp: 19.,
        ..Default::default()
    });
    h.media = crate::smtc::NowPlaying {
        title: "Test track".into(),
        artist: "Artist".into(),
        playing: true,
        duration: 180.,
        ..Default::default()
    };
    h.power = (Some(76), true);
    h.bt = vec![crate::bt::Device {
        id: "keyboard".into(),
        name: "Keyboard".into(),
        battery: Some(80),
        kind: crate::bt::DeviceKind::Keyboard,
        ..Default::default()
    }];
    for _ in 0..90 {
        frame(&mut h, dock, None, vec![]);
    }
    h
}

fn assert_destination(h: &H, destination: &Destination) {
    assert!(h.app.open, "{destination:?} should open");
    match destination {
        Destination::Page(page) => assert_eq!(h.app.page(), *page),
        Destination::Performance(view) => {
            assert_eq!(h.app.page(), PERFORMANCE_PAGE);
            assert_eq!(h.app.performance.view, Some(*view));
        }
        Destination::Wireless(id) => {
            assert_eq!(h.app.page(), BLUETOOTH_PAGE);
            assert_eq!(h.app.wireless_focus.as_ref(), Some(id));
        }
        Destination::Quota(id) => {
            assert_eq!(h.app.page(), QUOTA_PAGE);
            assert_eq!(&h.app.quota.config.selected, id);
        }
        #[cfg(feature = "sdk-js")]
        Destination::Plugin(_) => assert!(h.app.is_plugin_status_page()),
    }
}

#[test]
fn hover_module_painted_targets_match_clicks_in_all_docks_and_performance_styles() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    for dock in [Dock::Top, Dock::Bottom, Dock::Left, Dock::Right] {
        for style in BarStyle::ALL {
            let h = fixture(dock, style);
            let targets = registered(&h.ctx);
            assert!(
                targets
                    .iter()
                    .any(|(_, d)| *d == Destination::Page(BATTERY_PAGE)),
                "{dock:?}/{style:?}: battery link missing"
            );
            assert!(
                targets
                    .iter()
                    .any(|(_, d)| matches!(d, Destination::Performance(_))),
                "{dock:?}/{style:?}: performance link missing"
            );
            for (_, destination) in targets {
                let mut h = fixture(dock, style);
                let rect = registered(&h.ctx)
                    .into_iter()
                    .find(|(_, d)| *d == destination)
                    .unwrap()
                    .0;
                let point = rect.center();
                for _ in 0..32 {
                    frame(
                        &mut h,
                        dock,
                        Some(point),
                        vec![egui::Event::PointerMoved(point)],
                    );
                }
                assert_destination(&h, &destination);
                // Return to the compact layout, then activate the exact same target by click.
                for _ in 0..85 {
                    frame(&mut h, dock, None, vec![egui::Event::PointerGone]);
                }
                let point = registered(&h.ctx)
                    .into_iter()
                    .find(|(_, d)| *d == destination)
                    .unwrap()
                    .0
                    .center();
                for pressed in [true, false] {
                    frame(
                        &mut h,
                        dock,
                        Some(point),
                        vec![
                            egui::Event::PointerMoved(point),
                            egui::Event::PointerButton {
                                pos: point,
                                button: egui::PointerButton::Primary,
                                pressed,
                                modifiers: Default::default(),
                            },
                        ],
                    );
                }
                assert_destination(&h, &destination);
            }
        }
    }
}

#[test]
fn hover_module_restarts_dwell_on_another_metric_and_keeps_the_final_detail() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut h = fixture(Dock::Top, BarStyle::Gauges);
    h.app.settings.hover_page_delay_ms = 800;
    let targets = registered(&h.ctx);
    let cpu = targets
        .iter()
        .find(|(_, d)| *d == Destination::Performance(View::Cpu))
        .unwrap()
        .0
        .center();
    let memory = targets
        .iter()
        .find(|(_, d)| *d == Destination::Performance(View::Memory))
        .unwrap()
        .0
        .center();
    for _ in 0..25 {
        frame(
            &mut h,
            Dock::Top,
            Some(cpu),
            vec![egui::Event::PointerMoved(cpu)],
        );
    }
    for _ in 0..25 {
        frame(
            &mut h,
            Dock::Top,
            Some(memory),
            vec![egui::Event::PointerMoved(memory)],
        );
    }
    assert!(!h.app.open, "a new metric needs a complete dwell");
    for _ in 0..35 {
        frame(
            &mut h,
            Dock::Top,
            Some(memory),
            vec![egui::Event::PointerMoved(memory)],
        );
    }
    assert_destination(&h, &Destination::Performance(View::Memory));
}

#[test]
fn hover_module_does_not_open_during_a_mouse_drag() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut h = fixture(Dock::Top, BarStyle::Gauges);
    let point = registered(&h.ctx)
        .into_iter()
        .find(|(_, d)| *d == Destination::Performance(View::Memory))
        .unwrap()
        .0
        .center();
    frame(
        &mut h,
        Dock::Top,
        None,
        vec![egui::Event::PointerButton {
            pos: Pos2::new(10., 100.),
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: Default::default(),
        }],
    );
    for _ in 0..60 {
        frame(
            &mut h,
            Dock::Top,
            Some(point),
            vec![egui::Event::PointerMoved(point)],
        );
    }
    assert_ne!(
        h.app.page(),
        PERFORMANCE_PAGE,
        "dragging over a metric must not navigate"
    );
    assert!(
        !h.app.open,
        "holding a mouse button must not trigger hover expansion"
    );
}

#[test]
fn hover_module_lyrics_keep_their_music_destination_during_compact_notices() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut h = fixture(Dock::Top, BarStyle::Gauges);
    h.app.settings.island_lyrics = true;
    h.app.go(TODO_PAGE, 1.0);
    h.app.alert("测试提醒", "保留常驻歌词", MAIL_PAGE, false);
    for _ in 0..90 {
        frame(&mut h, Dock::Top, None, vec![]);
    }
    assert!(h.app.compact_notices());
    let point = registered(&h.ctx)
        .into_iter()
        .find(|(_, d)| *d == Destination::Page(MUSIC_PAGE))
        .unwrap()
        .0
        .center();
    for _ in 0..32 {
        frame(
            &mut h,
            Dock::Top,
            Some(point),
            vec![egui::Event::PointerMoved(point)],
        );
    }
    assert_destination(&h, &Destination::Page(MUSIC_PAGE));
}

#[test]
fn hover_module_disabled_destination_does_not_open_an_unrelated_previous_page() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut h = fixture(Dock::Top, BarStyle::Gauges);
    h.app.settings.visible_pages[CALENDAR_PAGE] = false;
    let point = registered(&h.ctx)
        .into_iter()
        .find(|(_, d)| *d == Destination::Page(CALENDAR_PAGE))
        .unwrap()
        .0
        .center();
    for _ in 0..90 {
        frame(
            &mut h,
            Dock::Top,
            Some(point),
            vec![egui::Event::PointerMoved(point)],
        );
    }
    assert!(
        !h.app.open,
        "a disabled calendar must not open the previous music page"
    );
    for pressed in [true, false] {
        frame(
            &mut h,
            Dock::Top,
            Some(point),
            vec![
                egui::Event::PointerMoved(point),
                egui::Event::PointerButton {
                    pos: point,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: Default::default(),
                },
            ],
        );
    }
    assert!(
        h.app.settings_open,
        "clicking a disabled module still opens its settings"
    );
}

#[test]
fn hover_module_custom_painted_labels_have_the_matching_destination() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    for dock in [Dock::Top, Dock::Bottom, Dock::Left, Dock::Right] {
        let h = fixture(dock, BarStyle::Custom);
        let targets = registered(&h.ctx);
        for metric in crate::performance::Metric::ALL {
            let label = metric.rate_label().unwrap_or(metric.label());
            let center = h
                .shapes
                .iter()
                .find_map(|s| match &s.shape {
                    egui::Shape::Text(t) if t.galley.text() == label => {
                        Some(t.visual_bounding_rect().center())
                    }
                    _ => None,
                })
                .unwrap_or_else(|| panic!("{dock:?}: missing painted {label}"));
            let destination = targets
                .iter()
                .rev()
                .find(|(r, _)| r.contains(center))
                .map(|(_, d)| d);
            assert_eq!(
                destination,
                Some(&Destination::Performance(metric.view())),
                "{dock:?}/{label}: the painted label and hover region must agree"
            );
        }
    }
}

#[test]
fn hover_module_new_component_needs_only_a_page_link_and_ghosts_do_not_register() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut h = fixture(Dock::Top, BarStyle::Gauges);
    let point = registered(&h.ctx)
        .into_iter()
        .find(|(_, d)| *d == Destination::Page(CALENDAR_PAGE))
        .unwrap()
        .0
        .center();
    // An arbitrary new component is drawn without adding it to any navigation table.
    for _ in 0..40 {
        h.t += 1. / 60.;
        let env = Env {
            t: h.t,
            cursor: Some(point),
            battery: Some(76),
            ..Default::default()
        };
        let win = Rect::from_min_size(Pos2::ZERO, Vec2::new(WIN_W, WIN_H));
        let _ = h.ctx.run(
            egui::RawInput {
                screen_rect: Some(win),
                time: Some(h.t),
                events: vec![egui::Event::PointerMoved(point)],
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default()
                    .frame(egui::Frame::new())
                    .show(ctx, |ui| {
                        h.app.frame(ui, win, &env);
                        if !h.app.open {
                            let c = crate::paint::Card::new(ui, point - egui::vec2(6., 8.), win);
                            c.text(
                                6.,
                                12.,
                                "待办",
                                10.,
                                crate::theme::W::Regular,
                                egui::Color32::WHITE,
                                crate::paint::H::Center,
                            );
                            c.page_link(
                                0.,
                                0.,
                                12.,
                                16.,
                                "new-component",
                                Destination::Page(TODO_PAGE),
                            );
                            c.ghost().page_link(
                                0.,
                                0.,
                                12.,
                                16.,
                                "hidden-component",
                                Destination::Page(MAIL_PAGE),
                            );
                            c.faded(0.1).page_link(
                                0.,
                                0.,
                                12.,
                                16.,
                                "fading-component",
                                Destination::Page(MAIL_PAGE),
                            );
                        }
                    });
            },
        );
    }
    assert_destination(&h, &Destination::Page(TODO_PAGE));
}

#[test]
fn hover_module_battery_details_render_real_data_and_export_preview() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut h = H::new();
    h.app.settings.pet = false;
    h.app.show_page(BATTERY_PAGE);
    let mut raster = crate::preview_raster::Raster::new(WIN_W as usize, WIN_H as usize);
    for _ in 0..70 {
        h.t += 1. / 60.;
        let env = Env {
            t: h.t,
            clock: "09:41:00".into(),
            battery: Some(76),
            charging: true,
            ..Default::default()
        };
        let output = h.ctx.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(WIN_W, WIN_H))),
                time: Some(h.t),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default()
                    .frame(egui::Frame::new())
                    .show(ctx, |ui| {
                        h.app.frame(
                            ui,
                            Rect::from_min_size(Pos2::ZERO, Vec2::new(WIN_W, WIN_H)),
                            &env,
                        );
                    });
            },
        );
        raster.update_textures(&output.textures_delta);
        h.shapes = output.shapes;
    }
    assert!(h
        .shapes
        .iter()
        .any(|s| matches!(&s.shape,egui::Shape::Text(t) if t.galley.text()=="76%")));
    assert!(h
        .shapes
        .iter()
        .any(|s| matches!(&s.shape,egui::Shape::Text(t) if t.galley.text()=="正在充电")));
    if let Some(path) = std::env::var_os("DAOZAI_HOVER_PREVIEW") {
        raster.buf.fill([0.12, 0.14, 0.18, 1.]);
        raster.draw(&h.ctx.tessellate(h.shapes, 1.), 1.);
        let mut rgba = Vec::new();
        raster.to_rgba8(&mut rgba);
        image::save_buffer(
            path,
            &rgba,
            WIN_W as u32,
            WIN_H as u32,
            image::ColorType::Rgba8,
        )
        .unwrap();
    }
}

#[test]
fn hover_module_quota_accounts_keep_the_exact_account_selection() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    for dock in [Dock::Top, Dock::Bottom, Dock::Left, Dock::Right] {
        let mut h = fixture(dock, BarStyle::Gauges);
        h.app.settings.show_performance = false;
        h.quota_accounts(2);
        for _ in 0..90 {
            frame(&mut h, dock, None, vec![]);
        }
        let quotas: Vec<_> = registered(&h.ctx)
            .into_iter()
            .filter(|(_, d)| matches!(d, Destination::Quota(_)))
            .collect();
        assert_eq!(
            quotas.len(),
            2,
            "both accounts should register their own link"
        );
        for (_, destination) in quotas {
            for _ in 0..90 {
                frame(&mut h, dock, None, vec![egui::Event::PointerGone]);
            }
            let point = registered(&h.ctx)
                .into_iter()
                .find(|(_, d)| *d == destination)
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
            assert_destination(&h, &destination);
        }
    }
}

#[cfg(feature = "sdk-js")]
#[test]
fn hover_module_new_plugin_uses_its_own_status_page_without_a_host_mapping() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    for dock in [Dock::Top, Dock::Left] {
        let mut h = fixture(dock, BarStyle::Gauges);
        h.app.settings.show_performance = false;
        h.app.settings.show_weather = false;
        h.app
            .plugins
            .install(std::path::Path::new(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/spike/examples/canvas-pinned-demo/plugin.json"
            )))
            .unwrap();
        h.app
            .plugins
            .enable("example.canvas-pinned-demo", true)
            .unwrap();
        h.app
            .plugins
            .set_pinned("example.canvas-pinned-demo", true)
            .unwrap();
        for _ in 0..100 {
            frame(&mut h, dock, None, vec![]);
        }
        let destination = Destination::Plugin("example.canvas-pinned-demo".into());
        let point = registered(&h.ctx)
            .into_iter()
            .find(|(_, d)| *d == destination)
            .expect("the plugin should register a link from its shared renderer")
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
        assert_destination(&h, &destination);
    }
}
