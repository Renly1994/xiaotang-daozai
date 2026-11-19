//! Fixed local status signals. No message accepts a string payload from another process.
use std::{fs, io, path::{Path, PathBuf}, sync::mpsc};
use eframe::egui;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Provider { Dsh, Codex }

/// 一个来源的全部可扩展点：线上标识、名称、图标、设置字段读写。
/// 新增来源只需追加 `SITES` 一项并补齐枚举与设置字段。
pub struct ProviderMeta {
    pub provider: Provider,
    pub key: &'static str,
    pub label: &'static str,
    pub icon: fn() -> egui::Image<'static>,
    pub setting: fn(&crate::model::Settings) -> bool,
    pub setting_mut: fn(&mut crate::model::Settings) -> &mut bool,
}
fn dsh_icon() -> egui::Image<'static> { egui::Image::new(egui::include_image!("../assets/ai-icons/deepseek-harness.png")) }
fn codex_icon() -> egui::Image<'static> { egui::Image::new(egui::include_image!("../assets/ai-icons/codex-app.png")) }
fn dsh_setting(s: &crate::model::Settings) -> bool { s.dsh_notifications }
fn codex_setting(s: &crate::model::Settings) -> bool { s.codex_notifications }
fn dsh_setting_mut(s: &mut crate::model::Settings) -> &mut bool { &mut s.dsh_notifications }
fn codex_setting_mut(s: &mut crate::model::Settings) -> &mut bool { &mut s.codex_notifications }

/// 表顺序即 `SIGNALS` 的 wire 顺序：只能追加，不能重排。
pub const SITES: [ProviderMeta; 2] = [
    ProviderMeta { provider: Provider::Dsh, key: "dsh", label: "DeepSeek Harness", icon: dsh_icon, setting: dsh_setting, setting_mut: dsh_setting_mut },
    ProviderMeta { provider: Provider::Codex, key: "codex", label: "Codex", icon: codex_icon, setting: codex_setting, setting_mut: codex_setting_mut },
];

impl Provider {
    pub const ALL: [Self; 2] = [Self::Dsh, Self::Codex];
    pub fn meta(self) -> &'static ProviderMeta {
        SITES.iter().find(|meta|meta.provider==self).expect("每个 Provider 都必须在 SITES 中登记")
    }
    fn id(self) -> &'static str { self.meta().key }
    pub fn label(self) -> &'static str { self.meta().label }
    /// 卡片状态行用的短名（对齐参考项目的 `DSH` chip）。
    pub fn short(self) -> &'static str {
        match self {
            Self::Dsh => "DSH",
            Self::Codex => "Codex",
        }
    }
    pub fn icon(self) -> egui::Image<'static> { (self.meta().icon)() }
    pub fn setting_mut(self, settings: &mut crate::model::Settings) -> &mut bool { (self.meta().setting_mut)(settings) }
    pub fn enabled(self, settings: &crate::model::Settings) -> bool {
        settings.notifications && (self.meta().setting)(settings)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status { Finished, Approval, Question }

impl Status {
    pub fn label(self) -> &'static str {
        match self {
            Self::Finished => "本轮已结束",
            Self::Approval => "等待你审批，请回到客户端",
            Self::Question => "有问题需要你回答，请回到客户端",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Notice(pub Provider, pub Status);

// The index + 1 is the wire ID; keep existing entries in order.
const SIGNALS: [(&str, &str, Notice); 6] = [
    ("dsh","finished",Notice(Provider::Dsh,Status::Finished)),
    ("dsh","approval",Notice(Provider::Dsh,Status::Approval)),
    ("dsh","question",Notice(Provider::Dsh,Status::Question)),
    ("codex","finished",Notice(Provider::Codex,Status::Finished)),
    ("codex","approval",Notice(Provider::Codex,Status::Approval)),
    ("codex","question",Notice(Provider::Codex,Status::Question)),
];

pub fn wire_id(provider: &str, status: &str) -> Option<usize> {
    SIGNALS.iter().position(|&(p,s,_)| (p,s)==(provider,status)).map(|i| i+1)
}
pub fn decode(id: usize) -> Option<Notice> {
    SIGNALS.get(id.checked_sub(1)?).map(|&(_,_,notice)| notice)
}

fn write_changed(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if fs::read(path).is_ok_and(|old|old==bytes) { return Ok(()); }
    crate::model::atomic_write(path, bytes)
}

pub fn export(dir: &Path, executable: &Path) -> io::Result<()> {
    fs::create_dir_all(dir.join("bridge"))?;
    for (name, data) in [
        ("README.md", include_str!("../docs/AGENT-NOTICES.md")),
        ("connect.mjs", include_str!("../integrations/agent-notices/connect.mjs")),
        ("bridge/index.mjs", include_str!("../integrations/agent-notices/bridge/index.mjs")),
        ("bridge/channel.cjs", include_str!("../integrations/agent-notices/bridge/channel.cjs")),
        ("bridge/notify.cjs", include_str!("../integrations/agent-notices/bridge/notify.cjs")),
        ("bridge/cordis.patch.yml", include_str!("../integrations/agent-notices/bridge/cordis.patch.yml")),
        ("bridge/package.json", include_str!("../integrations/agent-notices/bridge/package.json")),
    ] {
        write_changed(&dir.join(name), data.as_bytes())?;
    }
    write_changed(&dir.join("bridge/target.json"), &serde_json::to_vec(executable)?)
}

#[derive(Default)]
pub enum Connection { #[default] Off, Installing, Configured, Seen, Failed(&'static str) }
impl Connection {
    pub fn description(&self, provider: Provider) -> &'static str {
        match self {
            Self::Off => "轮次结束、等待审批或提问",
            Self::Installing => "正在接入…",
            Self::Configured => match provider {
                Provider::Dsh => "等待交互插件连接 · 请完全退出并重开客户端",
                Provider::Codex => "已配置 · 请在 Codex 确认 hook 信任并新开会话",
            },
            Self::Seen if provider==Provider::Dsh => "已收到通知 · 未连接交互插件，请完全退出并重开客户端",
            Self::Seen => "已收到客户端提示",
            Self::Failed(reason) => reason,
        }
    }
}
type SetupResult = Result<(), &'static str>;
#[derive(Default)]
pub struct Setup {
    states: [Connection; 2],
    worker: Option<(Provider,mpsc::Receiver<SetupResult>)>,
}
impl Setup {
    pub fn state(&self,p:Provider)->&Connection { &self.states[p as usize] }
    pub fn retry(&mut self,p:Provider) { self.states[p as usize]=Connection::Off; }
    pub fn received(&mut self,p:Provider) { self.states[p as usize]=Connection::Seen; }

    pub fn update(&mut self, settings:&crate::model::Settings, ctx:&egui::Context) {
        self.reconcile(Provider::ALL.map(|p|p.enabled(settings)), |provider| {
            let (tx,rx)=mpsc::channel();
            let ctx=ctx.clone();
            std::thread::spawn(move || {
                let result=install_default(provider);
                let _=tx.send(result);
                ctx.request_repaint();
            });
            rx
        });
    }
    fn reconcile(&mut self, enabled:[bool;2], start:impl FnOnce(Provider)->mpsc::Receiver<SetupResult>) {
        if let Some((p,rx))=&self.worker {
            let result=match rx.try_recv() {
                Ok(result)=>Some(result),
                Err(mpsc::TryRecvError::Disconnected)=>Some(Err("接入中断，可以重试")),
                Err(mpsc::TryRecvError::Empty)=>None,
            };
            if let Some(result)=result {
                if !matches!(self.state(*p),Connection::Seen) {
                    self.states[*p as usize]=match result {Ok(())=>Connection::Configured, Err(e)=>Connection::Failed(e)};
                }
                self.worker=None;
            }
        }
        for p in Provider::ALL {
            if !enabled[p as usize] {self.states[p as usize]=Connection::Off;}
        }
        if self.worker.is_none() {
            if let Some(p)=Provider::ALL.into_iter().find(|&p|enabled[p as usize] && matches!(self.state(p),Connection::Off)) {
                self.states[p as usize]=Connection::Installing;
                self.worker=Some((p,start(p)));
            }
        }
    }
}

pub(crate) fn node_runtime() -> Option<PathBuf> {
    let name=if cfg!(windows) {"node.exe"} else {"node"};
    let mut dirs=Vec::new();
    if let Some(root)=std::env::var_os("ProgramFiles") { dirs.push(PathBuf::from(root).join("nodejs")); }
    if let Some(paths)=std::env::var_os("PATH") { dirs.extend(std::env::split_paths(&paths)); }
    if let Some(root)=std::env::var_os("LOCALAPPDATA") {
        dirs.push(PathBuf::from(root).join("Programs/DeepSeek Harness/resources/runtime/primary-runtime/dependencies/node/bin"));
    }
    dirs.into_iter().filter(|dir|dir.is_absolute()).map(|dir|dir.join(name)).find(|p|p.is_file())
}

fn install_default(provider:Provider)->SetupResult {
    let node=node_runtime().ok_or("找不到 Node.js，请安装 Node.js 18 或更高版本后重试")?;
    let exe=std::env::current_exe().map_err(|_|"无法定位岛仔程序")?;
    let dir=crate::model::config_dir().join("agent-notices");
    export(&dir,&exe).map_err(|_|"无法写入岛仔接入文件，请检查目录权限")?;
    let mut cmd=std::process::Command::new(node);
    cmd.arg(dir.join("connect.mjs")).args([provider.id(),"install"]);
    let mut child=crate::child_process::spawn(&mut cmd).map_err(|_|"无法启动接入程序，请检查 Node.js")?;
    let started=std::time::Instant::now();
    loop {
        if let Some(status)=child.try_wait().map_err(|_|"无法确认接入结果，可以重试")? {
            return if status.success(){Ok(())}else{Err(match status.code(){
                Some(3)=>"未找到桌面配置，请先启动一次 DeepSeek Harness",
                Some(4)=>"接入配置被其他插件占用，已保留原配置",
                _=>"未能接入，请检查客户端配置和目录权限后重试",
            })};
        }
        if started.elapsed()>std::time::Duration::from_secs(15) {
            let _=child.kill();let _=child.wait();return Err("接入超时，可以重试");
        }
        std::thread::sleep(std::time::Duration::from_millis(40));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn agent_notice_reconciles_once_retries_on_demand_and_preserves_delivery() {
        let mut setup=Setup::default();
        let (tx,rx)=mpsc::channel();
        setup.reconcile([false,false], |_|panic!("disabled"));
        setup.reconcile([true,true], |p|{assert_eq!(p,Provider::Dsh);rx});
        setup.reconcile([true,true], |_|panic!("already running"));
        tx.send(Err("failed")).unwrap();
        let (tx,rx)=mpsc::channel();
        setup.reconcile([true,true], |p|{assert_eq!(p,Provider::Codex);rx});
        assert!(matches!(setup.state(Provider::Dsh),Connection::Failed(_)));
        setup.received(Provider::Codex);
        tx.send(Ok(())).unwrap();
        setup.reconcile([true,true], |_|panic!("do not retry every frame"));
        assert!(matches!(setup.state(Provider::Codex),Connection::Seen));
        setup.retry(Provider::Dsh);
        let (tx,rx)=mpsc::channel();
        setup.reconcile([true,true], |_|rx);
        setup.reconcile([false,true], |_|panic!("muted"));
        tx.send(Ok(())).unwrap();
        setup.reconcile([false,true], |_|panic!("late completion"));
        assert!(matches!(setup.state(Provider::Dsh),Connection::Off));
    }

    #[test]
    fn agent_notice_automatic_setup_uses_only_the_enabled_profile() {
        const PROBE:&str="DAOZAI_AUTO_SETUP_TEST_CHILD";
        if let Some(root)=std::env::var_os(PROBE) {
            let root=PathBuf::from(root);
            let profile=root.join("dsh/profiles/desktop");fs::create_dir_all(&profile).unwrap();
            fs::write(profile.join("package.json"),r#"{"private":true}"#).unwrap();
            let mut settings=crate::model::Settings::default();
            let mut setup=Setup::default();
            let ctx=egui::Context::default();
            let wait=|setup:&mut Setup,s:&crate::model::Settings,p| {
                let now=std::time::Instant::now();
                loop {
                    setup.update(s,&ctx);
                    match setup.state(p) {
                        Connection::Configured=>break,
                        Connection::Failed(e)=>panic!("{e}"),
                        _=>assert!(now.elapsed().as_secs()<20),
                    }
                    std::thread::sleep(std::time::Duration::from_millis(30));
                }
            };
            settings.dsh_notifications=true;
            wait(&mut setup,&settings,Provider::Dsh);
            assert!(profile.join("node_modules/@daozai/agent-notices/index.mjs").is_file());
            let hooks=root.join("codex/hooks.json");
            assert!(!hooks.exists());
            settings.codex_notifications=true;
            wait(&mut setup,&settings,Provider::Codex);
            assert!(fs::read_to_string(hooks).unwrap().contains("Daozai"));
            return;
        }
        let root=std::env::temp_dir().join(format!("daozai-agent-auto-{}-{}",std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        let result=std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact","agent_notice::tests::agent_notice_automatic_setup_uses_only_the_enabled_profile","--nocapture"])
            .env(PROBE,&root).env("DSH_HOME",root.join("dsh")).env("CODEX_HOME",root.join("codex"))
            .env("DAOZAI_DATA_DIR",root.join("data")).status().unwrap();
        fs::remove_dir_all(root).unwrap();
        assert!(result.success());
    }

    #[test]
    fn agent_notice_site_table_order_matches_wire_signals() {
        assert_eq!(SITES.map(|meta|meta.provider),Provider::ALL);
        let mut order=Vec::new();
        for (_,_,Notice(provider,_)) in SIGNALS {
            if !order.contains(&provider) { order.push(provider); }
        }
        assert_eq!(order,SITES.iter().map(|meta|meta.provider).collect::<Vec<_>>());
        let mut first:Vec<(&'static str,Provider)>=Vec::new();
        for (key,_,Notice(provider,_)) in SIGNALS {
            if !first.iter().any(|(_,known)|*known==provider) { first.push((key,provider)); }
        }
        assert_eq!(first,SITES.iter().map(|meta|(meta.key,meta.provider)).collect::<Vec<_>>());
    }

    #[test]
    fn agent_notice_site_keys_decode_to_their_own_provider() {
        for meta in SITES.iter() {
            assert_eq!(meta.provider.id(),meta.key);
            assert_eq!(meta.provider.label(),meta.label);
            for status in ["finished","approval","question"] {
                let expected=match status {"finished"=>Status::Finished,"approval"=>Status::Approval,_=>Status::Question};
                let id=wire_id(meta.key,status).unwrap_or_else(||panic!("{}/{status} 不在 wire 表中",meta.key));
                let Notice(provider,decoded)=decode(id).unwrap();
                assert_eq!(provider,meta.provider);
                assert_eq!(decoded,expected);
                assert_eq!(decoded.label(),expected.label());
            }
        }
    }

    #[test]
    fn agent_notice_site_setting_switch_round_trips() {
        let mut settings=crate::model::Settings::default();
        for meta in SITES.iter() {
            assert!(!(meta.setting)(&settings));
            assert!(!meta.provider.enabled(&settings));
            *(meta.setting_mut)(&mut settings)=true;
            assert!((meta.setting)(&settings));
            assert!(settings.notifications && meta.provider.enabled(&settings));
            assert!(*meta.provider.setting_mut(&mut settings));
            settings.notifications=false;
            assert!(!meta.provider.enabled(&settings));
            settings.notifications=true;
            assert!(meta.provider.enabled(&settings));
        }
        let mut fresh=crate::model::Settings::default();
        *Provider::Dsh.setting_mut(&mut fresh)=true;
        assert!(!*Provider::Codex.setting_mut(&mut fresh));
        assert!(fresh.dsh_notifications && !fresh.codex_notifications);
    }

    #[test]
    fn agent_notice_unknown_wire_ids_stay_unmapped() {
        assert_eq!(wire_id("dsh","unknown"),None);
        assert_eq!(wire_id("unknown","finished"),None);
        assert_eq!(decode(0),None);
        assert_eq!(decode(usize::MAX),None);
        assert_eq!(decode(SIGNALS.len()),Some(Notice(Provider::Codex,Status::Question)));
        assert_eq!(decode(SIGNALS.len()+1),None);
    }
}
