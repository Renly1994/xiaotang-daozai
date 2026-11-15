//! 邮件正文里的图片：内嵌图（`cid:`）随邮件下发，远程图按需下载。
//!
//! 默认自动加载远程图（用户明确要求「打开邮件就能看到图」），设置里可关掉。
//! 追踪像素不产生任何可见占位，避免把邮件正文撑成一排排小方框。

use crate::mail_doc::{Align,Image};
use eframe::egui::{self,Color32};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool,Ordering};
use std::sync::{Arc,Mutex};

/// 单张图片上限；超过就只显示占位，避免一张大图把内存吃掉。
const MAX_BYTES: usize = 4 * 1024 * 1024;
/// 一次会话里所有图片的总预算。
const MAX_TOTAL: usize = 24 * 1024 * 1024;

enum Entry {
    Loading,
    Ready(egui::TextureHandle,[usize;2]),
    /// 记录失败时刻：网络抖动后过一段时间自动重试，不把一次失败钉死。
    Failed(std::time::Instant),
}

/// 失败后多久允许重试。
const RETRY_AFTER: std::time::Duration = std::time::Duration::from_secs(20);

#[derive(Default)]
struct Inner {
    entries: HashMap<String,Entry>,
    bytes: usize,
}

pub struct ImageCache {
    inner: Arc<Mutex<Inner>>,
    /// 用户是否为这次阅读放开了远程图片
    remote: Arc<AtomicBool>,
    ctx: Arc<Mutex<Option<egui::Context>>>,
}

impl Default for ImageCache {
    fn default() -> Self {
        Self {
            inner: Arc::default(),
            // 默认放行：打开邮件即加载远程图。用户可在设置里关闭（防追踪）。
            remote: Arc::new(AtomicBool::new(true)),
            ctx: Arc::new(Mutex::new(None)),
        }
    }
}

impl ImageCache {
    pub fn remote_enabled(&self) -> bool {
        self.remote.load(Ordering::Relaxed)
    }

    /// 开关远程图片；关掉时清空已下载的远程图，避免继续占内存。
    pub fn enable_remote(&self,on:bool){
        self.remote.store(on,Ordering::Relaxed);
        if !on {
            if let Ok(mut inner)=self.inner.lock(){
                inner.entries.retain(|key,_|!key.starts_with("http"));
            }
        }
    }

    /// 新邮件打开时重置远程开关：按「自动显示远程图片」设置决定，
    /// 同时清掉上一封已下载的远程图，避免缓存无限增长。
    pub fn reset(&self, auto: bool){
        self.remote.store(auto, Ordering::Relaxed);
        if let Ok(mut inner) = self.inner.lock() {
            inner.entries.retain(|key, _| !key.starts_with("http"));
            inner.bytes = inner.entries.values().map(|e| match e {
                Entry::Ready(_, size) => size[0] * size[1] * 4,
                _ => 0,
            }).sum();
        }
    }

    pub fn show(&self,ui:&mut egui::Ui,image:&Image,max_width:f32,scale:f32,embedded:&HashMap<String,Arc<Vec<u8>>>){
        if let Ok(mut slot)=self.ctx.lock(){
            if slot.is_none(){*slot=Some(ui.ctx().clone());}
        }
        // 邮件图常按原始像素（1000+）给，正文宽度约束又可能被 ScrollArea 放大，
        // 于是单张图就能撑满整个窗口。取「可用宽度」与「可视区宽度」的较小值，
        // 再兜一个 80px 下限，避免极窄容器把图压成一条线。
        let width=max_width.min(ui.clip_rect().width()).max(80.0);
        match self.lookup(&image.src,embedded,ui.ctx()){
            Entry::Ready(texture,size)=>{
                self.draw(ui,image,&texture,size,width,scale);
            }
            Entry::Loading=>{
                self.placeholder(ui,image,"正在加载图片…",width);
            }
            Entry::Failed(_)=>{
                let remote=image.src.starts_with("http://")||image.src.starts_with("https://");
                // 追踪像素通常没有 alt，加载失败时不该再画一排小方框污染正文。
                if image.alt.trim().is_empty(){
                    return;
                }
                let note=if remote&&!self.remote_enabled(){
                    "远程图片已隐藏"
                }else{
                    "图片无法显示"
                };
                self.placeholder(ui,image,note,width);
            }
        }
    }

    fn lookup(&self,src:&str,embedded:&HashMap<String,Arc<Vec<u8>>>,ctx:&egui::Context)->Entry{
        if let Ok(mut inner)=self.inner.lock(){
            let expired=matches!(inner.entries.get(src),Some(Entry::Failed(at)) if at.elapsed()>=RETRY_AFTER);
            if expired{inner.entries.remove(src);}
            if let Some(entry)=inner.entries.get(src){
                return match entry{
                    Entry::Ready(texture,size)=>Entry::Ready(texture.clone(),*size),
                    Entry::Loading=>Entry::Loading,
                    Entry::Failed(at)=>Entry::Failed(*at),
                };
            }
        }
        // 内嵌图直接解码，不联网。
        if let Some(cid)=src.strip_prefix("cid:"){
            let key=normalize_cid(cid);
            let bytes=embedded.get(&key).or_else(||embedded.get(cid));
            return match bytes.and_then(|data|decode(data)){
                Some((color,size))=>{
                    if size[0]*size[1]*4>MAX_BYTES{return self.fail(src);}
                    let texture=ctx.load_texture(src,color,egui::TextureOptions::LINEAR);
                    self.store(src,texture,size)
                }
                None=>self.fail(src),
            };
        }
        // 协议相对 URL 在 HTML 里很常见，补成 https 再下载。
        let url=if src.starts_with("//"){format!("https:{src}")}else{src.to_string()};
        if url.starts_with("http://")||url.starts_with("https://"){
            if !self.remote_enabled(){return Entry::Failed(std::time::Instant::now());}
            self.spawn_remote(src,&url,ctx);
            Entry::Loading
        }else{
            Entry::Failed(std::time::Instant::now())
        }
    }

    fn fail(&self,src:&str)->Entry{
        let now=std::time::Instant::now();
        if let Ok(mut inner)=self.inner.lock(){inner.entries.insert(src.to_string(),Entry::Failed(now));}
        Entry::Failed(now)
    }

    fn store(&self,src:&str,texture:egui::TextureHandle,size:[usize;2])->Entry{
        if let Ok(mut inner)=self.inner.lock(){
            inner.bytes=inner.bytes.saturating_add(size[0]*size[1]*4);
            inner.entries.insert(src.to_string(),Entry::Ready(texture.clone(),size));
        }
        Entry::Ready(texture,size)
    }

    fn spawn_remote(&self,key:&str,url:&str,ctx:&egui::Context){
        let (key,url)=(key.to_string(),url.to_string());
        {
            let Ok(mut inner)=self.inner.lock()else{return};
            if inner.entries.contains_key(&key){return;}
            inner.entries.insert(key.clone(),Entry::Loading);
        }
        let inner=self.inner.clone();
        let ctx=ctx.clone();
        std::thread::spawn(move||{
            let decoded=fetch_and_decode(&url);
            {
                let mut guard=match inner.lock(){Ok(g)=>g,Err(_)=>return};
                match decoded{
                    Some((bytes,color,size))=>{
                        if guard.bytes+bytes>MAX_TOTAL{
                            guard.entries.insert(key.clone(),Entry::Failed(std::time::Instant::now()));
                        }else{
                            let texture=ctx.load_texture(&url,color,egui::TextureOptions::LINEAR);
                            guard.bytes+=bytes;
                            guard.entries.insert(key.clone(),Entry::Ready(texture,size));
                        }
                    }
                    None=>{guard.entries.insert(key.clone(),Entry::Failed(std::time::Instant::now()));}
                }
            }
            // 下载在后台线程完成，必须主动请求重绘，否则「正在加载…」可能一直挂着
            // 直到下一次定时器刷新，看起来就像远程图片没加载出来。
            ctx.request_repaint();
        });
    }

    fn draw(&self,ui:&mut egui::Ui,image:&Image,texture:&egui::TextureHandle,size:[usize;2],max_width:f32,scale:f32){
        let (mut w,mut h)=(size[0] as f32,size[1] as f32);
        if let Some(width)=image.width{
            h*=(width/w).max(0.01);
            w=width;
        }
        if let Some(height)=image.height{
            w*=(height/h).max(0.01);
            h=height;
        }
        // 邮件里的图经常写成固定像素宽，窗口变窄时等比缩回去。
        if w>max_width{
            h*=max_width/w;
            w=max_width;
        }
        // 竖图（表情包、商品长图）同样不能让一张图占满整屏：超过可视高度就等比缩。
        let max_height=(ui.clip_rect().height()).max(280.0);
        if h>max_height{
            w*=max_height/h;
            h=max_height;
        }
        if w<1.0||h<1.0{return;}
        let (w,h)=(w*scale,h*scale);
        let layout=match image.align{
            Align::Center=>egui::Layout::top_down(egui::Align::Center),
            Align::Right=>egui::Layout::top_down(egui::Align::Max),
            Align::Left=>egui::Layout::top_down(egui::Align::Min),
        };
        ui.with_layout(layout,|ui|{
            ui.add(egui::Image::new(texture).fit_to_exact_size(egui::vec2(w,h)).corner_radius(8.0));
        });
    }

    fn placeholder(&self,ui:&mut egui::Ui,image:&Image,note:&str,max_width:f32){
        let text=if !image.alt.is_empty(){
            format!("[图片] {}",image.alt)
        }else if !note.is_empty(){
            note.to_string()
        }else{
            String::new()
        };
        if text.is_empty(){return;}
        // 占位框别拉满整行：一张被拦下的远程图不该变成一条大灰条。
        let width=image.width.unwrap_or(max_width).min(max_width).min(260.0);
        let height=image.height.unwrap_or(30.0).clamp(26.0,44.0);
        let layout=match image.align{
            Align::Center=>egui::Layout::top_down(egui::Align::Center),
            Align::Right=>egui::Layout::top_down(egui::Align::Max),
            Align::Left=>egui::Layout::top_down(egui::Align::Min),
        };
        ui.with_layout(layout,|ui|{
            let (rect,_)=ui.allocate_exact_size(egui::vec2(width,height),egui::Sense::hover());
            ui.painter().rect_filled(rect,8.0,Color32::from_white_alpha(5));
            ui.painter().rect_stroke(rect,8.0,egui::Stroke::new(1.0,Color32::from_gray(58)),egui::StrokeKind::Inside);
            ui.painter().text(rect.center(),egui::Align2::CENTER_CENTER,&text,egui::FontId::proportional(11.5),Color32::from_gray(130));
        });
    }
}

/// `cid:` 在邮件里通常带尖括号，比如 `cid:<logo@example.com>`。
fn normalize_cid(raw:&str)->String{
    raw.trim().trim_start_matches('<').trim_end_matches('>').to_string()
}

fn decode(data:&[u8])->Option<(egui::ColorImage,[usize;2])>{
    let picture=image::load_from_memory(data).ok()?;
    let rgba=picture.to_rgba8();
    let (w,h)=rgba.dimensions();
    if w==0||h==0{return None;}
    let color=egui::ColorImage::from_rgba_unmultiplied([w as usize,h as usize],rgba.as_raw());
    Some((color,[w as usize,h as usize]))
}

fn fetch_and_decode(url:&str)->Option<(usize,egui::ColorImage,[usize;2])>{
    use std::io::Read;
    // 不少图床/CDN 会拒绝没有 User-Agent 的请求，直接返回 403。
    let response=ureq::get(url)
        .set("User-Agent","Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0 Safari/537.36")
        .set("Accept","image/avif,image/webp,image/apng,image/svg+xml,image/*,*/*;q=0.8")
        .timeout(std::time::Duration::from_secs(10))
        .call().ok()?;
    let mut data=Vec::new();
    response.into_reader().take(MAX_BYTES as u64+1).read_to_end(&mut data).ok()?;
    if data.len()>MAX_BYTES{return None;}
    let picture=image::load_from_memory(&data).ok()?;
    let rgba=picture.to_rgba8();
    let (w,h)=rgba.dimensions();
    if w==0||h==0{return None;}
    let bytes=(w as usize)*(h as usize)*4;
    let color=egui::ColorImage::from_rgba_unmultiplied([w as usize,h as usize],rgba.as_raw());
    Some((bytes,color,[w as usize,h as usize]))
}
