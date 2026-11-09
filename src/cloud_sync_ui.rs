use crate::cloud_sync::{self, Config, Mode, Outcome, Snapshot};
use eframe::egui;
use std::{
    collections::BTreeMap,
    sync::mpsc::{self, Receiver},
    time::{Duration, Instant},
};

struct Completed {
    outcome: Outcome,
    restored: bool,
    mail_changed: bool,
}
struct Pending {
    rx: Receiver<Result<Completed, String>>,
    identity: String,
}
#[derive(Clone, Copy)]
enum Notice {
    Info,
    Busy,
    Success,
    Error,
    Warning,
}
pub(crate) struct State {
    config: Config,
    bound_identity: Option<String>,
    webdav_password: String,
    password: String,
    note: String,
    pending: Option<Pending>,
    next: Instant,
    bases: BTreeMap<String, String>,
    dirty: bool,
    notice: Notice,
    show_webdav_password: bool,
    show_password: bool,
}
impl Default for State {
    fn default() -> Self {
        let config = Config::load();
        let webdav_password = cloud_sync::load_secret(&config, "webdav").unwrap_or_default();
        let password = cloud_sync::load_secret(&config, "encryption").unwrap_or_default();
        let bases = std::fs::read(crate::model::config_dir().join("cloud-sync-state.json"))
            .ok()
            .and_then(|v| serde_json::from_slice(&v).ok())
            .unwrap_or_default();
        Self {
            bound_identity:config.identity().ok(),
            config,
            webdav_password,
            password,
            note: String::new(),
            pending: None,
            next: Instant::now() + Duration::from_secs(20),
            bases,
            dirty: false,
            notice: Notice::Info,
            show_webdav_password: false,
            show_password: false,
        }
    }
}
impl State {
    #[cfg(test)]
    pub(crate) fn preview_restored(&mut self,mail_changed:bool){
        let (tx,rx)=mpsc::channel();
        tx.send(Ok(Completed{outcome:Outcome{snapshot:None,fingerprint:"test".into(),note:"恢复完成".into()},restored:true,mail_changed})).unwrap_or_else(|_|panic!("测试通道关闭"));
        self.pending=Some(Pending{rx,identity:"test".into()});
    }
    #[cfg(test)]
    pub(crate) fn preview_notice(&mut self, success: bool) {
        self.notice = if success {
            Notice::Success
        } else {
            Notice::Error
        };
        self.note = if success {
            "配置和邮箱授权码已加密上传 · 12:30:00"
        } else {
            "WebDAV 登录失败，请检查用户名和应用密码"
        }
        .into();
    }
    // Run before showing password fields and again before every save/network action.
    // Invalid intermediate addresses also clear secrets, so A's password cannot leak to B.
    fn bind_connection(&mut self) {
        let identity=self.config.identity().ok();
        if identity!=self.bound_identity {
            self.webdav_password.clear();
            self.password.clear();
            self.show_webdav_password=false;
            self.show_password=false;
            if identity.is_some() {
                self.webdav_password=cloud_sync::load_secret(&self.config,"webdav").unwrap_or_default();
                self.password=cloud_sync::load_secret(&self.config,"encryption").unwrap_or_default();
            }
            self.bound_identity=identity;
        }
    }
    fn start(&mut self, ctx: &egui::Context, mode: Mode) {
        if self.pending.is_some() {return;}
        self.bind_connection();
        self.next=Instant::now()+Duration::from_secs(300);
        let prepare=(||{
            let identity=format!("{}:mail-passwords={}",self.config.identity()?,self.config.mail_passwords);
            if self.webdav_password.is_empty(){return Err("请填写此连接的 WebDAV 应用密码".to_string());}
            if self.password.chars().count()<12{return Err("同步密码至少需要 12 个字符".to_string());}
            Ok(identity)
        })();
        let identity=match prepare {Ok(v)=>v,Err(error)=>{self.note=error;self.notice=Notice::Error;return;}};
        let (tx,rx)=mpsc::channel();
        let cfg=self.config.clone();
        let webdav_password=self.webdav_password.clone();
        let password=self.password.clone();
        let base=self.bases.get(&identity).cloned();
        let ctx=ctx.clone();
        match std::thread::Builder::new().name("daozai-cloud-sync".into()).spawn(move||{
            let result=(||{
                let snapshot=Snapshot::capture_with_passwords(cfg.mail_passwords)?;
                let original=snapshot.fingerprint()?;
                let mut outcome=cloud_sync::run(&cfg,&webdav_password,&password,snapshot,base,mode)?;
                let restored=outcome.snapshot.is_some();
                let mail_changed=if let Some(snapshot)=outcome.snapshot.take(){
                    snapshot.restore_with_passwords(&original,&password,cfg.mail_passwords)?
                }else{false};
                Ok(Completed{outcome,restored,mail_changed})
            })();
            let _=tx.send(result);
            ctx.request_repaint();
        }) {
            Ok(_)=>{self.pending=Some(Pending{rx,identity});self.note="正在同步…".into();self.notice=Notice::Busy;},
            Err(_)=>{self.note="无法启动同步，请稍后重试".into();self.notice=Notice::Error;},
        }
    }
    fn save(&mut self) -> Result<(), String> {
        self.bind_connection();
        self.config.target()?;
        if self.password.chars().count() < 12 {
            return Err("同步密码至少需要 12 个字符".into());
        }
        if self.webdav_password.is_empty() {
            return Err("请填写 WebDAV 应用密码".into());
        }
        cloud_sync::save_secret(&self.config, "webdav", &self.webdav_password)?;
        cloud_sync::save_secret(&self.config, "encryption", &self.password)?;
        self.config.save()?;
        self.dirty = false;
        Ok(())
    }
    fn poll(&mut self) -> Option<bool> {
        let Some(pending) = self.pending.as_ref() else {
            return None;
        };
        let result = match pending.rx.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return None,
            Err(mpsc::TryRecvError::Disconnected) => Err("同步线程已退出，请重新同步".into()),
        };
        let pending = self.pending.take().unwrap();
        let outcome = match result {
            Ok(v) => v,
            Err(error) => {
                self.note = error;
                self.notice = Notice::Error;
                return None;
            }
        };
        let Completed{outcome,restored,mail_changed}=outcome;
        self.bases.insert(pending.identity, outcome.fingerprint);
        let save: std::io::Result<()> = (|| {
            let root = crate::model::config_dir();
            std::fs::create_dir_all(&root)?;
            crate::model::atomic_write(
                &root.join("cloud-sync-state.json"),
                &serde_json::to_vec(&self.bases)?,
            )
        })();
        self.notice = if save.is_ok() {
            Notice::Success
        } else {
            Notice::Warning
        };
        self.note = if save.is_ok() {
            format!(
                "{} · {}",
                outcome.note,
                chrono::Local::now().format("%H:%M:%S")
            )
        } else {
            format!("{}；同步状态未能保存，下次可能需要手动选择", outcome.note)
        };
        restored.then_some(mail_changed)
    }
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        use crate::extras::{settings_card, settings_control_row, settings_hint, settings_label};
        ui.label("通过 WebDAV，在多台电脑之间同步设置和邮箱账号。");
        settings_label(ui, "连接设置");
        settings_card(ui, |ui| {
            ui.add_enabled_ui(self.pending.is_none(), |ui| {
                let width = (ui.available_width() - 72.0 - ui.spacing().item_spacing.x).max(100.0);
                self.dirty |= settings_control_row(ui, "服务器", 72.0, |ui| {
                    ui.add_sized(
                        [width, 30.0],
                        egui::TextEdit::singleline(&mut self.config.folder)
                            .vertical_align(egui::Align::Center)
                            .hint_text("http://100.107.108.80:6086/"),
                    )
                })
                .changed();
                settings_hint(ui, "支持 HTTP / HTTPS、域名和 IP；文件夹需预先创建。");
                self.dirty |= settings_control_row(ui, "用户名", 72.0, |ui| {
                    ui.add_sized(
                        [width, 30.0],
                        egui::TextEdit::singleline(&mut self.config.username)
                            .vertical_align(egui::Align::Center)
                            .hint_text("输入 WebDAV 用户名"),
                    )
                })
                .changed();
                self.bind_connection();
                let secret_width = (width - 48.0 - ui.spacing().item_spacing.x).max(70.0);
                self.dirty |= settings_control_row(ui, "应用密码", 72.0, |ui| {
                    let field = ui.add_sized(
                        [secret_width, 30.0],
                        egui::TextEdit::singleline(&mut self.webdav_password)
                            .vertical_align(egui::Align::Center)
                            .password(!self.show_webdav_password)
                            .hint_text("输入 WebDAV 密码"),
                    );
                    if ui
                        .add_sized(
                            [48.0, 30.0],
                            egui::Button::new(if self.show_webdav_password {
                                "隐藏"
                            } else {
                                "显示"
                            }),
                        )
                        .clicked()
                    {
                        self.show_webdav_password = !self.show_webdav_password;
                    }
                    field
                })
                .changed();
                self.dirty |= settings_control_row(ui, "同步密码", 72.0, |ui| {
                    let field = ui.add_sized(
                        [secret_width, 30.0],
                        egui::TextEdit::singleline(&mut self.password)
                            .vertical_align(egui::Align::Center)
                            .password(!self.show_password)
                            .hint_text("至少 12 个字符，各设备保持一致"),
                    );
                    if ui
                        .add_sized(
                            [48.0, 30.0],
                            egui::Button::new(if self.show_password {
                                "隐藏"
                            } else {
                                "显示"
                            }),
                        )
                        .clicked()
                    {
                        self.show_password = !self.show_password;
                    }
                    field
                })
                .changed();
                settings_hint(
                    ui,
                    "同步密码用于加密配置，请与 WebDAV 密码分开设置并妥善保存。",
                );
                self.dirty |= ui.checkbox(&mut self.config.mail_passwords,"同时备份和恢复邮箱授权码").changed();
                settings_hint(ui,"默认只同步通用设置和邮箱账号。勾选后，邮箱授权码也会加密同步；未配置授权码的账号仍可正常恢复。");
                self.dirty |= ui
                    .checkbox(&mut self.config.automatic, "每 5 分钟自动同步")
                    .changed();
                if self.dirty {
                    settings_hint(ui, "有未保存的修改，自动同步暂时暂停。");
                }
                ui.horizontal_wrapped(|ui| {
                    if ui.button("保存连接").clicked() {
                        match self.save() {
                            Ok(()) => {
                                self.note = "连接已保存".into();
                                self.notice = Notice::Success;
                            }
                            Err(e) => {
                                self.note = e;
                                self.notice = Notice::Error;
                            }
                        }
                    }
                    if ui
                        .add(
                            egui::Button::new(
                                egui::RichText::new("立即同步").color(egui::Color32::WHITE),
                            )
                            .fill(egui::Color32::from_rgb(41, 111, 190)),
                        )
                        .clicked()
                    {
                        self.sync_action(ui.ctx(), Mode::Sync);
                    }
                });
            });
            self.status_ui(ui);
        });
        settings_label(ui, "首次恢复与冲突处理");
        settings_card(ui, |ui| {
            settings_hint(ui,"第一次连接已有云端配置，或两端都有修改时，选择要保留的版本。恢复前会自动备份本机配置。");
            ui.add_enabled_ui(self.pending.is_none(), |ui| {
                ui.horizontal_wrapped(|ui| {
                    if ui.button("用本机覆盖云端").clicked() {
                        self.sync_action(ui.ctx(), Mode::Upload);
                    }
                    if ui.button("用云端恢复本机").clicked() {
                        self.sync_action(ui.ctx(), Mode::Download);
                    }
                });
            });
        });
        ui.add_space(8.0);
        settings_hint(ui,"仅同步明确列出的外观、时钟、专注饮水与收信偏好及邮箱账号；授权码单独选择。邮件正文、验证码、会议信息、设备权限和 AI 登录状态不参与同步。HTTP 建议仅在可信内网或 Tailscale 中使用。");
    }
    fn sync_action(&mut self, ctx: &egui::Context, mode: Mode) {
        match self.save() {
            Ok(()) => self.start(ctx, mode),
            Err(e) => {
                self.note = e;
                self.notice = Notice::Error;
            }
        }
    }
    fn status_ui(&self, ui: &mut egui::Ui) {
        if self.note.is_empty() {
            return;
        }
        let dark = ui.visuals().dark_mode;
        let (title, color) = match self.notice {
            Notice::Error => (
                "同步未完成",
                if dark {
                    egui::Color32::from_rgb(255, 139, 139)
                } else {
                    egui::Color32::from_rgb(175, 35, 45)
                },
            ),
            Notice::Success => (
                if self.note.contains("配置已是最新") { "无需同步" }
                else if self.note.starts_with("连接已保存") { "连接已保存" }
                else { "同步成功" },
                if dark {
                    egui::Color32::from_rgb(112, 221, 160)
                } else {
                    egui::Color32::from_rgb(25, 117, 65)
                },
            ),
            Notice::Warning => (
                "需要注意",
                if dark {
                    egui::Color32::from_rgb(246, 200, 112)
                } else {
                    egui::Color32::from_rgb(144, 91, 15)
                },
            ),
            _ => (
                "正在同步",
                if dark {
                    egui::Color32::from_rgb(140, 192, 255)
                } else {
                    egui::Color32::from_rgb(35, 99, 173)
                },
            ),
        };
        ui.add_space(4.0);
        egui::Frame::new()
            .fill(color.gamma_multiply(if dark { 0.12 } else { 0.07 }))
            .stroke(egui::Stroke::new(1.0, color.gamma_multiply(0.45)))
            .corner_radius(7.0)
            .inner_margin(10)
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                let show_detail = matches!(self.notice, Notice::Error | Notice::Warning);
                ui.horizontal_wrapped(|ui| {
                    if matches!(self.notice, Notice::Busy) {
                        ui.spinner();
                    }
                    ui.add(egui::Label::new(egui::RichText::new(if show_detail { title } else { &self.note }).strong().color(color)).wrap());
                });
                if show_detail {
                    ui.add(egui::Label::new(egui::RichText::new(&self.note).strong().color(color)).wrap());
                }
            });
    }
}
impl crate::app::App {
    pub(crate) fn poll_cloud_sync(&mut self, ctx: &egui::Context) {
        if let Some(mail_changed)=self.cloud_sync.poll() {
            self.settings = crate::model::Settings::load();
            self.set_page_visible(self.page, self.settings.visible_pages[self.page]);
            if mail_changed {
                self.mail = crate::app::MailState {
                    accounts: crate::mail::load_accounts(),
                    ..Default::default()
                };
                self.pending.push(crate::app::Cmd::MailReconnect);
            }
        }
        if self.cloud_sync.config.automatic
            && !self.cloud_sync.dirty
            && self.cloud_sync.pending.is_none()
            && Instant::now() >= self.cloud_sync.next
        {
            self.cloud_sync.start(ctx, Mode::Sync);
        }
        if self.cloud_sync.config.automatic {
            ctx.request_repaint_after(Duration::from_secs(5));
        }
        if self.cloud_sync.pending.is_some() {
            ctx.request_repaint_after(Duration::from_millis(250));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cloud_sync_changing_connection_never_reuses_old_passwords() {
        let name=format!("synthetic-{}",std::process::id());
        let a=Config{folder:"https://a.example.invalid/dav".into(),username:name.clone(),..Default::default()};
        let b=Config{folder:"https://b.example.invalid/dav".into(),username:name.clone(),..Default::default()};
        let mut state=State::default();
        state.config=a.clone();state.bound_identity=a.identity().ok();
        state.webdav_password="password-for-a".into();state.password="encryption-for-a".into();
        state.config=b;state.bind_connection();
        assert!(state.webdav_password.is_empty());assert!(state.password.is_empty());
        state.webdav_password="password-for-b".into();
        state.config.username.push_str("-other-user");state.bind_connection();
        assert!(state.webdav_password.is_empty());
        state.webdav_password="password-for-other-user".into();
        state.config.folder="not-a-url".into();state.bind_connection();
        assert!(state.webdav_password.is_empty());assert!(state.password.is_empty());
        assert!(!Config::default().mail_passwords);
    }
}
