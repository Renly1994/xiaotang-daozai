use crate::{
    app::{App, Cmd},
    paint::{Card, H},
    steam::{self, Action, Config, Event, Snapshot},
    theme::{self, W},
};
use eframe::egui::{self, Color32};

pub(crate) struct State {
    pub config: Option<Config>,
    pub snapshot: Snapshot,
    pub busy: bool,
    pub stale: bool,
    key: String,
    show_key: bool,
    watch_input: String,
    pub note: String,
    pub error: bool,
}
impl State {
    pub fn load() -> Self {
        let config = Config::load();
        let watch_input = config
            .as_ref()
            .and_then(|c| c.watch_app)
            .map(|id| id.to_string())
            .unwrap_or_default();
        Self {
            config,
            snapshot: Snapshot::default(),
            busy: false,
            stale: true,
            key: String::new(),
            show_key: false,
            watch_input,
            note: String::new(),
            error: false,
        }
    }
    pub fn game(&self) -> Option<&steam::Game> {
        if self.stale || self.config.is_none() {
            None
        } else {
            self.snapshot.player.as_ref()?.game.as_ref()
        }
    }
    pub fn badge(&self) -> Option<(&'static str, &steam::Game)> {
        if let Some(game) = self.game() {
            return Some(("正在玩", game));
        }
        if !self.stale && self.config.as_ref().is_some_and(|c| c.watch_app.is_some()) {
            return self
                .snapshot
                .achievement_game
                .as_ref()
                .map(|game| ("", game));
        }
        None
    }
}

impl App {
    pub fn open_steam_settings(&mut self) {
        self.settings_tab = 12;
        self.open_settings();
    }
    pub fn steam_event(&mut self, event: Event) {
        match event {
            Event::LoginUrl(url) => {
                self.steam.note = "请在浏览器中完成 Steam 官方登录，最多等待 3 分钟。".into();
                self.pending.push(Cmd::OpenUrl(url));
            }
            Event::Connected(config) => {
                let target_changed =
                    self.steam.config.as_ref().and_then(|c| c.watch_app) != config.watch_app;
                if target_changed || self.steam.config.is_none() {
                    self.steam.watch_input = config
                        .watch_app
                        .map(|id| id.to_string())
                        .unwrap_or_default();
                }
                if target_changed {
                    self.steam.snapshot.achievement_game = None;
                    self.steam.snapshot.achievements_at.clear();
                    self.steam.snapshot.achievement_icons.clear();
                    self.steam.snapshot.unlocked = 0;
                    self.steam.snapshot.total = 0;
                    self.steam.snapshot.note = "正在同步所选游戏的成就基线…".into();
                }
                if self.steam.config.as_ref().map(|c| &c.steamid) != Some(&config.steamid) {
                    self.steam.snapshot = Snapshot::default();
                    self.steam.stale = true;
                    self.alerts
                        .retain(|a| !matches!(a.source, crate::extras::AlertSource::Steam(..)));
                    self.history
                        .retain(|a| !matches!(a.source, crate::extras::AlertSource::Steam(..)));
                    if self
                        .notice
                        .as_ref()
                        .is_some_and(|a| matches!(a.source, crate::extras::AlertSource::Steam(..)))
                    {
                        self.notice = None;
                    }
                }
                self.steam.config = Some(config);
                self.steam.busy = false;
                self.steam.error = false;
                self.steam.note = "连接已保存。".into();
            }
            Event::Snapshot { steamid, snapshot } => {
                if self
                    .steam
                    .config
                    .as_ref()
                    .is_some_and(|c| c.steamid == steamid)
                {
                    self.steam.stale = snapshot.player_stale;
                    self.steam.snapshot = snapshot;
                    self.steam.busy = false;
                    self.steam.error = false;
                    self.steam.note.clear();
                }
            }
            Event::Unlocked {
                steamid,
                game,
                achievement,
            } => {
                if self
                    .steam
                    .config
                    .as_ref()
                    .is_some_and(|c| c.steamid == steamid && c.notify)
                {
                    self.enqueue_alert(
                        crate::extras::Alert {
                            source: crate::extras::AlertSource::Steam(
                                steamid,
                                format!("{}:{}", game.id, achievement.api_name),
                            ),
                            title: steam::NOTICE.into(),
                            body: format!("{} · {}", achievement.name, game.name),
                            page: 0,
                            at: self.t,
                            duration: self.settings.steam_notice_seconds as f64,
                            bluetooth: None,
                            meeting: None,
                            steam: Some(crate::extras::SteamNotice {
                                game: game.name,
                                achievement: achievement.name,
                                description: achievement.description,
                                icon_url: achievement.icon,
                            }),
                        },
                        false,
                    );
                }
            }
            Event::Error(note) => {
                self.steam.note = note;
                self.steam.error = true;
                self.steam.busy = false;
                self.steam.stale = true;
            }
            Event::Disconnected(error) => {
                self.steam = State::load();
                self.steam.config = None;
                self.steam.error = error.is_some();
                self.steam.note =
                    error.unwrap_or_else(|| "已断开连接并清除本地凭据和成就缓存。".into());
                self.alerts.retain(|a| a.title != steam::NOTICE);
                if self
                    .notice
                    .as_ref()
                    .is_some_and(|a| a.title == steam::NOTICE)
                {
                    self.notice = None;
                }
                self.history.retain(|a| a.title != steam::NOTICE);
            }
            Event::Cancelled => {
                self.steam.busy = false;
                self.steam.note = "已取消连接。".into();
                self.steam.error = false;
            }
        }
    }
    pub fn steam_fullscreen_notice(&self) -> bool {
        self.steam.config.as_ref().is_some_and(|c|c.notify&&c.fullscreen)&&self.notice.as_ref().is_some_and(|a|matches!(&a.source,crate::extras::AlertSource::Steam(account,_) if self.steam.config.as_ref().is_some_and(|c|c.steamid==*account)))
    }
    pub(crate) fn steam_settings(&mut self, ui: &mut egui::Ui) {
        use crate::extras::{setting_row, settings_card, settings_label};
        ui.label("连接你的 Steam，查看当前游戏，并在解锁成就后收到岛上提醒。");
        ui.add_space(8.0);
        settings_label(ui, "账号与游戏");
        settings_card(ui, |ui| {
            if let Some(config) = &self.steam.config {
                let name = self
                    .steam
                    .snapshot
                    .player
                    .as_ref()
                    .map(|p| p.name.as_str())
                    .unwrap_or("已连接 Steam 账号");
                ui.label(egui::RichText::new(name).strong().size(15.0));
                ui.label(egui::RichText::new(format!("SteamID · {}", config.steamid)).small());
                if let Some(player) = &self.steam.snapshot.player {
                    ui.label(if self.steam.stale {
                        format!("上次状态 · {}", player.status())
                    } else {
                        player.status().into()
                    });
                    if let Some(game) = &player.game {
                        ui.label(
                            egui::RichText::new(format!(
                                "{}{}",
                                if self.steam.stale {
                                    "上次游戏 · "
                                } else {
                                    "正在玩 · "
                                },
                                game.name
                            ))
                            .strong(),
                        );
                    }
                } else {
                    ui.label("正在获取账号状态…");
                }
                if self.steam.snapshot.total > 0 {
                    ui.label(format!(
                        "{} · {} / {}",
                        if self.steam.stale || !self.steam.snapshot.note.is_empty() {
                            "最近读取成就"
                        } else {
                            if config.watch_app.is_some() {
                                "跟踪游戏成就"
                            } else {
                                "当前游戏成就"
                            }
                        },
                        self.steam.snapshot.unlocked,
                        self.steam.snapshot.total
                    ));
                }
                if let Some(game) = &self.steam.snapshot.achievement_game {
                    if config.watch_app.is_some()
                        || self.steam.game().map(|g| g.id) != Some(game.id)
                    {
                        ui.label(format!("成就跟踪 · {}", game.name));
                    }
                    if self.steam.snapshot.achievements_at.is_empty() {
                        ui.label("等待首次同步，历史成就保持静默");
                    } else {
                        ui.label(
                            egui::RichText::new(format!(
                                "基线已建立 · 成就读取 {}",
                                self.steam.snapshot.achievements_at
                            ))
                            .small(),
                        );
                    }
                }
                if !self.steam.snapshot.local_status.is_empty() {
                    ui.label(egui::RichText::new(self.steam.snapshot.local_status).small());
                }
                if !self.steam.snapshot.at.is_empty()
                    && self.steam.snapshot.achievements_at.is_empty()
                {
                    ui.label(
                        egui::RichText::new(format!("上次同步 {}", self.steam.snapshot.at)).small(),
                    );
                }
                if !self.steam.snapshot.note.is_empty() {
                    ui.label(&self.steam.snapshot.note);
                }
                ui.add_space(6.0);
                ui.horizontal_wrapped(|ui| {
                    if ui
                        .add_enabled(
                            !self.steam.busy,
                            egui::Button::new(if self.steam.busy {
                                "同步中…"
                            } else {
                                "刷新状态"
                            }),
                        )
                        .clicked()
                    {
                        self.steam.busy = true;
                        self.pending.push(Cmd::Steam(Action::Refresh));
                    }
                    if ui.button("断开连接").clicked() {
                        self.steam.busy = true;
                        self.pending.push(Cmd::Steam(Action::Disconnect));
                    }
                    if ui.button("预览成就通知").clicked() {
                        self.steam_preview();
                    }
                });
            } else {
                ui.label("尚未连接 Steam");
                ui.label("先在下方填写 API Key，再选择本机账号或浏览器登录。");
            }
        });
        if let Some(config) = self.steam.config.clone() {
            settings_label(ui, "跟踪游戏");
            settings_card(ui, |ui| {
                ui.label(if let Some(id) = config.watch_app {
                    format!("指定游戏 · AppID {id}")
                } else {
                    "自动跟随正在玩的游戏".into()
                });
                ui.add(
                    egui::TextEdit::singleline(&mut self.steam.watch_input)
                        .hint_text("游戏 AppID 或 Steam 商店链接")
                        .char_limit(256)
                        .desired_width(f32::INFINITY),
                );
                ui.horizontal_wrapped(|ui| {
                    let app = steam::parse_app_id(&self.steam.watch_input);
                    if ui
                        .add_enabled(
                            app.is_some() && !self.steam.busy,
                            egui::Button::new("指定跟踪"),
                        )
                        .clicked()
                    {
                        self.steam.busy = true;
                        self.pending.push(Cmd::Steam(Action::WatchGame(app)));
                    }
                    if ui
                        .add_enabled(
                            config.watch_app.is_some() && !self.steam.busy,
                            egui::Button::new("恢复自动跟随"),
                        )
                        .clicked()
                    {
                        self.steam.busy = true;
                        self.pending.push(Cmd::Steam(Action::WatchGame(None)));
                    }
                });
                ui.label(
                    egui::RichText::new(
                        "指定游戏后，游戏关闭时也会检查成就。测试前请先等待基线建立。",
                    )
                    .small(),
                );
                ui.label(egui::RichText::new("重复测试：反解锁后刷新一次，再解锁并刷新。").small());
            });
        }
        settings_label(
            ui,
            if self.steam.config.is_some() {
                "更换账号或 API Key"
            } else {
                "连接 Steam"
            },
        );
        settings_card(ui, |ui| {
            ui.label("Steam Web API Key");
            ui.add(
                egui::TextEdit::singleline(&mut self.steam.key)
                    .password(!self.steam.show_key)
                    .hint_text("粘贴 32 位 API Key")
                    .char_limit(64)
                    .desired_width(f32::INFINITY),
            );
            ui.horizontal_wrapped(|ui| {
                ui.checkbox(&mut self.steam.show_key, "显示 Key");
                if ui.link("获取 API Key").clicked() {
                    self.pending
                        .push(Cmd::OpenUrl("https://steamcommunity.com/dev/apikey".into()));
                }
            });
            ui.label(
                egui::RichText::new(
                    "Key 仅保存在本机 Windows 凭据管理器中。这里不填写 Steam 密码。",
                )
                .small(),
            );
            ui.add_space(7.0);
            ui.horizontal_wrapped(|ui| {
                let valid = steam::valid_key(self.steam.key.trim());
                if ui
                    .add_enabled(
                        valid && !self.steam.busy,
                        egui::Button::new("使用本机 Steam 账号"),
                    )
                    .clicked()
                {
                    self.start_steam_connect(false);
                }
                if ui
                    .add_enabled(
                        valid && !self.steam.busy,
                        egui::Button::new("在 Steam 官网登录"),
                    )
                    .clicked()
                {
                    self.start_steam_connect(true);
                }
            });
            ui.label(egui::RichText::new("本机方式读取 Steam 当前账号编号；官网方式在浏览器验证身份，岛仔不会读取 Steam 登录凭据。").small());
            if self.steam.busy {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("正在连接或同步…");
                    if ui.button("取消").clicked() {
                        self.pending.push(Cmd::Steam(Action::Cancel));
                    }
                });
            }
        });
        if !self.steam.note.is_empty() {
            ui.add_space(6.0);
            ui.colored_label(
                if self.steam.error {
                    crate::settings_theme::color(
                        ui.ctx(),
                        Color32::from_rgb(255, 145, 145),
                        Color32::from_rgb(165, 35, 35),
                    )
                } else {
                    ui.visuals().text_color()
                },
                &self.steam.note,
            );
            if self.steam.error
                && self.steam.config.is_none()
                && ui.button("清除本地连接信息").clicked()
            {
                self.pending.push(Cmd::Steam(Action::Disconnect));
            }
        }
        if let Some(c) = self.steam.config.clone() {
            if !self.settings.notifications {
                ui.label("通知总开关已关闭，Steam 成就提醒也会暂停。");
                if ui.link("打开通知设置").clicked() {
                    self.settings_tab = 6;
                }
            }
            settings_label(ui, "成就提醒");
            settings_card(ui, |ui| {
                let mut notify = c.notify;
                let mut fullscreen = c.fullscreen;
                ui.horizontal(|ui| {
                    ui.label("显示时长");
                    ui.add(
                        egui::Slider::new(&mut self.settings.steam_notice_seconds, 3..=30)
                            .suffix(" 秒"),
                    );
                });
                ui.separator();
                setting_row(
                    ui,
                    "成就解锁提醒",
                    "首次同步不弹历史成就；新成就按顺序显示",
                    &mut notify,
                );
                setting_row(
                    ui,
                    "全屏游戏时允许提醒",
                    "仅允许 Steam 成就提醒；独占全屏可能无法显示",
                    &mut fullscreen,
                );
                if notify != c.notify || fullscreen != c.fullscreen {
                    self.pending
                        .push(Cmd::Steam(Action::Options { notify, fullscreen }));
                }
            });
        }
        ui.add_space(10.0);
        ui.label(egui::RichText::new("每秒检查本地成就缓存，Steam 写盘后即可检测。在线同步约每 30 秒运行一次，空闲时约 60 秒。账号编号和成就去重记录保存在本机，断开连接时清除。").small());
    }
    pub(crate) fn steam_preview(&mut self) {
        let Some(config) = self.steam.config.clone() else {
            return;
        };
        if !self.settings.notifications || !config.notify {
            self.steam.note = "请先开启通知总开关和 Steam 成就提醒。".into();
            self.steam.error = true;
            return;
        }
        let game = self
            .steam
            .snapshot
            .achievement_game
            .clone()
            .or_else(|| {
                self.steam
                    .snapshot
                    .player
                    .as_ref()
                    .and_then(|p| p.game.clone())
            })
            .unwrap_or(steam::Game {
                id: 0,
                name: "Steam 通知预览".into(),
                icon: None,
            });
        self.steam_event(Event::Unlocked {
            steamid: config.steamid,
            game,
            achievement: steam::Achievement {
                api_name: format!("preview-{}", self.t),
                name: "测试成就通知".into(),
                description: String::new(),
                unlocked: true,
                unlocktime: 0,
                icon: None,
            },
        });
        self.steam.note = "测试通知已进入岛上队列。".into();
        self.steam.error = false;
    }

    pub(crate) fn draw_steam_shortcut(&mut self, c: &Card<'_>, x: f32, y: f32) {
        let badge = self.steam.badge();
        let icon = badge.and_then(|(_, game)| game.icon.as_deref());
        if let Some(texture) = self.steam_images.get(c.ui.ctx(), icon, None) {
            c.painter.image(texture, c.rect(x-9.0,y-9.0,18.0,18.0),
                egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)), Color32::WHITE);
        } else {
            crate::extras::device_icon(c, crate::bt::DeviceKind::Gamepad, x-9.0, y-9.0, 18.0);
        }
        let (id, tip) = match badge {
            Some((label, game)) if !label.is_empty() => ("steam-current-game", format!("{label} · {}", game.name)),
            Some((_, game)) => ("steam-current-game", game.name.clone()),
            None => ("steam-shortcut", "打开 Steam 页面".to_owned()),
        };
        if c.hit(x-13.0,y-13.0,26.0,26.0,id).on_hover_text(tip).clicked() { self.show_page(8); }
    }

    pub(crate) fn draw_steam_avatar(&mut self, c: &Card<'_>) {
        if let Some(url) = self
            .steam
            .snapshot
            .player
            .as_ref()
            .and_then(|player| player.avatar.as_deref())
        {
            if let Some(texture) = self.steam_images.get(c.ui.ctx(), None, Some(url)) {
                c.painter.image(
                    texture,
                    c.rect(73.0, 9.0, 19.0, 19.0),
                    egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                    Color32::WHITE,
                );
            }
        }
    }

    pub(crate) fn page_steam(&mut self, c: &Card<'_>) {
        let side = self.side();
        if !side { self.draw_steam_avatar(c); }
        if let Some((_, game)) = self
            .steam
            .badge()
            .map(|(label, game)| (label, game.clone()))
        {
            let icon = self
                .steam_images
                .get(c.ui.ctx(), game.icon.as_deref(), None);
            let (icon_x, icon_y) = if side { (57.0, 12.0) } else { (38.0, 58.0) };
            if let Some(texture) = icon {
                let rect = c.rect(icon_x, icon_y, 42.0, 42.0);
                c.painter.image(
                    texture,
                    rect,
                    egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                    Color32::WHITE,
                );
            } else {
                crate::extras::device_icon(c, crate::bt::DeviceKind::Gamepad, icon_x+5.0, icon_y+5.0, 32.0);
            }
            if side && self.steam.game().is_some() {
                c.centered_text(78.0, 66.0, "正在玩", 10.5, W::Regular, theme::GRAY, H::Center);
            }
            let (title_x, align) = if side {
                c.scrolling_title((12.0, 85.0, 132.0), &game.name, 15.0, self.t, "side-steam-title");
                (78.0, H::Center)
            } else {
                c.text(88.0, 76.0, &c.fit(&game.name, 15.0, W::Semibold, 150.0), 15.0, W::Semibold, theme::WHITE, H::Left);
                (88.0, H::Left)
            };
            let tracked = self.steam.snapshot.achievement_game.as_ref().filter(|tracked| tracked.id != game.id);
            if self.steam.snapshot.total > 0 {
                let (x, y, align) = match (side, tracked.is_some()) {
                    (true, true) => (78.0, 135.0, H::Center),
                    (false, true) => (248.0, 113.0, H::Left),
                    (true, false) => (title_x, 111.0, align),
                    (false, false) => (title_x, 98.0, align),
                };
                c.text(
                    x,
                    y,
                    &format!(
                        "已解锁 {} / {} 项成就",
                        self.steam.snapshot.unlocked, self.steam.snapshot.total
                    ),
                    11.5,
                    W::Regular,
                    Color32::from_gray(190),
                    align,
                );
            }
            if let Some(tracked) = tracked {
                let label = format!("成就 · {}", tracked.name);
                let (x, y, align) = if side { (78.0, 117.0, H::Center) } else { (248.0, 59.0, H::Left) };
                c.text(x, y, &c.fit(&label, 9.5, W::Regular, 132.0), 9.5, W::Regular, theme::GRAY, align);
            } else if side && !self.steam.snapshot.recent_achievements.is_empty() {
                c.text(78.0, 135.0, "近期解锁", 11.0, W::Semibold, theme::WHITE, H::Center);
            }
            for (index,achievement) in self.steam.snapshot.recent_achievements.iter().filter(|achievement|achievement.icon.is_some()).take(4).enumerate() {
                let url=achievement.icon.as_deref().unwrap();
                let (x,y) = if side { (63.0, 145.0+34.0*index as f32) } else { (248.0+34.0*index as f32, 66.0) };
                c.fill(x,y,30.0,30.0,5.0,Color32::from_gray(24));
                if let Some(texture)=self.steam_images.get(c.ui.ctx(),None,Some(url)) {
                    c.painter.image(texture,c.rect(x,y,30.0,30.0),egui::Rect::from_min_max(egui::Pos2::ZERO,egui::pos2(1.0,1.0)),Color32::WHITE);
                }
                c.hit(x,y-2.0,30.0,34.0,("steam-recent-achievement",index)).on_hover_text(&achievement.name);
            }
        } else if side {
            crate::extras::device_icon(c, crate::bt::DeviceKind::Gamepad, 62.0, 22.0, 32.0);
            let connected = self.steam.config.is_some();
            let heading = if connected { "等待游戏状态" } else { "连接 Steam" };
            c.centered_text(78.0, 73.0, heading, 13.0, W::Semibold, theme::WHITE, H::Center);
            let note = if !connected { "连接账号后查看游戏与成就" }
                else if self.steam.note.is_empty() { &self.steam.snapshot.note } else { &self.steam.note };
            let lines = c.wrap(note, 10.0, W::Regular, 130.0, 4);
            for (index, line) in lines.iter().enumerate() {
                c.centered_text(78.0, 94.0 + index as f32 * 15.0, line, 10.0, W::Regular, theme::GRAY, H::Center);
            }
            let y = 94.0 + lines.len() as f32 * 15.0;
            let (response, button) = crate::paint::capsule(c, 30.0, y, 96.0, 24.0, "steam-side-settings", theme::BTN, theme::BTN_HOVER);
            button.centered_text(78.0, y + 12.0, "Steam 设置", 11.0, W::Semibold, theme::WHITE, H::Center);
            if response.clicked() { self.open_steam_settings(); }
        } else if self.steam.config.is_some() {
            crate::extras::device_icon(c, crate::bt::DeviceKind::Gamepad, 43.0, 63.0, 32.0);
            c.text(
                88.0,
                70.0,
                "等待 Steam 游戏状态",
                13.0,
                W::Semibold,
                theme::WHITE,
                H::Left,
            );
            c.text(
                88.0,
                89.0,
                &c.fit(&self.steam.snapshot.note, 11.0, W::Regular, 280.0),
                10.0,
                W::Regular,
                theme::GRAY,
                H::Left,
            );
            if !self.steam.note.is_empty() {
                c.text(
                    88.0,
                    106.0,
                    &c.fit(&self.steam.note, 9.5, W::Regular, 220.0),
                    9.5,
                    W::Regular,
                    theme::GRAY,
                    H::Left,
                );
            }
        } else {
            crate::extras::device_icon(c, crate::bt::DeviceKind::Gamepad, 43.0, 63.0, 32.0);
            c.text(
                88.0,
                70.0,
                "连接 Steam 查看游戏与成就",
                13.0,
                W::Semibold,
                theme::WHITE,
                H::Left,
            );
            c.text(
                88.0,
                89.0,
                "也可以点击右上角齿轮连接账号",
                10.0,
                W::Regular,
                theme::GRAY,
                H::Left,
            );
            let (response, button) = crate::paint::capsule(
                c,
                88.0,
                101.0,
                96.0,
                24.0,
                "steam-page-settings",
                theme::CARD,
                Color32::from_gray(48),
            );
            button.centered_text(
                136.0,
                113.0,
                "Steam 设置",
                10.5,
                W::Semibold,
                theme::WHITE,
                H::Center,
            );
            if response.clicked() {
                self.open_steam_settings();
            }
        }
    }
    fn start_steam_connect(&mut self, browser: bool) {
        let key = std::mem::take(&mut self.steam.key);
        self.steam.show_key = false;
        self.steam.busy = true;
        self.steam.error = false;
        self.steam.note = if browser {
            "正在打开 Steam 官方登录…"
        } else {
            "正在连接本机 Steam 账号…"
        }
        .into();
        self.pending.push(Cmd::Steam(if browser {
            Action::Login(key)
        } else {
            Action::Local(key)
        }));
    }
}
