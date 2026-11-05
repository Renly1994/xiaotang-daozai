//! Read-only access to Steam's binary KeyValues achievement caches.
use crate::steam::Achievement;
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime},
};

const MAX_FILE: u64 = 4 * 1024 * 1024;
const MIN_UNLOCK_TIME: u64 = 1_000_000_000;

enum Value<'a> {
    Object(Vec<Node<'a>>),
    Text(&'a [u8]),
    Integer(u64),
    Ignored,
}

struct Node<'a> {
    key: &'a [u8],
    value: Value<'a>,
}

impl<'a> Node<'a> {
    fn children(&self) -> &[Node<'a>] {
        match &self.value {
            Value::Object(nodes) => nodes,
            _ => &[],
        }
    }

    fn child(&self, key: &[u8]) -> Option<&Node<'a>> {
        self.children().iter().find(|node| node.key == key)
    }

    fn text(&self) -> Option<String> {
        match self.value {
            Value::Text(bytes) if !bytes.is_empty() => {
                Some(String::from_utf8_lossy(bytes).into_owned())
            }
            _ => None,
        }
    }

    fn number(&self) -> Option<u32> {
        std::str::from_utf8(self.key).ok()?.parse().ok()
    }

    fn walk(&self, group: u32, key: &[u8], visit: &mut impl FnMut(u32, &Node<'a>)) {
        if self.key == key {
            visit(group, self);
        }
        for child in self.children() {
            child.walk(self.number().unwrap_or(group), key, visit);
        }
    }
}

struct Parser<'a>(&'a [u8]);

impl<'a> Parser<'a> {
    fn take(&mut self, len: usize) -> Option<&'a [u8]> {
        let bytes = self.0.get(..len)?;
        self.0 = &self.0[len..];
        Some(bytes)
    }

    fn string(&mut self) -> Option<&'a [u8]> {
        let len = self.0.iter().position(|byte| *byte == 0)?;
        Some(&self.take(len + 1)?[..len])
    }

    fn node(&mut self, depth: u32) -> Option<Node<'a>> {
        if depth > 128 {
            return None;
        }
        let kind = self.take(1)?[0];
        let key = self.string()?;
        let value = match kind {
            0 => {
                let mut children = Vec::new();
                while *self.0.first()? != 8 {
                    children.push(self.node(depth + 1)?);
                }
                self.take(1)?;
                Value::Object(children)
            }
            1 => Value::Text(self.string()?),
            2 => u64::try_from(i32::from_le_bytes(self.take(4)?.try_into().ok()?))
                .map_or(Value::Ignored, Value::Integer),
            3 | 6 => {
                self.take(4)?;
                Value::Ignored
            }
            5 => {
                let len = u32::from_le_bytes(self.take(4)?.try_into().ok()?) as usize;
                self.take(len.checked_mul(2)?)?;
                Value::Ignored
            }
            7 => Value::Integer(u64::from_le_bytes(self.take(8)?.try_into().ok()?)),
            _ => return None,
        };
        Some(Node { key, value })
    }

    fn root(bytes: &'a [u8]) -> Option<Node<'a>> {
        let mut parser = Self(bytes);
        let root = parser.node(0)?;
        (matches!(root.value, Value::Object(_)) && parser.0.iter().all(|byte| *byte == 8))
            .then_some(root)
    }
}

fn translated(node: Option<&Node<'_>>) -> Option<String> {
    let node = node?;
    [b"schinese".as_slice(), b"tchinese", b"english"]
        .iter()
        .find_map(|key| node.child(key)?.text())
        .or_else(|| node.text())
        .or_else(|| node.children().iter().find_map(Node::text))
}

fn achievement_icon(app: u32, value: Option<&Node<'_>>) -> Option<String> {
    let hash = value?.text()?;
    let hash = hash.strip_suffix(".jpg")?;
    (hash.len() == 40 && hash.bytes().all(|byte| byte.is_ascii_hexdigit())).then(|| {
        format!("https://cdn.cloudflare.steamstatic.com/steamcommunity/public/images/apps/{app}/{hash}.jpg")
    })
}

fn parse(stats: &[u8], schema: &[u8], app_id: u32) -> Option<Vec<Achievement>> {
    let mut times = BTreeMap::new();
    Parser::root(stats)?.walk(0, b"AchievementTimes", &mut |group, node| {
        for bit in node.children() {
            if let (Some(bit_id), Value::Integer(time)) = (bit.number(), &bit.value) {
                if *time >= MIN_UNLOCK_TIME && *time <= u32::MAX as u64 {
                    times.insert((group, bit_id), *time);
                }
            }
        }
    });
    let mut items = BTreeMap::new();
    Parser::root(schema)?.walk(0, b"bits", &mut |group, node| {
        for bit in node.children() {
            let Some(bit_id) = bit.number() else { continue };
            let Some(api_name) = bit.child(b"name").and_then(Node::text) else {
                continue;
            };
            let display = bit.child(b"display");
            let name = translated(display.and_then(|node| node.child(b"name")))
                .unwrap_or_else(|| api_name.clone());
            let description =
                translated(display.and_then(|node| node.child(b"desc"))).unwrap_or_default();
            let icon = achievement_icon(app_id, display.and_then(|node|node.child(b"icon")).or_else(||bit.child(b"icon")));
            let unlocktime = times.remove(&(group, bit_id)).unwrap_or(0);
            items.insert(
                api_name.clone(),
                Achievement {
                    api_name,
                    name,
                    description,
                    unlocked: unlocktime != 0,
                    unlocktime,
                    icon,
                },
            );
        }
    });
    // A mismatched schema cannot tell us which previously unlocked entries were relocked.
    (!items.is_empty() && times.is_empty()).then(|| items.into_values().collect())
}

type Stamp = (SystemTime, u64);

fn stamp(path: &Path) -> std::io::Result<Stamp> {
    let metadata = fs::metadata(path)?;
    Ok((metadata.modified()?, metadata.len()))
}

fn read(path: &Path) -> Option<Vec<u8>> {
    let mut bytes = Vec::new();
    File::open(path)
        .ok()?
        .take(MAX_FILE + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    (bytes.len() as u64 <= MAX_FILE).then_some(bytes)
}

#[derive(Default)]
pub(super) struct LocalCache {
    directory: Option<PathBuf>,
    path_due: Option<Instant>,
    stamp: Option<(Stamp, Stamp)>,
    game_icons: BTreeMap<u32, Option<PathBuf>>,
}

impl LocalCache {
    pub(super) fn reset_game(&mut self) {
        self.stamp = None;
    }

    pub(super) fn game_icon(&mut self, app_id: u32) -> Option<PathBuf> {
        // Do not cache a miss before the install path has been discovered on the
        // first polling cycle. The Steam directory is commonly registered under
        // HKLM on 32-bit Steam installs.
        let library = self
            .directory
            .as_ref()?
            .parent()?
            .join("librarycache")
            .join(app_id.to_string());
        self.game_icons
            .entry(app_id)
            .or_insert_with(|| {
                let mut icons = fs::read_dir(&library)
                    .ok()?
                    .filter_map(Result::ok)
                    .map(|entry| entry.path())
                    .filter(|path| {
                        path.extension()
                            .is_some_and(|extension| extension.eq_ignore_ascii_case("jpg"))
                            && path.file_stem().is_some_and(|stem| {
                                let value = stem.to_string_lossy();
                                value.len() == 40
                                    && value.bytes().all(|byte| byte.is_ascii_hexdigit())
                            })
                    })
                    .collect::<Vec<_>>();
                icons.sort();
                icons.into_iter().next()
            })
            .clone()
    }

    pub(super) fn poll(
        &mut self,
        account: u32,
        game: u32,
    ) -> (&'static str, Option<Vec<Achievement>>) {
        if self.path_due.map_or(true, |due| Instant::now() >= due) {
            let directory = steam_directory().map(|path| path.join("appcache").join("stats"));
            if directory != self.directory {
                self.stamp = None;
                self.game_icons.clear();
                self.directory = directory;
            }
            self.path_due = Some(Instant::now() + Duration::from_secs(60));
        }
        let Some(dir) = &self.directory else {
            return ("未找到本机 Steam 安装目录", None);
        };
        let stats = dir.join(format!("UserGameStats_{account}_{game}.bin"));
        let schema = dir.join(format!("UserGameStatsSchema_{game}.bin"));
        self.poll_files(&stats, &schema, game)
    }

    fn poll_files(
        &mut self,
        stats: &Path,
        schema: &Path,
        app_id: u32,
    ) -> (&'static str, Option<Vec<Achievement>>) {
        let stamps = || Some((stamp(stats).ok()?, stamp(schema).ok()?));
        let Some(before) = stamps() else {
            self.stamp = None;
            return ("等待 Steam 写入该账号的成就缓存", None);
        };
        if self.stamp == Some(before) {
            return ("正在监视本地缓存", None);
        }
        let read_items = || parse(&read(stats)?, &read(schema)?, app_id);
        if let Some(items) = read_items() {
            if stamps() == Some(before) {
                self.stamp = Some(before);
                return ("正在监视本地缓存", Some(items));
            }
        }
        ("本地缓存暂时无法读取，稍后重试", None)
    }
}

#[cfg(windows)]
fn steam_directory() -> Option<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    use windows::{
        core::{w, PCWSTR},
        Win32::System::Registry::{
            RegGetValueW, HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ,
        },
    };
    fn read_path(root: HKEY, subkey: PCWSTR, value: PCWSTR) -> Option<PathBuf> {
        let mut path = [0u16; 1024];
        let mut size = std::mem::size_of_val(&path) as u32;
        let status = unsafe {
            RegGetValueW(
                root,
                subkey,
                value,
                RRF_RT_REG_SZ,
                None,
                Some(path.as_mut_ptr().cast()),
                Some(&mut size),
            )
        };
        if status.is_err() {
            return None;
        }
        let end = path.iter().position(|unit| *unit == 0)?;
        (end > 0).then(|| std::ffi::OsString::from_wide(&path[..end]).into())
    }
    [
        (
            HKEY_CURRENT_USER,
            w!("Software\\Valve\\Steam"),
            w!("SteamPath"),
        ),
        (
            HKEY_CURRENT_USER,
            w!("Software\\WOW6432Node\\Valve\\Steam"),
            w!("SteamPath"),
        ),
        (
            HKEY_LOCAL_MACHINE,
            w!("Software\\Valve\\Steam"),
            w!("InstallPath"),
        ),
        (
            HKEY_LOCAL_MACHINE,
            w!("Software\\WOW6432Node\\Valve\\Steam"),
            w!("InstallPath"),
        ),
    ]
    .into_iter()
    .find_map(|(root, subkey, value)| read_path(root, subkey, value))
}

#[cfg(not(windows))]
fn steam_directory() -> Option<PathBuf> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn object(name: &[u8], fields: &[&[u8]]) -> Vec<u8> {
        let mut bytes = vec![0];
        bytes.extend(name);
        bytes.push(0);
        for field in fields {
            bytes.extend(*field);
        }
        bytes.push(8);
        bytes
    }

    fn schema() -> Vec<u8> {
        let names = object(
            b"name",
            &[b"\x01english\0First\0", "\x01schinese\0初次\0".as_bytes()],
        );
        let icon = b"\x01icon\00123456789abcdef0123456789abcdef01234567.jpg\0";
        let display = object(b"display", &[&names, icon]);
        let first = object(b"0", &[b"\x01name\0FIRST\0", &display]);
        let second = object(b"0", &[b"\x01name\0SECOND\0"]);
        let first_group = object(b"1", &[&object(b"bits", &[&first])]);
        let second_group = object(b"2", &[&object(b"bits", &[&second])]);
        object(b"schema", &[&first_group, &second_group])
    }

    fn stats(unlocked: bool) -> Vec<u8> {
        let mut timestamp = b"\x020\0".to_vec();
        timestamp.extend(1_760_000_000u32.to_le_bytes());
        let values = if unlocked {
            vec![timestamp.as_slice()]
        } else {
            Vec::new()
        };
        let times = object(b"AchievementTimes", &values);
        object(b"stats", &[&object(b"2", &[&times])])
    }

    #[test]
    fn binary_cache_maps_group_and_bit_to_the_right_achievement() {
        let items = parse(&stats(true), &schema(), 570).unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!((&*items[0].name, items[0].unlocked), ("初次", false));
        assert_eq!(items[0].icon.as_deref(),Some("https://cdn.cloudflare.steamstatic.com/steamcommunity/public/images/apps/570/0123456789abcdef0123456789abcdef01234567.jpg"));
        assert_eq!(
            (&*items[1].api_name, items[1].unlocktime),
            ("SECOND", 1_760_000_000)
        );
        assert!(parse(&stats(true)[..8], &schema(), 570).is_none());
    }

    #[test]
    fn watcher_retries_partial_writes_and_skips_unchanged_files() {
        let root = std::env::temp_dir().join(format!("daozai-steam-files-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let stats_path = root.join("stats.bin");
        let schema_path = root.join("schema.bin");
        fs::write(&schema_path, schema()).unwrap();
        fs::write(&stats_path, stats(false)).unwrap();
        let mut cache = LocalCache::default();
        let (_, first) = cache.poll_files(&stats_path, &schema_path, 570);
        assert!(!first.unwrap()[1].unlocked);
        assert!(cache.poll_files(&stats_path, &schema_path, 570).1.is_none());
        fs::write(&stats_path, &stats(true)[..8]).unwrap();
        assert!(cache.poll_files(&stats_path, &schema_path, 570).1.is_none());
        fs::write(&stats_path, stats(true)).unwrap();
        let (_, updated) = cache.poll_files(&stats_path, &schema_path, 570);
        assert!(updated.unwrap()[1].unlocked);
        fs::remove_file(stats_path).unwrap();
        fs::remove_file(schema_path).unwrap();
        fs::remove_dir(root).unwrap();
    }
}
