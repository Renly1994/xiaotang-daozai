//! 配色、字体与字号。
//!
//! 字体：拉丁/数字用 Segoe UI（Windows 原生 UI 字体，与原版 DirectWrite 渲染同源），
//! 中文回退到微软雅黑 UI，韩文回退到 Malgun Gothic。三档字重：常规 / 半粗 / 粗。
//! 非 Windows（开发机、录屏）按 `DAOZAI_FONT_DIR` → 常见系统路径依次查找
//! 度量兼容的 Selawik 与 Noto Sans SC。

use eframe::egui::{self, Color32, FontData, FontDefinitions, FontFamily, FontId};
use std::path::{Path, PathBuf};
use std::sync::Arc;

// ---------- 颜色（原版逐像素取色，少量为审美微调） ----------
pub const RED: Color32 = Color32::from_rgb(242, 48, 70);
pub const RED_TOP: Color32 = Color32::from_rgb(253, 104, 92);
pub const BLUE: Color32 = Color32::from_rgb(62, 172, 248);
pub const BLUE_DEEP: Color32 = Color32::from_rgb(13, 69, 128);
pub const CYAN: Color32 = Color32::from_rgb(102, 219, 255);
pub const SOFT_GREEN: Color32 = Color32::from_rgb(118, 196, 151);
pub const GREEN: Color32 = Color32::from_rgb(48, 209, 88);
/// 健康页「久坐」模式的主色（与喝水的蓝成对）。
pub const SIT_GREEN: Color32 = Color32::from_rgb(70, 200, 125);
pub const CHARGE: Color32 = Color32::from_rgb(33, 235, 84);
pub const WHITE: Color32 = Color32::WHITE;
pub const GRAY: Color32 = Color32::from_rgb(178, 178, 186);
pub const DIM: Color32 = Color32::from_rgb(142, 142, 153);
pub const BTN: Color32 = Color32::from_rgb(40, 40, 43);
pub const BTN_HOVER: Color32 = Color32::from_rgb(56, 56, 60);
pub const CARD: Color32 = Color32::from_rgb(22, 22, 24);
pub const TRACK: Color32 = Color32::from_rgb(36, 36, 39);
pub const ARROW_BG: Color32 = Color32::from_rgb(48, 48, 51);
pub const WATER_TRACK: Color32 = Color32::from_rgb(15, 38, 64);
pub const YELLOW: Color32 = Color32::from_rgb(255, 196, 61);

// ---------- Agent 卡片语义色（对齐 island-hud 的 alert / idle / work 体系） ----------
/// 等待用户处理的警示橙：审批、提问、倒计时。
pub const AGENT_ALERT: Color32 = Color32::from_rgb(240, 151, 72);
/// 完成、成功、已选中。
pub const AGENT_OK: Color32 = Color32::from_rgb(52, 199, 89);
/// 运行中 / 可跳转。
pub const AGENT_WORK: Color32 = Color32::from_rgb(78, 128, 237);
/// 次级文字（正文下一档）。
pub const AGENT_MUTED: Color32 = Color32::from_rgb(179, 179, 179);
/// 三级文字（标签、元信息）。
pub const AGENT_SUBTLE: Color32 = Color32::from_rgb(150, 145, 142);
/// 卡片内嵌块（代码块、选项）的底色。
pub const AGENT_INSET: Color32 = Color32::from_rgb(24, 22, 20);
/// 内嵌块描边。
pub const AGENT_INSET_EDGE: Color32 = Color32::from_rgb(52, 46, 39);
/// 主按钮（反白）与其次级按钮。
pub const AGENT_PRIMARY: Color32 = Color32::from_rgb(232, 232, 234);
pub const AGENT_PRIMARY_HOVER: Color32 = Color32::from_rgb(255, 255, 255);
pub const AGENT_SECONDARY: Color32 = Color32::from_rgb(38, 38, 40);
pub const AGENT_SECONDARY_HOVER: Color32 = Color32::from_rgb(56, 56, 60);
/// 禁用态整体透明度。
pub const AGENT_DISABLED: f32 = 0.42;

pub fn alpha_blue(a: f32) -> Color32 {
    Color32::from_rgba_unmultiplied(BLUE.r(), BLUE.g(), BLUE.b(), (255.0 * a) as u8)
}

/// 字重档位。
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum W {
    Regular,
    Semibold,
    Bold,
}

pub fn font(size: f32, w: W) -> FontId {
    match w {
        W::Regular => FontId::proportional(size),
        W::Semibold => FontId::new(size, FontFamily::Name("semibold".into())),
        W::Bold => FontId::new(size, FontFamily::Name("bold".into())),
    }
}

/// 等宽字体：命令行、代码块、倒计时、队列计数与数字列。
/// 用等宽字体保证逐帧刷新时数字宽度不跳（等价于 CSS 的 `tabular-nums`）。
pub fn mono(size: f32, w: W) -> FontId {
    match w {
        W::Regular => FontId::new(size, FontFamily::Name("mono-regular".into())),
        W::Semibold => FontId::new(size, FontFamily::Name("mono-semibold".into())),
        W::Bold => FontId::new(size, FontFamily::Name("mono-bold".into())),
    }
}

/// 字号表（逻辑点）。汉字天然比拉丁字母「显大」，中文字号单独一套，
/// 让两种文字的视觉重量一致，而不是硬把墨高压到拉丁大写高度。
pub mod size {
    pub const TITLE: f32 = 14.5; // 页标题（中文）
    pub const DATE: f32 = 11.0; // 顶栏日期
    pub const CLOCK: f32 = 12.5; // 顶栏时钟
    pub const PILL_CLOCK: f32 = 15.5; // 药丸时钟
    pub const HERO: f32 = 29.0; // 大数字（25:00 / 0.1 / 0/0）
    pub const BODY: f32 = 11.5; // 正文、按钮
    pub const SMALL: f32 = 11.0; // 说明
    pub const TINY: f32 = 10.0; // 脚注、坐标轴
}

struct Face {
    path: PathBuf,
    index: u32,
}

fn find(cands: &[(&str, u32)], dirs: &[PathBuf]) -> Option<Face> {
    for (name, index) in cands {
        let p = Path::new(name);
        if p.is_absolute() && p.exists() {
            return Some(Face { path: p.to_path_buf(), index: *index });
        }
        for d in dirs {
            let q = d.join(name);
            if q.exists() {
                return Some(Face { path: q, index: *index });
            }
        }
    }
    None
}

fn font_dirs() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Ok(d) = std::env::var("DAOZAI_FONT_DIR") {
        v.push(PathBuf::from(d));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(d) = exe.parent() {
            v.push(d.join("fonts"));
        }
    }
    v.push(PathBuf::from("assets/fonts"));
    let windir = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into());
    v.push(PathBuf::from(windir).join("Fonts"));
    for d in [
        "/usr/share/fonts/google-noto-cjk",
        "/usr/share/fonts/opentype/noto",
        "/usr/share/fonts/noto-cjk",
        "/usr/share/fonts/truetype",
    ] {
        v.push(PathBuf::from(d));
    }
    v
}

/// 装载三档字重。每档 = 拉丁字体 + 中文回退 + 韩文回退；缺哪个就跳过，最后兜底 egui 内置字体。
pub fn load_fonts(ctx: &egui::Context) {
    ctx.set_fonts(shared_fonts().clone());
}

/// 测试导出「修复前」对照图：去掉藏文字体后的字体表。
#[cfg(test)]
pub(crate) fn fonts_without_tibetan()->FontDefinitions {
    let mut fonts=shared_fonts().clone();
    for list in fonts.families.values_mut() {list.retain(|key|key!="tibetan");}
    fonts
}

fn shared_fonts()->&'static FontDefinitions {
    static FONTS:std::sync::OnceLock<FontDefinitions>=std::sync::OnceLock::new();
    FONTS.get_or_init(build_fonts)
}

#[cfg(windows)]
struct MappedFont {
    _source:std::fs::File,
    bytes:memmap2::Mmap,
}

#[cfg(windows)]
impl MappedFont {
    fn open(path:&Path)->std::io::Result<Self>{
        use std::os::windows::fs::OpenOptionsExt;
        // 保留文件句柄并禁止原地写入；允许系统以替换文件的方式更新字体。
        let source=std::fs::OpenOptions::new().read(true).share_mode(0x1|0x4).open(path)?;
        let bytes=unsafe{memmap2::Mmap::map(&source)?};
        Ok(Self{_source:source,bytes})
    }
}

fn font_bytes(path:&Path)->Option<&'static [u8]>{
    #[cfg(windows)]
    if let Ok(mapped)=MappedFont::open(path){
        // 所有字体由进程级 OnceLock 持有，映射和只读句柄与排版器同寿命。
        let mapped=Box::leak(Box::new(mapped));return Some(&mapped.bytes);
    }
    Some(Box::leak(std::fs::read(path).ok()?.into_boxed_slice()))
}

/// 微软喜马拉雅体的字形比同字号的中文小一圈，放大后藏文与中文、英文大致等高。
const TIBETAN_SCALE: f32 = 1.35;

fn build_fonts()->FontDefinitions {
    let dirs = font_dirs();
    let latin = [
        find(&[("segoeui.ttf", 0), ("selawk.ttf", 0)], &dirs),
        find(&[("seguisb.ttf", 0), ("selawksb.ttf", 0), ("segoeuib.ttf", 0)], &dirs),
        find(&[("segoeuib.ttf", 0), ("selawkb.ttf", 0)], &dirs),
    ];
    let cjk = [
        find(&[("msyh.ttc", 1), ("NotoSansSC-Regular.otf", 0), ("NotoSansCJK-Regular.ttc", 2)], &dirs),
        find(&[("msyhbd.ttc", 1), ("NotoSansSC-Medium.otf", 0), ("NotoSansCJK-Medium.ttc", 2)], &dirs),
        find(&[("msyhbd.ttc", 1), ("NotoSansSC-Bold.otf", 0), ("NotoSansCJK-Bold.ttc", 2)], &dirs),
    ];

    let korean = [
        find(&[("malgun.ttf", 0), ("NotoSansKR-Regular.otf", 0), ("NotoSansCJK-Regular.ttc", 1)], &dirs),
        find(&[("malgunbd.ttf", 0), ("malgun.ttf", 0), ("NotoSansKR-Bold.otf", 0), ("NotoSansCJK-Bold.ttc", 1)], &dirs),
    ];

    // 藏文：前面几种字体都没有藏文字形（#200）。微软喜马拉雅体的下加字、元音符号本身不占宽度、叠在前一个字上，
    // 不做复杂排版也能拼出常见的叠字；它的字形偏小，放大到与中文大致等高。只有一档字重，三档共用。
    let tibetan = find(&[("himalaya.ttf", 0), ("NotoSerifTibetan-Regular.ttf", 0), ("NotoSansTibetan-Regular.ttf", 0)], &dirs);

    let mut fonts = FontDefinitions::default();
    let mut cache: Vec<(PathBuf, u32, String)> = Vec::new();
    let mut add = |fonts: &mut FontDefinitions, face: &Option<Face>, key: String| -> Option<String> {
        let face = face.as_ref()?;
        if let Some((_, _, k)) = cache.iter().find(|(p, i, _)| *p == face.path && *i == face.index) {
            return Some(k.clone());
        }
        let mut d = FontData::from_static(font_bytes(&face.path)?);
        d.index = face.index;
        fonts.font_data.insert(key.clone(), Arc::new(d));
        cache.push((face.path.clone(), face.index, key.clone()));
        Some(key)
    };

    let builtin = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    let families = [
        FontFamily::Proportional,
        FontFamily::Name("semibold".into()),
        FontFamily::Name("bold".into()),
    ];
    for (i, fam) in families.iter().enumerate() {
        let mut list = Vec::new();
        if let Some(k) = add(&mut fonts, &latin[i], format!("latin{i}")) {
            list.push(k);
        }
        if let Some(k) = add(&mut fonts, &cjk[i], format!("cjk{i}")) {
            list.push(k);
        }
        if let Some(k) = add(&mut fonts, &korean[usize::from(i > 0)], format!("korean{i}")) {
            list.push(k);
        }
        if let Some(k) = add(&mut fonts, &tibetan, "tibetan".into()) {
            if let Some(data) = fonts.font_data.get_mut(&k) {
                if data.tweak.scale == 1.0 { Arc::make_mut(data).tweak.scale = TIBETAN_SCALE; }
            }
            list.push(k);
        }
        list.extend(builtin.iter().cloned());
        fonts.families.insert(fam.clone(), list);
    }
    // 等宽家族：Cascadia Mono（Windows Terminal 自带）→ Consolas → Noto Sans Mono。
    // 注意：不改写内置的 FontFamily::Monospace —— 设置页等既有 UI 依赖它的默认度量，
    // 覆盖会让分段控件的文字垂直居中偏移 1.25px（有回归测试盯着）。这里只注册命名家族。
    let mono = [
        find(&[("CascadiaMono.ttf", 0), ("consola.ttf", 0), ("NotoSansMono-Regular.ttf", 0)], &dirs),
        find(&[("CascadiaMono.ttf", 0), ("consolab.ttf", 0), ("consola.ttf", 0)], &dirs),
        find(&[("consolab.ttf", 0), ("CascadiaMono.ttf", 0), ("consola.ttf", 0)], &dirs),
    ];
    let mono_families = [
        FontFamily::Name("mono-regular".into()),
        FontFamily::Name("mono-semibold".into()),
        FontFamily::Name("mono-bold".into()),
    ];
    for (i, fam) in mono_families.iter().enumerate() {
        let mut list = Vec::new();
        if let Some(k) = add(&mut fonts, &mono[i], format!("mono{i}")) {
            list.push(k);
        }
        if let Some(k) = add(&mut fonts, &cjk[0], "cjk0".into()) {
            list.push(k);
        }
        list.extend(builtin.iter().cloned());
        fonts.families.insert(fam.clone(), list);
    }    fonts
}

/// 各个原生窗口统一使用岛仔品牌图标，避免回落到框架默认图标。
pub fn app_icon()->std::sync::Arc<eframe::egui::IconData>{
    static ICON:std::sync::OnceLock<std::sync::Arc<eframe::egui::IconData>>=std::sync::OnceLock::new();
    ICON.get_or_init(||{
        let image=image::load_from_memory(include_bytes!("../assets/daozai-logo.png")).expect("岛仔图标资源应有效").thumbnail(128,128).to_rgba8();
        std::sync::Arc::new(eframe::egui::IconData{width:image.width(),height:image.height(),rgba:image.into_raw()})
    }).clone()
}

#[cfg(test)]
mod tests {
    #[cfg(windows)]
    #[test]
    fn korean_titles_and_lyrics_have_glyphs_in_all_weights(){
        let ctx=eframe::egui::Context::default();
        super::load_fonts(&ctx);
        let _=ctx.run(Default::default(),|ctx|{
            for weight in [super::W::Regular,super::W::Semibold,super::W::Bold]{
                let font=super::font(14.0,weight);
                for ch in "눈, 코, 입 향기로운 숨을 담은 너의 코 사랑해 眼鼻嘴 TAEYANG ㄱㄴㅏ".chars().filter(|c|!c.is_whitespace()){
                    assert!(ctx.fonts(|f|f.has_glyph(&font,ch)),"缺少字形：{ch}，字体：{font:?}");
                }
            }
        });
    }

    /// #200：维语歌词转换后用到的连写字形、藏文歌词，在三档字重下都要有字形（不能显示成方框）。
    #[cfg(windows)]
    #[test]
    fn uyghur_and_tibetan_lyrics_have_glyphs_in_all_weights(){
        let ctx=eframe::egui::Context::default();
        super::load_fonts(&ctx);
        // 覆盖维语全部字母：每个字母在词首、词中、词尾、独立各出现一次。
        let uyghur="سوي گۈ دىگەن شۇنداق بولامدۇ يايرىم سېنى سېغىندىم شۇنچە ئا ئە ب پ ت ج چ خ د ر ز ژ س ش غ ف ق ك گ ڭ ل م ن ھ و ۇ ۆ ۈ ۋ ې ى ي ئ لا بلا بەبە بۆبۆ بڭب بھب بېب بىب";
        let shown=crate::rtl_text::display(uyghur).into_owned();
        let tibetan="བཀྲ་ཤིས་བདེ་ལེགས། སྐྱིད་པོ། གྲོགས་པོ། ཁྱེད་རང་།";
        let _=ctx.run(Default::default(),|ctx|{
            for weight in [super::W::Regular,super::W::Semibold,super::W::Bold]{
                let font=super::font(14.0,weight);
                for ch in shown.chars().chain(tibetan.chars()).filter(|c|!c.is_whitespace()){
                    assert!(ctx.fonts(|f|f.has_glyph(&font,ch)),"缺少字形：U+{:04X}，字体：{font:?}",ch as u32);
                }
            }
        });
    }

    #[cfg(windows)]
    #[test]
    fn mapped_fonts_prevent_in_place_writes_and_keep_shared_reads(){
        let path=std::env::temp_dir().join(format!("daozai-font-map-{}-{}.bin",std::process::id(),std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::write(&path,b"readonly font fixture").unwrap();
        let mapped=super::MappedFont::open(&path).unwrap();
        assert_eq!(&mapped.bytes[..],b"readonly font fixture");
        assert_eq!(std::fs::read(&path).unwrap(),b"readonly font fixture");
        assert!(std::fs::OpenOptions::new().write(true).open(&path).is_err(),"映射存活期间不能允许原地改写字体");
        drop(mapped);std::fs::remove_file(path).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn system_font_bytes_use_read_only_file_mapping(){
        #[link(name="kernel32")]
        unsafe extern "system"{
            fn K32GetMappedFileNameW(process:*mut core::ffi::c_void,address:*const core::ffi::c_void,buffer:*mut u16,size:u32)->u32;
        }
        let fonts=super::shared_fonts();let font=&fonts.font_data["cjk0"];
        let mut name=[0u16;1024];
        let count=unsafe{K32GetMappedFileNameW(windows::Win32::System::Threading::GetCurrentProcess().0,font.font.as_ptr().cast(),name.as_mut_ptr(),name.len()as u32)};
        assert!(count>0,"中文字体仍占用完整的私有堆副本，没有复用系统字体文件页");
        let name=String::from_utf16_lossy(&name[..count as usize]).to_lowercase();
        assert!([".ttc",".ttf",".otf"].iter().any(|suffix|name.ends_with(suffix)),"映射必须来自实际字体文件");
    }

    #[test]
    fn font_data_is_shared_across_contexts_without_owned_byte_copies(){
        let fonts=super::shared_fonts();
        assert!(fonts.font_data.values().all(|font|matches!(font.font,std::borrow::Cow::Borrowed(_))),"字体不能在每个排版上下文中产生完整副本");
        let cloned=fonts.clone();
        for (name,font) in &fonts.font_data {assert!(std::sync::Arc::ptr_eq(font,&cloned.font_data[name]));}
    }
}
