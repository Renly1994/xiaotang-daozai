//! 插件集市的目录读取、作者工具与包校验。

use super::{
    bounded,
    cover::{open_plain_file, COVER_LIMIT},
    devtools, fail, valid_id, CoverPng, Manifest, Package, Result, SourceInput, Values, COVER_FILE,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use ring::digest::{digest, SHA256};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    cmp::Ordering,
    collections::BTreeSet,
    ffi::OsString,
    fs::{self, File, OpenOptions},
    io::{self, Cursor, Read, Write},
    path::{Path, PathBuf},
    process::ExitCode,
    time::{Duration, Instant},
};
use url::Url;
use zip::{write::SimpleFileOptions, CompressionMethod, ZipArchive, ZipWriter};

const MANIFEST_LIMIT: u64 = 32 * 1024;
const ZIP_LIMIT: u64 = 832 * 1024;
const CANVAS_ZIP_LIMIT: u64 = ZIP_LIMIT + super::canvas_images::IMAGE_LIMIT + 32 * 1024;
const FONT_ZIP_LIMIT: u64 = super::font_assets::PACKAGE_LIMIT + 32 * 1024;
const CATALOG_LIMIT: u64 = 64 * 1024;
const AVATAR_LIMIT: u64 = 64 * 1024;
const CATALOG_FORMAT: u32 = 1;
const GITEE_REPOSITORY: &str = "xiaotangtoken/xiaotang-daozai-plugins";
pub(super) const IS_PREVIEW_BUILD: bool = option_env!("DAOZAI_TEST_BUILD").is_some();

fn official_index_path(channel: &str) -> String {
    format!("/api/v5/repos/{GITEE_REPOSITORY}/contents/{channel}/catalog.json")
}

#[cfg(not(test))]
pub(super) fn official_index_url() -> Result<Url> {
    let channel = if IS_PREVIEW_BUILD { "preview" } else { "stable" };
    Ok(Url::parse(&format!(
        "https://gitee.com{}?ref=master",
        official_index_path(channel)
    ))?)
}

fn allows_market_material(index: Option<&Url>, preview_build: bool) -> bool {
    let channel = if preview_build { "preview" } else { "stable" };
    index.is_none_or(|index| {
        is_https_host(index, "gitee.com")
            && index.query() == Some("ref=master")
            && index.path() == official_index_path(channel)
    })
}

pub(super) fn official_cache_path(dir: &Path, index: &Url) -> PathBuf {
    dir.join(format!(
        "official-market-{}.json",
        sha256(index.as_str().as_bytes())
    ))
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Entry {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) author: String,
    pub(super) version: String,
    pub(super) size: u64,
    pub(super) sha256: String,
    /// 设置归属，打包投稿时从清单的 owner 带入。早期条目没有这一项。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) owner: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) presentation: Option<Presentation>,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub(super) revoked: BTreeSet<String>,
}

impl Entry {
    fn matches(&self, manifest: &Manifest) -> bool {
        self.id == manifest.id
            && self.version == manifest.version
            && self.name == manifest.name
            && self.author == manifest.author
            && self.owner.as_deref().is_none_or(|owner| owner == manifest.owner)
    }

    /// 目录写明的设置归属。早期条目没有这一项，界面不猜测。
    pub(super) fn owner(&self) -> Option<&str> {
        self.owner.as_deref()
    }

    pub(super) fn revokes(&self, version: &str) -> bool {
        self.revoked.contains(version)
    }

    fn zip_name(&self) -> String {
        format!("plugin-{}-{}.zip", self.id, self.version)
    }

    fn path_in(&self, catalog: &Path) -> PathBuf {
        catalog.with_file_name(self.zip_name())
    }

    pub(super) fn avatar_name(&self) -> Option<&str> {
        self.presentation.as_ref()?.author_avatar.as_deref()
    }
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(super) struct Presentation {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) updated_on: Option<String>,
    #[serde(skip_serializing_if = "super::is_false")]
    pub(super) official: bool,
    #[serde(skip_serializing_if = "super::is_false")]
    pub(super) sample: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) maintainer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) author_avatar: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) downloads: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) favorites: Option<u64>,
}

impl Presentation {
    fn valid(&self) -> bool {
        self.description.as_deref().is_none_or(|description| {
            !description.trim().is_empty() && description.chars().count() <= 1000
        }) && self.updated_on.as_deref().is_none_or(|date| {
            chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d")
                .is_ok_and(|parsed| parsed.to_string() == date)
        }) && self.maintainer.as_deref().is_none_or(valid_github_login)
            && self.author_avatar.as_deref().is_none_or(valid_avatar_name)
    }
}

fn valid_github_login(login: &str) -> bool {
    (1..=39).contains(&login.len())
        && login.starts_with(|c: char| c.is_ascii_alphanumeric())
        && login.ends_with(|c: char| c.is_ascii_alphanumeric())
        && !login.contains("--")
        && login.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

fn valid_avatar_name(name: &str) -> bool {
    name.strip_prefix("avatar-")
        .and_then(|name| name.strip_suffix(".png"))
        .is_some_and(|stem| {
            (1..=64).contains(&stem.len())
                && stem
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CatalogFile {
    format: u32,
    plugins: Vec<Entry>,
}

#[derive(Clone)]
pub(super) struct Catalog {
    pub(super) path: PathBuf,
    pub(super) entries: Vec<Entry>,
    official_index: Option<Url>,
}

impl Catalog {
    pub(super) fn open(path: &Path) -> Result<Self> {
        let path = path.canonicalize()?;
        let bytes = bounded(&path, CATALOG_LIMIT)?;
        Self::parse(path, None, bytes.as_bytes())
    }

    pub(super) fn restore_official(path: PathBuf, index: Url) -> Result<Self> {
        let bytes = bounded(&path, CATALOG_LIMIT)?;
        Self::parse(path, Some(index), bytes.as_bytes())
    }

    pub(super) fn sync_official(path: PathBuf, index: Url) -> Result<Self> {
        let bytes = download(&index, CATALOG_LIMIT)?;
        let catalog = Self::parse(path, Some(index), &bytes)?;
        crate::model::atomic_write(&catalog.path, &bytes)?;
        Ok(catalog)
    }

    pub(super) fn is_official(&self) -> bool {
        self.official_index.is_some()
    }

    pub(super) fn allows_material(&self) -> bool {
        allows_market_material(self.official_index.as_ref(), IS_PREVIEW_BUILD)
    }

    fn parse(path: PathBuf, official_index: Option<Url>, bytes: &[u8]) -> Result<Self> {
        let file: CatalogFile = serde_json::from_slice(bytes)
            .map_err(|error| fail(&format!("集市目录格式无效：{error}")))?;
        if file.format != CATALOG_FORMAT {
            return Err(fail("当前岛仔不支持此集市目录版本"));
        }
        let mut seen = BTreeSet::new();
        for entry in &file.plugins {
            if !valid_id(&entry.id)
                || version(&entry.version).is_none()
                || entry.revoked.iter().any(|item| version(item).is_none())
                || entry.name.trim().is_empty()
                || entry.name.chars().count() > 80
                || entry.author.trim().is_empty()
                || entry.author.chars().count() > 80
                || !(1..=FONT_ZIP_LIMIT).contains(&entry.size)
                || entry.sha256.len() != 64
                || !entry
                    .sha256
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
                || entry
                    .presentation
                    .as_ref()
                    .is_some_and(|presentation| !presentation.valid())
                || !seen.insert(&entry.id)
            {
                return Err(fail("集市目录包含无效或重复的插件条目"));
            }
        }
        Ok(Self {
            path,
            entries: file.plugins,
            official_index,
        })
    }

    pub(super) fn load(&self, entry: &Entry) -> Result<Package> {
        if entry.revokes(&entry.version) {
            return Err(fail("此插件版本已撤回"));
        }
        let raw = match &self.official_index {
            Some(index) => download_catalog_asset(index, &entry.zip_name(), entry.size)?,
            None => read_catalog_zip(&entry.path_in(&self.path))?,
        };
        if raw.len() as u64 != entry.size || sha256(&raw) != entry.sha256 {
            return Err(fail("插件包大小或 SHA-256 与集市目录不一致"));
        }
        let package = decode_zip(&raw, self.allows_material())?;
        if !entry.matches(&package.manifest) {
            return Err(fail("插件清单与集市目录不一致"));
        }
        Ok(package)
    }

    pub(super) fn cover_png(&self, entry: &Entry) -> Result<Vec<u8>> {
        self.load(entry)?
            .cover_png()
            .ok_or_else(|| fail("插件 ZIP 缺少封面"))
    }

    pub(super) fn avatar_png(&self, name: &str) -> Option<CoverPng> {
        if !valid_avatar_name(name) {
            return None;
        }
        let bytes = match &self.official_index {
            Some(source) => download_catalog_asset(source, name, AVATAR_LIMIT),
            None => read_local_avatar(&self.path.with_file_name(name)),
        }
        .ok()?;
        CoverPng::decode(bytes).ok()
    }

    pub(super) fn has_image(&self, key: &str) -> bool {
        self.entries
            .iter()
            .any(|entry| entry.sha256 == key || entry.avatar_name() == Some(key))
    }
}

fn download_catalog_asset(index: &Url, name: &str, limit: u64) -> Result<Vec<u8>> {
    let mut asset = index.join(name)?;
    asset.set_query(index.query());
    download(&asset, limit)
}

fn version(value: &str) -> Option<[u32; 3]> {
    let mut segments = value.split('.');
    let mut result = [0; 3];
    for part in &mut result {
        let raw = segments.next()?;
        *part = raw.parse().ok()?;
        if part.to_string() != raw {
            return None;
        }
    }
    segments.next().is_none().then_some(result)
}

pub(super) fn compare_versions(left: &str, right: &str) -> Option<Ordering> {
    Some(version(left)?.cmp(&version(right)?))
}

fn sha256(raw: &[u8]) -> String {
    let mut hex = String::with_capacity(64);
    for byte in digest(&SHA256, raw).as_ref() {
        use std::fmt::Write as _;
        write!(&mut hex, "{byte:02x}").expect("写入字符串不会失败");
    }
    hex
}

fn read_limited(reader: impl Read, limit: u64) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(fail("集市文件超过大小限制"));
    }
    Ok(bytes)
}

fn download(url: &Url, limit: u64) -> Result<Vec<u8>> {
    let (_, path) = url
        .path()
        .split_once("/contents/")
        .ok_or_else(|| fail("集市下载地址无效"))?;
    let agent = ureq::AgentBuilder::new()
        .try_proxy_from_env(url.scheme() == "https")
        .redirects(0)
        .timeout_connect(Duration::from_secs(6))
        .timeout(Duration::from_secs(15))
        .build();
    let response = match market_request(&agent, url, "application/json").call() {
        Ok(response) => response,
        Err(ureq::Error::Status(status, response)) => {
            if gitee_rate_limited(status, response.into_reader()) {
                if let Some(raw) = gitee_raw_url(url) {
                    return download_gitee_raw(&agent, &raw, limit);
                }
            }
            return Err(fail(&format!("集市服务返回 HTTP {status}")));
        }
        Err(_) => return Err(fail("无法连接集市，请检查网络后重试")),
    };
    if response.status() != 200 {
        return Err(fail("集市下载地址发生非预期跳转"));
    }
    let encoded_limit = limit.div_ceil(3) * 4;
    // 为 JSON 元数据和 Base64 换行留出空间，读取前仍有固定上限。
    let body = read_limited(
        response.into_reader(),
        encoded_limit + encoded_limit / 8 + 16 * 1024,
    )?;
    decode_gitee_file(&body, path, limit)
}

fn market_request(agent: &ureq::Agent, url: &Url, accept: &str) -> ureq::Request {
    agent
        .get(url.as_str())
        .set("User-Agent", concat!("Daozai/", env!("CARGO_PKG_VERSION")))
        .set("Accept", accept)
        .set("Cache-Control", "no-cache")
}

fn gitee_rate_limited(status: u16, body: impl Read) -> bool {
    const MESSAGE: &[u8] = b"Rate Limit Exceeded";
    status == 429
        || (status == 403
            && read_limited(body, 4096)
                .is_ok_and(|body| body.windows(MESSAGE.len()).any(|part| part == MESSAGE)))
}

fn is_https_host(url: &Url, host: &str) -> bool {
    url.scheme() == "https"
        && url.host_str() == Some(host)
        && url.port().is_none()
        && url.username().is_empty()
        && url.password().is_none()
        && url.fragment().is_none()
}

fn read_local_avatar(path: &Path) -> Result<Vec<u8>> {
    let file = open_plain_file(path, "插件头像必须是测试集市同目录中的普通文件")?;
    read_limited(file, AVATAR_LIMIT)
}

fn github_avatar_url(profile: &[u8], username: &str) -> Result<Url> {
    #[derive(Deserialize)]
    struct Profile {
        login: String,
        avatar_url: String,
    }
    let Profile { login, avatar_url } = serde_json::from_slice(profile)
        .map_err(|_| fail("GitHub 未返回有效的作者头像信息"))?;
    let mut avatar_url = Url::parse(&avatar_url)
        .map_err(|_| fail("GitHub 返回的头像地址无效"))?;
    if !login.eq_ignore_ascii_case(username)
        || !is_https_host(&avatar_url, "avatars.githubusercontent.com")
    {
        return Err(fail("GitHub 返回的账号或头像地址与请求不符"));
    }
    avatar_url.set_query(Some("s=64"));
    Ok(avatar_url)
}

fn github_file(agent: &ureq::Agent, url: &Url, accept: &str, limit: u64) -> Result<Vec<u8>> {
    let response = match market_request(agent, url, accept).call() {
        Ok(response) => response,
        Err(ureq::Error::Status(status, response)) => {
            if status == 429
                || (status == 403 && response.header("X-RateLimit-Remaining") == Some("0"))
            {
                return Err(fail("GitHub 请求次数已达上限，请稍后重试"));
            }
            return Err(fail(&format!("GitHub 返回 HTTP {status}")));
        }
        Err(_) => return Err(fail("无法连接 GitHub，请检查网络后重试")),
    };
    if response.status() != 200 {
        return Err(fail("GitHub 头像请求发生非预期跳转"));
    }
    read_limited(response.into_reader(), limit)
}

fn sync_author_avatar(path: &Path, id: &str, username: &str) -> Result<()> {
    if !valid_id(id) || !valid_github_login(username) {
        return Err(fail("请填写有效的插件 ID 和 GitHub 用户名"));
    }
    let agent = ureq::AgentBuilder::new()
        .try_proxy_from_env(true)
        .redirects(0)
        .timeout_connect(Duration::from_secs(6))
        .timeout(Duration::from_secs(15))
        .build();
    let endpoint = Url::parse(&format!("https://api.github.com/users/{username}"))?;
    let profile = github_file(&agent, &endpoint, "application/vnd.github+json", 16 * 1024)?;
    let url = github_avatar_url(&profile, username)?;
    let raw = github_file(&agent, &url, "image/png,image/jpeg,image/webp", AVATAR_LIMIT)?;
    let mut reader = image::ImageReader::new(Cursor::new(raw)).with_guessed_format()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(512);
    limits.max_image_height = Some(512);
    limits.max_alloc = Some(8 * 1024 * 1024);
    reader.limits(limits);
    let avatar = reader.decode()?.thumbnail(64, 64).into_rgba8();
    let mut png = Cursor::new(Vec::new());
    avatar.write_to(&mut png, image::ImageFormat::Png)?;
    set_author_avatar(path, id, png.get_ref())
}

fn gitee_raw_url(source: &Url) -> Option<Url> {
    if !is_https_host(source, "gitee.com") || source.query() != Some("ref=master") {
        return None;
    }
    let prefix = format!("/api/v5/repos/{GITEE_REPOSITORY}/contents/");
    let (channel, name) = source
        .path()
        .strip_prefix(prefix.as_str())?
        .split_once('/')?;
    let plugin_zip = name
        .strip_prefix("plugin-")
        .and_then(|name| name.strip_suffix(".zip"))
        .and_then(|name| name.rsplit_once('-'))
        .is_some_and(|(id, version_name)| valid_id(id) && version(version_name).is_some());
    if !matches!(channel, "preview" | "stable")
        || !(name == "catalog.json" || valid_avatar_name(name) || plugin_zip)
    {
        return None;
    }
    Url::parse(&format!(
        "https://gitee.com/{GITEE_REPOSITORY}/raw/master/{channel}/{name}"
    ))
    .ok()
}

fn gitee_signed_location(raw: &Url, location: &str) -> Option<Url> {
    let url = Url::parse(location).ok()?;
    (is_https_host(&url, "raw.giteeusercontent.com")
        && url.path() == raw.path()
        && url
            .query_pairs()
            .any(|(key, value)| key == "signature" && !value.is_empty()))
    .then_some(url)
}

fn download_gitee_raw(agent: &ureq::Agent, raw: &Url, limit: u64) -> Result<Vec<u8>> {
    let request = |url: &Url| {
        market_request(agent, url, "*/*")
            .call()
            .map_err(|error| match error {
                ureq::Error::Status(status, _) => fail(&format!("集市镜像返回 HTTP {status}")),
                _ => fail("无法连接集市，请检查网络后重试"),
            })
    };
    let response = request(raw)?;
    let response = if response.status() == 302 {
        let target = response
            .header("Location")
            .and_then(|location| gitee_signed_location(raw, location))
            .ok_or_else(|| fail("集市下载地址发生非预期跳转"))?;
        request(&target)?
    } else {
        response
    };
    if response.status() != 200 {
        return Err(fail("集市下载地址发生非预期跳转"));
    }
    read_limited(response.into_reader(), limit)
}

fn decode_gitee_file(body: &[u8], expected_path: &str, limit: u64) -> Result<Vec<u8>> {
    #[derive(Deserialize)]
    struct Content {
        #[serde(rename = "type")]
        kind: String,
        encoding: String,
        path: String,
        size: u64,
        content: String,
    }
    let file: Content = serde_json::from_slice(body).map_err(|_| fail("集市接口未返回有效文件"))?;
    if file.kind != "file" || file.encoding != "base64" || file.path != expected_path {
        return Err(fail("集市返回的文件信息与请求不一致"));
    }
    if file.size > limit {
        return Err(fail("集市文件超过大小限制"));
    }
    let expected_encoded = (file.size.div_ceil(3) * 4) as usize;
    let encoded: Vec<_> = file
        .content
        .bytes()
        .filter(|byte| !matches!(byte, b'\r' | b'\n'))
        .take(expected_encoded + 1)
        .collect();
    if encoded.len() != expected_encoded {
        return Err(fail("集市返回的文件大小不一致"));
    }
    let bytes = STANDARD
        .decode(encoded)
        .map_err(|_| fail("集市返回的文件编码无效"))?;
    if bytes.len() as u64 != file.size {
        return Err(fail("集市返回的文件大小不一致"));
    }
    Ok(bytes)
}

fn read_catalog_zip(path: &Path) -> Result<Vec<u8>> {
    if !fs::symlink_metadata(path)?.is_file() || path.canonicalize()?.parent() != path.parent() {
        return Err(fail("测试集市插件包必须是同目录的普通文件"));
    }
    read_limited(File::open(path)?, FONT_ZIP_LIMIT)
}

fn supported(manifest: &Manifest, allow_material: bool) -> Result<()> {
    let material = manifest.is_material() && allow_material;
    if (manifest.runtime != "js" && !manifest.is_page_shortcuts() && !manifest.is_wireless() && !manifest.is_mail_view() && !manifest.is_lyrics_style()
        && manifest.appearance_kind().is_none() && !material)
        || manifest.entry.contains('/')
    {
        return Err(fail("集市支持入口在包根目录的脚本插件、外观资源包、快捷切换插件、邮箱视图、歌词样式和无线设备兼容包"));
    }
    Ok(())
}

fn decode_zip(raw: &[u8], allow_material: bool) -> Result<Package> {
    if raw.len() as u64 > FONT_ZIP_LIMIT {
        return Err(fail("插件 ZIP 超过大小限制"));
    }
    let mut zip = ZipArchive::new(Cursor::new(raw))
        .map_err(|error| fail(&format!("插件 ZIP 格式无效：{error}")))?;
    if zip.has_duplicate_names()
        || zip.offset() != 0
        || zip.has_overlapping_entries()?
    {
        return Err(fail("插件 ZIP 包含重复或无效文件"));
    }
    let manifest = Manifest::decode(&zip_text(&mut zip, "plugin.json", MANIFEST_LIMIT)?)?;
    supported(&manifest, allow_material)?;
    let canvas = manifest.has_capability("status.canvas");
    let font = manifest.has_capability("appearance.font");
    if canvas && raw.len() as u64 > CANVAS_ZIP_LIMIT {
        return Err(fail("画布插件 ZIP 超过大小限制"));
    }
    if !canvas && !font && raw.len() as u64 > ZIP_LIMIT {
        return Err(fail("插件 ZIP 超过 832 KiB"));
    }
    if !canvas && !font && zip.len() != 3 + manifest.data.len() {
        return Err(fail("插件 ZIP 只能包含清单、入口、封面和声明的数据文件"));
    }
    let mut images = Vec::new();
    if canvas {
        for name in zip.file_names() {
            let name = name?;
            if name != "plugin.json" && name != manifest.entry.as_str()
                && !manifest.data.iter().any(|data| data == name.as_ref())
            {
                images.push(name.into_owned());
            }
        }
    }
    let source = zip_text(&mut zip, &manifest.entry, manifest.source_limit())?;
    let cover = CoverPng::decode(zip_bytes(&mut zip, COVER_FILE, COVER_LIMIT)?)?;
    let data = super::package_data::read_data(&manifest.data, |name| {
        zip_text(&mut zip, name, super::package_data::DATA_FILE_LIMIT)
    })?;
    let mut package = Package::from_entry(manifest, source, cover)?;
    package.data = data;
    package.canvas_images = super::canvas_images::decode_all(images, |name, remaining| {
        zip_bytes(&mut zip, name, remaining)
    })?;
    if let Some(entry) = package.font_entry() {
        let licenses: Vec<_> = super::font_assets::LICENSES.into_iter()
            .filter(|name| zip.by_name(name).is_ok()).collect();
        if zip.len() != 3 + package.data.len() + entry.files().count() + licenses.len() {
            return Err(fail("字体 ZIP 包含未声明的文件"));
        }
        package.font_assets = super::font_assets::read(entry, licenses.into_iter(), package.font_base_size()?, |name, limit| {
            zip_bytes(&mut zip, name, limit)
        })?;
    }
    validate_material(&package)?;
    Ok(package)
}

fn validate_material(package: &Package) -> Result<()> {
    if package.manifest.is_material() {
        #[cfg(all(windows, feature = "sdk-material"))]
        crate::material::validate_plugin_shader(&package.source).map_err(|error| fail(&error))?;
        #[cfg(not(all(windows, feature = "sdk-material")))]
        return Err(fail("当前版本未启用 GPU 材质支持"));
    }
    Ok(())
}

fn zip_text(zip: &mut ZipArchive<Cursor<&[u8]>>, name: &str, limit: u64) -> Result<String> {
    String::from_utf8(zip_bytes(zip, name, limit)?)
        .map_err(|_| fail(&format!("插件 ZIP 中的 {name} 必须使用 UTF-8")))
}

fn zip_bytes(zip: &mut ZipArchive<Cursor<&[u8]>>, name: &str, limit: u64) -> Result<Vec<u8>> {
    let file = zip
        .by_name(name)
        .map_err(|_| fail(&format!("插件 ZIP 缺少 {name}")))?;
    if file.name_raw() != name.as_bytes()
        || !file.is_file()
        || file
            .unix_mode()
            .is_some_and(|mode| !matches!(mode & 0o170000, 0 | 0o100000))
        || file.size() > limit
    {
        return Err(fail(&format!(
            "插件 ZIP 中的 {name} 必须是大小合规的普通文件"
        )));
    }
    read_limited(file, limit)
}

fn smoke(package: &Package) -> Result<()> {
    if package.manifest.runtime != "js" {
        return Ok(());
    }
    let values: Values = package
        .manifest
        .settings
        .iter()
        .map(|field| (field.key.clone(), field.default.clone()))
        .collect();
    let today = chrono::Local::now().date_naive().to_string();
    package
        .render(&values, &Value::Null, &today)
        .map_err(|error| {
            fail(&format!(
                "入口 {} 的 refresh 自检失败：{error}",
                package.manifest.entry
            ))
        })?;
    Ok(())
}

fn checked_project(path: &Path) -> Result<Package> {
    if path.file_name().and_then(|name| name.to_str()) != Some("plugin.json") {
        return Err(fail("请指定插件根目录的 plugin.json"));
    }
    let package =
        Package::read(path).map_err(|error| fail(&format!("{}：{error}", path.display())))?;
    supported(&package.manifest, true)?;
    smoke(&package)?;
    Ok(package)
}

pub(super) fn validate_project(path: &Path) -> Result<String> {
    let package = checked_project(path)?;
    validate_material(&package)?;
    Ok(format!(
        "{} v{} 校验通过",
        package.manifest.id, package.manifest.version
    ))
}

pub(super) fn test_project(path: &Path, mock_path: Option<&Path>) -> Result<String> {
    if path.file_name().and_then(|name| name.to_str()) != Some("plugin.json") {
        return Err(fail("请指定插件根目录的 plugin.json"));
    }
    let manifest: Value = serde_json::from_str(&bounded(path, MANIFEST_LIMIT)?)?;
    let report = devtools::capability_report(&manifest);
    let package = Package::read(path)
        .map_err(|error| fail(&format!("{}：{error}\n{report}", path.display())))?;
    let manifest = &package.manifest;
    let heading = format!("{} v{} 校验通过\n{report}", manifest.id, manifest.version);
    if manifest.runtime != "js" {
        if mock_path.is_some() {
            return Err(fail("模拟输入只适用于 JavaScript 插件"));
        }
        validate_material(&package)
            .map_err(|error| fail(&format!("{}：{error}\n{report}", path.display())))?;
        return Ok(heading);
    }
    let mut mock = mock_path
        .map(|path| serde_json::from_str::<devtools::MockInput>(&bounded(path, 2 * 1024 * 1024)?)
            .map_err(|error| fail(&format!("模拟输入 {} 无效：{error}", path.display()))))
        .transpose()?
        .unwrap_or_default();
    let values = package.settings_values(mock.settings.as_ref().unwrap_or(&Values::new()));
    let clock = match mock.now {
        Some(now) => chrono::DateTime::from_timestamp_millis(now)
            .ok_or_else(|| fail("模拟输入 now 超出 Unix 毫秒时间戳的范围"))?
            .with_timezone(&chrono::Local),
        None => chrono::Local::now(),
    };
    mock.now = Some(clock.timestamp_millis());
    let today = clock.date_naive().to_string();
    let mut lines = Vec::new();
    let started = Instant::now();
    let run = package.render_input_with(
        &values, &mock.storage, &today, SourceInput::Unconfigured, super::interactions::Call::Refresh,
        Some(&mock), &mut lines,
    );
    let elapsed = started.elapsed().as_secs_f64() * 1000.0;
    let logs = if lines.is_empty() { String::new() } else {
        format!("\n脚本日志\n{}", lines.join("\n"))
    };
    let (_, raw) = run.map_err(|error| fail(&format!("{error}\n{report}{logs}")))?;
    Ok(format!("{heading}\nrefresh 用时 {elapsed:.2} ms\n{raw}{logs}"))
}

pub(super) fn create_template(
    parent: &Path,
    id: &str,
    name: &str,
    author: &str,
) -> Result<PathBuf> {
    let (id, name, author) = (id.trim(), name.trim(), author.trim());
    if !valid_id(id) {
        return Err(fail("插件 ID 格式应类似 xiaoming.payday，最多 64 个字符"));
    }
    if name.is_empty() || name.chars().count() > 80 {
        return Err(fail("插件名称需要 1 到 80 个字符"));
    }
    if author.is_empty() || author.chars().count() > 80 {
        return Err(fail("作者名称需要 1 到 80 个字符"));
    }

    let mut manifest = Manifest::decode(include_str!("examples/payday/plugin.json"))?;
    manifest.id = id.into();
    manifest.name = name.into();
    manifest.author = author.into();
    let bytes = serde_json::to_vec_pretty(&manifest)?;
    let files: [(&str, &[u8]); 4] = [
        ("plugin.json", &bytes),
        ("main.js", include_bytes!("examples/payday/main.js")),
        (COVER_FILE, include_bytes!("examples/payday/cover.png")),
        ("LICENSE", include_bytes!("examples/payday/LICENSE")),
    ];

    let project = parent.join(id);
    let path = target_path(&project)?;
    if fs::symlink_metadata(&path).is_ok() {
        return Err(fail("同名文件或目录已存在，请更换插件 ID 或保存位置"));
    }
    let tmp = loop {
        let tmp = temporary_sibling(&path);
        match fs::create_dir(&tmp) {
            Ok(()) => break tmp,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    };
    // 文件全部写好后再移入目标路径，已有目录始终保留。
    let result = files
        .iter()
        .try_for_each(|(name, contents)| fs::write(tmp.join(name), contents))
        .and_then(|_| move_new(&tmp, &path));
    if let Err(error) = result {
        for (name, _) in files {
            let _ = fs::remove_file(tmp.join(name));
        }
        let _ = fs::remove_dir(&tmp);
        return Err(fail(&format!(
            "无法创建插件目录 {}：{error}",
            project.display()
        )));
    }
    Ok(project.join("plugin.json"))
}

fn target_path(path: &Path) -> io::Result<PathBuf> {
    let filename = path
        .file_name()
        .ok_or_else(|| io::Error::other("缺少目标文件名"))?;
    let directory = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
        .canonicalize()?;
    Ok(directory.join(filename))
}

fn lock_catalog(path: &Path) -> io::Result<File> {
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path.with_file_name(".daozai-test-market.lock"))?;
    lock.lock()?;
    Ok(lock)
}

fn temporary_sibling(path: &Path) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};

    static SERIAL: AtomicU64 = AtomicU64::new(0);
    let mut name = path.file_name().unwrap().to_os_string();
    name.push(format!(
        ".{}-{}.tmp",
        std::process::id(),
        SERIAL.fetch_add(1, AtomicOrdering::Relaxed)
    ));
    path.with_file_name(name)
}

fn move_new(tmp: &Path, path: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::{HRESULT, PCWSTR};
    use windows::Win32::{
        Foundation::{ERROR_ALREADY_EXISTS, ERROR_FILE_EXISTS},
        Storage::FileSystem::{MoveFileExW, MOVEFILE_WRITE_THROUGH},
    };

    let source: Vec<u16> = tmp.as_os_str().encode_wide().chain(Some(0)).collect();
    let destination: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    unsafe {
        MoveFileExW(
            PCWSTR(source.as_ptr()),
            PCWSTR(destination.as_ptr()),
            MOVEFILE_WRITE_THROUGH,
        )
    }
    .map_err(|error| {
        if error.code() == HRESULT::from_win32(ERROR_ALREADY_EXISTS.0)
            || error.code() == HRESULT::from_win32(ERROR_FILE_EXISTS.0)
        {
            io::Error::new(io::ErrorKind::AlreadyExists, error)
        } else {
            io::Error::other(error)
        }
    })
}

fn write_new(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let path = target_path(path)?;
    let (tmp, mut file) = loop {
        let tmp = temporary_sibling(&path);
        match OpenOptions::new().write(true).create_new(true).open(&tmp) {
            Ok(file) => break (tmp, file),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    };

    // 版本文件只在完整写盘后出现，移动时保留已存在的目标。
    let result = file.write_all(bytes).and_then(|_| file.sync_all());
    drop(file);
    let result = result.and_then(|_| move_new(&tmp, &path));
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

fn set_author_avatar(path: &Path, id: &str, raw: &[u8]) -> Result<()> {
    if raw.len() as u64 > AVATAR_LIMIT {
        return Err(fail("作者头像不能超过 64 KiB"));
    }
    CoverPng::decode(raw.to_vec()).map_err(|_| fail("作者头像必须是完整的 PNG 图片"))?;
    let path = target_path(path)?;
    let _lock = lock_catalog(&path)?;
    let file = open_plain_file(&path, "测试集市目录必须是普通文件")?;
    let mut catalog = Catalog::parse(path.clone(), None, &read_limited(file, CATALOG_LIMIT)?)?;
    let entry = catalog
        .entries
        .iter_mut()
        .find(|entry| entry.id == id)
        .ok_or_else(|| fail("测试集市没有这个插件"))?;
    let name = format!("avatar-{}.png", sha256(raw));
    let avatar_path = path.with_file_name(&name);
    entry.presentation.get_or_insert_with(Presentation::default).author_avatar = Some(name);
    let updated = serde_json::to_vec_pretty(&CatalogFile {
        format: CATALOG_FORMAT,
        plugins: catalog.entries,
    })?;
    if updated.len() as u64 > CATALOG_LIMIT {
        return Err(fail("测试集市目录超过大小限制"));
    }
    let created = match write_new(&avatar_path, raw) {
        Ok(()) => true,
        Err(error)
            if error.kind() == io::ErrorKind::AlreadyExists
                && read_local_avatar(&avatar_path).is_ok_and(|existing| existing == raw) =>
        {
            false
        }
        Err(error) => return Err(fail(&format!("无法保存作者头像：{error}"))),
    };
    if let Err(error) = crate::model::atomic_write(&path, &updated) {
        if created {
            let _ = fs::remove_file(&avatar_path);
        }
        return Err(fail(&format!("测试集市目录更新失败：{error}")));
    }
    Ok(())
}

pub(super) fn pack(manifest_path: &Path, output: &Path) -> Result<()> {
    let package = checked_project(manifest_path)?;
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    zip.start_file("plugin.json", options)?;
    zip.write_all(&serde_json::to_vec(&package.manifest)?)?;
    zip.start_file(&package.manifest.entry, options)?;
    zip.write_all(package.source.as_bytes())?;
    zip.start_file(COVER_FILE, options)?;
    let cover = package.cover.as_ref().ok_or_else(|| fail("缺少插件封面"))?;
    zip.write_all(cover.bytes())?;
    for (name, text) in &package.data {
        zip.start_file(name, options)?;
        zip.write_all(text.as_bytes())?;
    }
    for (name, image) in &package.canvas_images {
        if name != COVER_FILE {
            zip.start_file(name, options)?;
            zip.write_all(image.bytes())?;
        }
    }
    for (name, font) in &package.font_assets {
        zip.start_file(name, options)?;
        zip.write_all(font.bytes())?;
    }
    let raw = zip.finish()?.into_inner();
    decode_zip(&raw, true)?;
    write_new(output, &raw)
        .map_err(|error| fail(&format!("无法创建 ZIP {}：{error}", output.display())))?;
    Ok(())
}

pub(super) fn submit(input: &Path, path: &Path) -> Result<()> {
    let raw = read_limited(File::open(input)?, FONT_ZIP_LIMIT)?;
    let package = decode_zip(&raw, true)?;
    smoke(&package)?;
    let manifest = package.manifest;
    let updated_on = chrono::Local::now().date_naive().to_string();
    let mut entry = Entry {
        id: manifest.id,
        name: manifest.name,
        author: manifest.author,
        version: manifest.version,
        size: raw.len() as u64,
        sha256: sha256(&raw),
        owner: Some(manifest.owner),
        presentation: Some(Presentation {
            updated_on: Some(updated_on.clone()),
            ..Presentation::default()
        }),
        revoked: BTreeSet::new(),
    };
    let path = target_path(path)?;
    // 操作系统在作者进程退出时释放锁，目录里的版本更新串行提交。
    let _lock = lock_catalog(&path)?;
    let mut plugins = match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.is_file() => Catalog::open(&path)?.entries,
        Ok(_) => return Err(fail("测试集市目录必须是普通文件")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(error.into()),
    };
    let zip_path = entry.path_in(&path);
    if let Some(old) = plugins.iter_mut().find(|old| old.id == entry.id) {
        if compare_versions(&entry.version, &old.version) != Some(Ordering::Greater) {
            return Err(fail("提交版本必须高于测试集市已有版本"));
        }
        if old.revokes(&entry.version) {
            return Err(fail("此插件版本已撤回"));
        }
        if let Some(mut presentation) = old.presentation.take() {
            presentation.updated_on = Some(updated_on);
            entry.presentation = Some(presentation);
        }
        entry.revoked = std::mem::take(&mut old.revoked);
        *old = entry;
    } else {
        plugins.push(entry);
    }
    plugins.sort_by(|left, right| left.id.cmp(&right.id));
    let catalog = serde_json::to_vec_pretty(&CatalogFile {
        format: CATALOG_FORMAT,
        plugins,
    })?;
    if catalog.len() as u64 > CATALOG_LIMIT {
        return Err(fail("测试集市目录超过大小限制"));
    }
    let created = match write_new(&zip_path, &raw) {
        Ok(()) => true,
        Err(error)
            if error.kind() == io::ErrorKind::AlreadyExists
                && read_catalog_zip(&zip_path).is_ok_and(|existing| existing == raw) =>
        {
            // 上次在写入目录前退出，可继续提交字节完全相同的版本包。
            false
        }
        Err(error) => return Err(fail(&format!("无法保存不可覆盖的版本 ZIP：{error}"))),
    };
    if let Err(error) = crate::model::atomic_write(&path, &catalog) {
        if created {
            let _ = fs::remove_file(&zip_path);
        }
        return Err(fail(&format!("测试集市目录提交失败：{error}")));
    }
    Ok(())
}

pub(in crate::sdk_spike) fn author_command(args: &[OsString]) -> Option<ExitCode> {
    let operation = args.get(1)?.to_str()?;
    if !matches!(
        operation,
        "--plugin-test" | "--plugin-pack" | "--plugin-submit" | "--plugin-avatar"
    ) {
        return None;
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn AttachConsole(process_id: u32) -> i32;
    }
    // Windows 图形程序的显式作者命令与父终端共用控制台。
    unsafe { AttachConsole(u32::MAX) };
    let result = match (operation, &args[2..]) {
        ("--plugin-test", [manifest]) => test_project(Path::new(manifest), None),
        ("--plugin-test", [manifest, flag, mock]) if flag == "--input" => {
            test_project(Path::new(manifest), Some(Path::new(mock)))
        }
        ("--plugin-pack", [manifest, output]) => pack(Path::new(manifest), Path::new(output))
            .map(|_| format!("已生成插件包 {}", Path::new(output).display())),
        ("--plugin-submit", [zip, catalog]) => submit(Path::new(zip), Path::new(catalog))
            .map(|_| format!("已提交到测试集市 {}", Path::new(catalog).display())),
        ("--plugin-avatar", [catalog, id, username]) => sync_author_avatar(
            Path::new(catalog),
            id.to_str().unwrap_or_default(),
            username.to_str().unwrap_or_default(),
        )
        .map(|_| "已更新测试集市作者头像".into()),
        _ => Err(fail(match operation {
            "--plugin-test" => "用法：--plugin-test <plugin.json> [--input <mock.json>]",
            "--plugin-pack" => "用法：--plugin-pack <plugin.json> <输出 ZIP>",
            "--plugin-avatar" => "用法：--plugin-avatar <catalog.json> <插件 ID> <GitHub 用户名>",
            _ => "用法：--plugin-submit <插件 ZIP> <catalog.json>",
        })),
    };
    Some(match result {
        Ok(message) => {
            println!("{message}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    })
}

#[cfg(test)]
#[path = "market_tests.rs"]
mod tests;
