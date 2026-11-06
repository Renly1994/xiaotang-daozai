use eframe::egui;
use std::{
    collections::{HashMap, HashSet},
    io::Read,
    path::{Path, PathBuf},
    sync::{mpsc, Arc},
    time::{Duration, Instant},
};

enum Source {
    File(PathBuf),
    Url(String),
}

pub(crate) struct Images {
    requests: mpsc::Sender<(String, Option<egui::ColorImage>)>,
    completed: mpsc::Receiver<(String, Option<egui::ColorImage>)>,
    textures: HashMap<String, egui::TextureHandle>,
    pending: HashSet<String>,
    retry_after: HashMap<String, Instant>,
}

impl Default for Images {
    fn default() -> Self {
        let (requests, completed) = mpsc::channel();
        Self {
            requests,
            completed,
            textures: HashMap::new(),
            pending: HashSet::new(),
            retry_after: HashMap::new(),
        }
    }
}

impl Images {
    fn poll_completed(&mut self,ctx:&egui::Context){
        while let Ok((key, image)) = self.completed.try_recv() {
            self.pending.remove(&key);
            if let Some(image) = image {
                self.retry_after.remove(&key);
                let texture = ctx.load_texture(key.clone(), image, egui::TextureOptions::LINEAR);
                self.textures.insert(key, texture);
            } else {
                self.retry_after.insert(key, Instant::now() + Duration::from_secs(60));
            }
            ctx.request_repaint();
        }
    }

    pub(crate) fn prefetch(&mut self,ctx:&egui::Context,urls:&[String]){
        self.poll_completed(ctx);
        for url in urls {
            if self.pending.len()>=4 {break;}
            if self.textures.contains_key(url)||self.pending.contains(url)
                ||self.retry_after.get(url).is_some_and(|retry|Instant::now()<*retry){continue;}
            let _=self.get(ctx,None,Some(url));
        }
        if !self.pending.is_empty(){ctx.request_repaint_after(Duration::from_millis(100));}
    }

    pub(crate) fn get(
        &mut self,
        ctx: &egui::Context,
        file: Option<&Path>,
        url: Option<&str>,
    ) -> Option<egui::TextureId> {
        self.poll_completed(ctx);
        let source = if let Some(url) = url {
            Source::Url(url.to_owned())
        } else {
            Source::File(file?.to_owned())
        };
        let key = match &source {
            Source::File(path) => format!("file:{}", path.display()),
            Source::Url(url) => url.clone(),
        };
        if let Some(texture) = self.textures.get(&key) {
            return Some(texture.id());
        }
        if self.pending.contains(&key)
            || self
                .retry_after
                .get(&key)
                .is_some_and(|retry| Instant::now() < *retry)
        {
            return None;
        }
        self.pending.insert(key.clone());
        let sender = self.requests.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let image = load(&source);
            let _ = sender.send((key, image));
            ctx.request_repaint();
        });
        None
    }
}

fn load(source: &Source) -> Option<egui::ColorImage> {
    let (bytes, cache_path) = match source {
        Source::File(path) => (std::fs::read(path).ok()?, None),
        Source::Url(url) => {
            let parsed = url::Url::parse(url).ok()?;
            let host = parsed.host_str()?;
            if parsed.scheme() != "https"
                || !(matches!(
                    host,
                    "cdn.cloudflare.steamstatic.com"
                        | "steamcdn-a.akamaihd.net"
                        | "cdn.akamai.steamstatic.com"
                ) && (parsed
                    .path()
                    .starts_with("/steamcommunity/public/images/apps/")
                    || parsed
                        .path()
                        .starts_with("/steamcommunity/public/images/avatars/"))
                    || host == "avatars.steamstatic.com" && parsed.path().ends_with(".jpg"))
            {
                return None;
            }
            let digest = ring::digest::digest(&ring::digest::SHA256, url.as_bytes());
            let hash = digest
                .as_ref()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            let path = crate::model::config_dir()
                .join("steam-images")
                .join(format!("{hash}.jpg"));
            if let Ok(bytes) = std::fs::read(&path) {
                if image::load_from_memory(&bytes).is_ok() {
                    (bytes, None)
                } else {
                    let _ = std::fs::remove_file(&path);
                    (download_steam_image(&parsed)?, Some(path))
                }
            } else {
                (download_steam_image(&parsed)?, Some(path))
            }
        }
    };
    if bytes.is_empty() || bytes.len() > 2 * 1024 * 1024 {
        return None;
    }
    let decoded = image::load_from_memory(&bytes).ok()?.to_rgba8();
    let (width, height) = decoded.dimensions();
    if width == 0 || height == 0 || width > 1024 || height > 1024 {
        return None;
    }
    if let Some(path) = cache_path {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
            let _ = crate::model::atomic_write(&path, &bytes);
        }
    }
    Some(egui::ColorImage::from_rgba_unmultiplied(
        [width as usize, height as usize],
        decoded.as_raw(),
    ))
}

fn download_steam_image(url: &url::Url) -> Option<Vec<u8>> {
    let builder = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(3))
        .timeout_read(Duration::from_secs(3));
    #[cfg(windows)]
    let builder = builder.tls_connector(Arc::new(native_tls::TlsConnector::new().ok()?));
    let agent = builder.build();
    let mut hosts = vec![url.host_str()?.to_owned()];
    for host in if url
        .path()
        .starts_with("/steamcommunity/public/images/apps/")
    {
        &["steamcdn-a.akamaihd.net", "cdn.akamai.steamstatic.com"][..]
    } else {
        &[][..]
    } {
        if !hosts.iter().any(|found| found == *host) {
            hosts.push((*host).to_owned());
        }
    }
    for host in hosts {
        let mut candidate = url.clone();
        candidate.set_host(Some(&host)).ok()?;
        let response = agent.get(candidate.as_str()).call();
        let Ok(response) = response else { continue };
        let mut bytes = Vec::new();
        if response
            .into_reader()
            .take(2 * 1024 * 1024)
            .read_to_end(&mut bytes)
            .is_ok()
            && !bytes.is_empty()
            && image::load_from_memory(&bytes).is_ok()
        {
            return Some(bytes);
        }
    }
    None
}
