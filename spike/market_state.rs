use super::{market, market_metrics, *};
use std::{cmp::Ordering, sync::Arc, thread::JoinHandle};

const FAVORITES_FILE: &str = "market-favorites.json";
const FAVORITES_LIMIT: u64 = 256 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum MarketStatus {
    Install,
    Update,
    Installed,
    OtherSource,
    Unavailable,
    Revoked,
}

pub(super) struct MarketJob {
    id: String,
    update: bool,
    cancelled: bool,
    result: JoinHandle<(Option<String>, std::result::Result<Record, String>)>,
}

#[derive(Clone)]
struct OfficialImage {
    png: CoverPng,
    is_material: bool,
}

#[derive(Default)]
pub(super) struct OfficialMarket {
    catalog: Option<Arc<market::Catalog>>,
    initialized: bool,
    sync_job: Option<JoinHandle<std::result::Result<market::Catalog, String>>>,
    error: Option<String>,
    image_job: Option<(String, JoinHandle<Option<OfficialImage>>)>,
    images: BTreeMap<String, Option<OfficialImage>>,
    #[cfg(test)]
    test_index: Option<url::Url>,
}

impl LocalPlugins {
    fn saved_market_path(&self) -> Option<PathBuf> {
        let json = bounded(&self.dir.join("market-catalog.json"), 128 * 1024).ok()?;
        serde_json::from_str(&json).ok()
    }

    pub(super) fn restore_market_catalog(&mut self) {
        self.market_catalog = self
            .saved_market_path()
            .and_then(|path| market::Catalog::open(&path).ok())
            .map(Arc::new);
        if let Some(catalog) = self.market_catalog.clone() {
            self.apply_market_revocations(&catalog);
        }
        let mut favorites: BTreeMap<String, String> =
            bounded(&self.dir.join(FAVORITES_FILE), FAVORITES_LIMIT)
                .ok()
                .and_then(|contents| serde_json::from_str(&contents).ok())
                .unwrap_or_default();
        favorites.retain(|id, name| {
            valid_id(id) && !name.trim().is_empty() && name.chars().count() <= 80
        });
        self.market_favorites = favorites;
    }

    pub(super) fn restore_official_revocations(&mut self) {
        #[cfg(not(test))]
        if self.records.values().any(|record| record.official_market && record.package.is_some()) {
            if let Ok(index) = market::official_index_url() {
                let path = market::official_cache_path(&self.dir, &index);
                if let Ok(catalog) = market::Catalog::restore_official(path, index) {
                    self.apply_market_revocations(&catalog);
                    self.official_market.catalog = Some(Arc::new(catalog));
                }
            }
        }
    }

    fn apply_market_revocations(&mut self, catalog: &market::Catalog) {
        for entry in &catalog.entries {
            let Some(mut record) = self.records.get(&entry.id).filter(|record| {
                record.from_market
                    && record.origin == catalog.path
                    && record.official_market == catalog.is_official()
                    && !record.market_revoked
                    && record.package.as_ref().is_some_and(|package| entry.revokes(&package.manifest.version))
            }).cloned() else {
                continue;
            };
            record.enabled = false;
            record.market_revoked = true;
            self.forget_view(&entry.id);
            if let Err(error) = self.put(&entry.id, record) {
                let record = self.records.get_mut(&entry.id).unwrap();
                record.enabled = false;
                record.market_revoked = true;
                self.cancel_market(&entry.id);
                self.refresh_page_shortcuts();
                self.sync_wireless();
                self.notice = format!("插件已停用，无法保存撤回状态：{error}");
            }
        }
    }

    pub(super) fn favorites(&self) -> &BTreeMap<String, String> {
        &self.market_favorites
    }

    pub(super) fn is_favorite(&self, id: &str) -> bool {
        self.market_favorites.contains_key(id)
    }

    pub(super) fn set_favorite(&mut self, entry: &market::Entry, saved: bool) -> Result<()> {
        if !saved {
            return self.remove_favorite(&entry.id);
        }
        if self.is_favorite(&entry.id) {
            return Ok(());
        }
        let mut favorites = self.market_favorites.clone();
        favorites.insert(entry.id.clone(), entry.name.clone());
        self.save_favorites(favorites)
    }

    pub(super) fn remove_favorite(&mut self, id: &str) -> Result<()> {
        if !self.is_favorite(id) {
            return Ok(());
        }
        let mut favorites = self.market_favorites.clone();
        favorites.remove(id);
        self.save_favorites(favorites)
    }

    fn save_favorites(&mut self, favorites: BTreeMap<String, String>) -> Result<()> {
        let contents = serde_json::to_vec(&favorites)?;
        if contents.len() as u64 > FAVORITES_LIMIT {
            return Err(fail("已达到本机收藏上限"));
        }
        crate::model::atomic_write(&self.dir.join(FAVORITES_FILE), &contents)?;
        self.market_favorites = favorites;
        self.sync_market_favorites();
        Ok(())
    }

    fn sync_market_favorites(&mut self) {
        let official_ids = self
            .official_market
            .catalog
            .iter()
            .filter(|catalog| catalog.is_official())
            .flat_map(|catalog| catalog.entries.iter().map(|entry| entry.id.as_str()));
        self.market_metrics
            .sync_favorites(&self.market_favorites, official_ids);
    }

    pub(super) fn market_counts(&self, id: &str) -> Option<market_metrics::Counts> {
        self.market_metrics.counts(id)
    }

    pub(super) fn market_stats_repaint_delay(&self) -> Option<Duration> {
        self.market_metrics.repaint_delay()
    }

    pub(super) fn set_market_catalog(&mut self, path: &Path) -> Result<()> {
        if self.market_busy() {
            return Err(fail("请等待当前集市操作完成"));
        }
        let catalog = market::Catalog::open(path)?;
        crate::model::atomic_write(
            &self.dir.join("market-catalog.json"),
            &serde_json::to_vec(&catalog.path)?,
        )?;
        self.apply_market_revocations(&catalog);
        self.market_catalog = Some(Arc::new(catalog));
        Ok(())
    }

    pub(super) fn refresh_market_catalog(&mut self) -> Result<()> {
        if self.market_busy() {
            return Err(fail("请等待当前集市操作完成"));
        }
        let path = self
            .market_catalog
            .as_ref()
            .map(|catalog| catalog.path.clone())
            .or_else(|| self.saved_market_path())
            .ok_or_else(|| fail("请先选择测试集市目录"))?;
        let catalog = market::Catalog::open(&path)?;
        self.apply_market_revocations(&catalog);
        self.market_catalog = Some(Arc::new(catalog));
        Ok(())
    }

    pub(super) fn ensure_official_catalog(&mut self) {
        self.load_official_catalog(false);
    }

    pub(super) fn refresh_official_catalog(&mut self) {
        self.load_official_catalog(true);
    }

    fn load_official_catalog(&mut self, refresh: bool) {
        let official = &mut self.official_market;
        if official.sync_job.is_some() || (!refresh && official.initialized) {
            return;
        }
        official.initialized = true;
        self.market_metrics.request_refresh(refresh);
        #[cfg(not(test))]
        let index = market::official_index_url().map(Some);
        #[cfg(test)]
        let index: Result<_> = Ok(official.test_index.clone());
        let index = match index {
            Ok(Some(index)) => index,
            Ok(None) => return,
            Err(error) => {
                official.error = Some(error.to_string());
                return;
            }
        };
        let path = market::official_cache_path(&self.dir, &index);
        if official.catalog.is_none() {
            official.catalog = market::Catalog::restore_official(path.clone(), index.clone())
                .ok()
                .map(Arc::new);
        }
        if refresh || official.catalog.is_none() {
            official.error = None;
            official.images.retain(|_, image| image.is_some());
            official.sync_job = Some(std::thread::spawn(move || {
                market::Catalog::sync_official(path, index).map_err(|error| error.to_string())
            }));
        }
        if let Some(catalog) = official.catalog.clone() {
            self.apply_market_revocations(&catalog);
        }
        self.sync_market_favorites();
    }

    pub(super) fn official_catalog_busy(&self) -> bool {
        self.official_market.sync_job.is_some()
    }

    #[cfg(test)]
    pub(super) fn set_official_test_index(&mut self, index: url::Url) {
        self.official_market.test_index = Some(index);
    }

    pub(super) fn official_catalog_error(&self) -> Option<&str> {
        self.official_market.error.as_deref()
    }

    pub(super) fn market_catalog(&self, official: bool) -> Option<Arc<market::Catalog>> {
        if official {
            self.official_market.catalog.clone()
        } else {
            self.market_catalog.clone()
        }
    }

    pub(super) fn official_cover(
        &mut self,
        catalog: &Arc<market::Catalog>,
        entry: &market::Entry,
    ) -> Option<std::result::Result<CoverPng, ()>> {
        self.official_image(&entry.sha256, catalog, entry, |catalog, entry| {
            let package = catalog.load(entry).ok()?;
            let is_material = package.manifest.is_material();
            Some(OfficialImage { png: package.cover?, is_material })
        })
        .map(|result| result.map(|image| image.png))
    }

    pub(super) fn official_cover_is_material(&self, entry: &market::Entry) -> Option<bool> {
        let image = self.official_market.images.get(&entry.sha256)?.as_ref()?;
        Some(image.is_material)
    }

    pub(super) fn official_avatar(
        &mut self,
        catalog: &Arc<market::Catalog>,
        entry: &market::Entry,
    ) -> Option<std::result::Result<CoverPng, ()>> {
        let Some(name) = entry.avatar_name().filter(|_| catalog.is_official()) else {
            return Some(Err(()));
        };
        self.official_image(name, catalog, entry, |catalog, entry| {
            let png = catalog.avatar_png(entry.avatar_name()?)?;
            Some(OfficialImage { png, is_material: false })
        })
        .map(|result| result.map(|image| image.png))
    }

    fn official_image(
        &mut self,
        key: &str,
        catalog: &Arc<market::Catalog>,
        entry: &market::Entry,
        load: fn(&market::Catalog, &market::Entry) -> Option<OfficialImage>,
    ) -> Option<std::result::Result<OfficialImage, ()>> {
        let official = &mut self.official_market;
        if let Some(image) = official.images.get(key) {
            return Some(image.clone().ok_or(()));
        }
        if official.image_job.is_none() {
            let (catalog, entry) = (Arc::clone(catalog), entry.clone());
            official.image_job = Some((
                key.to_owned(),
                std::thread::spawn(move || load(&catalog, &entry)),
            ));
        }
        None
    }

    pub(super) fn market_status(
        &self,
        catalog: &market::Catalog,
        entry: &market::Entry,
    ) -> MarketStatus {
        if entry.revokes(&entry.version) {
            return MarketStatus::Revoked;
        }
        let Some(record) = self.records.get(&entry.id) else {
            return MarketStatus::Install;
        };
        if !record.from_market
            || record.official_market != catalog.is_official()
            || record.origin != catalog.path
        {
            return MarketStatus::OtherSource;
        }
        let Some(package) = &record.package else {
            return MarketStatus::Install;
        };
        match market::compare_versions(&entry.version, &package.manifest.version) {
            Some(Ordering::Greater) => MarketStatus::Update,
            Some(Ordering::Equal) => MarketStatus::Installed,
            _ => MarketStatus::Unavailable,
        }
    }

    pub(super) fn market_busy(&self) -> bool {
        self.market_job.is_some()
    }

    pub(super) fn market_job_id(&self) -> Option<&str> {
        self.market_job.as_ref().map(|job| job.id.as_str())
    }

    pub(super) fn market_pending(&self) -> bool {
        self.market_busy()
            || self.official_catalog_busy()
            || self.official_market.image_job.is_some()
            || self.market_metrics.busy()
    }

    pub(super) fn cancel_market(&mut self, id: &str) {
        if let Some(job) = self.market_job.as_mut().filter(|job| job.id == id) {
            job.cancelled = true;
        }
    }

    pub(super) fn market_action(&mut self, official: bool, id: &str) -> Result<()> {
        if self.market_busy() {
            return Err(fail("请等待当前集市操作完成"));
        }
        let catalog = self
            .market_catalog(official)
            .ok_or_else(|| fail("集市目录尚未加载"))?;
        let entry = catalog
            .entries
            .iter()
            .find(|entry| entry.id == id)
            .ok_or_else(|| fail("集市没有这个插件"))?;
        let update = match self.market_status(&catalog, entry) {
            MarketStatus::Install => false,
            MarketStatus::Update => true,
            MarketStatus::Installed | MarketStatus::Unavailable => {
                return Err(fail("集市没有更高版本"));
            }
            MarketStatus::OtherSource => return Err(fail("请从原来源管理这个插件")),
            MarketStatus::Revoked => return Err(fail("此插件版本已撤回")),
        };
        let record = if let Some(record) = self.records.get(id) {
            record.clone()
        } else {
            if self.path(id).exists() && !self.invalid.contains(id) {
                return Err(fail("已有无法读取的插件记录"));
            }
            Record {
                origin: catalog.path.clone(),
                from_market: true,
                official_market: official,
                presentation: None,
                package: None,
                enabled: false,
                values: Values::new(),
                storage: Value::Null,
                material_granted: false,
                source_grant: None,
                source_refresh_seconds: None,
                market_revoked: false,
                status_page_disabled: false,
                status_page_settings_hidden: false,
                notice_history: NoticeHistory::default(),
            }
        };
        let entry = entry.clone();
        self.market_job = Some(MarketJob {
            id: id.to_owned(),
            update,
            cancelled: false,
            result: std::thread::spawn(move || {
                let package = match catalog.load(&entry) {
                    Ok(package) => package,
                    Err(error) => return (None, Err(error.to_string())),
                };
                let version = catalog.is_official().then_some(entry.version);
                let mut record = record;
                record.presentation = entry.presentation.filter(|_| official);
                let result = prepare_record(record, package).map_err(|error| error.to_string());
                (version, result)
            }),
        });
        Ok(())
    }

    pub(super) fn poll_market(&mut self) {
        self.poll_official_market();
        if self.market_metrics.poll() {
            self.sync_market_favorites();
        }
        if !self
            .market_job
            .as_ref()
            .is_some_and(|job| job.result.is_finished())
        {
            return;
        }
        let job = self.market_job.take().unwrap();
        let (verified_version, result) = job
            .result
            .join()
            .unwrap_or_else(|_| (None, Err("集市校验线程退出".into())));
        if let Some(version) = verified_version {
            self.market_metrics.record_download(&job.id, version);
        }
        if job.cancelled {
            self.notice = "插件状态已变化，请重新操作".into();
            return;
        }
        let result = result.and_then(|mut record| {
            let version = &record.package.as_ref().unwrap().manifest.version;
            if self.market_catalog(record.official_market).is_some_and(|catalog| {
                catalog.path == record.origin && catalog.entries.iter().any(|entry| {
                    entry.id == job.id && entry.revokes(version)
                })
            }) {
                return Err("此插件版本已撤回".into());
            }
            // 撤回后更新时，等待用户重新启用材质。
            #[cfg(feature = "sdk-material")]
            if self.selected_material.as_deref() == Some(job.id.as_str())
                && self.records.get(&job.id).is_some_and(|current| current.market_revoked)
            {
                self.activate_material(None, false).map_err(|error| error.to_string())?;
            }
            record.market_revoked = false;
            let mut previous_secret = None;
            // 校验期间，旧插件可能完成一次后台刷新，保留该次已保存的数据。
            if let Some(current) = self.records.get(&job.id) {
                record.storage = current.storage.clone();
                record.notice_history = current.notice_history.clone();
                record.enabled = current.enabled;
                record.source_grant = current.source_grant().cloned();
                record.source_refresh_seconds = current.source_refresh_seconds;
                let scope_changed = match (current.network(), record.network()) {
                    (Some(before), Some(after)) => !before.same_authorization(after),
                    (None, None) => false,
                    _ => true,
                };
                let old_credential = current.network().and_then(|network| network.credential.as_ref());
                if record.source_grant().is_none() || scope_changed {
                    if old_credential.is_some() {
                        previous_secret = source_https::load_secret(&job.id).map_err(|error| error.to_string())?;
                        source_https::delete_secret(&job.id).map_err(|error| error.to_string())?;
                    }
                    record.source_grant = None;
                }
            }
            if let Err(error) = self.put(&job.id, record) {
                if let Some(secret) = previous_secret {
                    source_https::save_secret(&job.id, &secret).map_err(|error| error.to_string())?;
                }
                return Err(error.to_string());
            }
            self.invalid.remove(&job.id);
            self.forget_view(&job.id);
            Ok(())
        });
        self.notice = match result {
            Ok(()) if job.update => "已更新，设置和数据已保留".into(),
            Ok(()) => "已安装。启用后开始运行".into(),
            Err(error) if job.update => format!("更新失败，原版本仍可用。{error}"),
            Err(error) => format!("安装失败。{error}"),
        };
    }

    fn poll_official_market(&mut self) {
        let official = &mut self.official_market;
        let mut synced = false;
        if official
            .sync_job
            .as_ref()
            .is_some_and(JoinHandle::is_finished)
        {
            let job = official.sync_job.take().unwrap();
            match job
                .join()
                .unwrap_or_else(|_| Err("同步集市时出现错误".into()))
            {
                Ok(catalog) => {
                    official.images.retain(|key, _| catalog.has_image(key));
                    official.catalog = Some(Arc::new(catalog));
                    synced = true;
                }
                Err(error) => official.error = Some(error),
            }
        }
        if official
            .image_job
            .as_ref()
            .is_some_and(|(_, job)| job.is_finished())
        {
            let (key, job) = official.image_job.take().unwrap();
            if official
                .catalog
                .as_ref()
                .is_some_and(|catalog| catalog.has_image(&key))
            {
                official.images.insert(key, job.join().unwrap_or(None));
            }
        }
        if synced {
            if let Some(catalog) = official.catalog.clone() {
                self.apply_market_revocations(&catalog);
            }
            self.sync_market_favorites();
        }
    }
}

pub(super) fn prepare_record(mut record: Record, package: Package) -> Result<Record> {
    let manifest = &package.manifest;
    if let Some(old) = &record.package {
        let old = &old.manifest;
        let lunar_relocation = old.is_retired_lunar_preview()
            && manifest.id == old.id
            && manifest.version == "1.0.1"
            && manifest.owner == "calendar";
        if old.runtime != manifest.runtime
            || (old.owner != manifest.owner && !lunar_relocation)
            || old.permissions != manifest.permissions
            || old.data_version != manifest.data_version
            || old
                .capabilities
                .iter()
                .any(|capability| !manifest.capabilities.contains(capability))
        {
            return Err(fail("新版本与已安装版本的能力、权限或数据格式不兼容"));
        }
        let lyrics_migration = record
            .package
            .as_ref()
            .and_then(|old_pkg| old_pkg.migrate_lyrics_style_values(&package, &record.values));
        if lyrics_migration.is_none()
            && !old.settings.iter().all(|previous| {
                manifest
                    .settings
                    .iter()
                    .any(|field| field.key == previous.key && field.kind == previous.kind)
            })
        {
            return Err(fail("新版本删除或更改了已设置的字段类型"));
        }
    }
    let values = if let Some(migrated) = record
        .package
        .as_ref()
        .and_then(|old_pkg| old_pkg.migrate_lyrics_style_values(&package, &record.values))
    {
        migrated
    } else {
        let values = package.settings_values(&record.values);
        if record
            .values
            .iter()
            .any(|(key, value)| values.get(key) != Some(value))
        {
            return Err(fail("新版本与已保存的设置不兼容"));
        }
        values
    };
    if manifest.runtime == "js" {
        package.render(
            &values,
            &record.storage,
            &chrono::Local::now().date_naive().to_string(),
        )?;
    }
    record.values = values;
    record.package = Some(package);
    Ok(record)
}
