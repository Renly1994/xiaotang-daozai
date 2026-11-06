//! 【新增】按接口缓存及调度电量读取；2.4G 默认 5 秒，其余 HID 保持 60 秒。
use super::{Device, Sample};
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

pub const DEFAULT_REFRESH_SECS: u32 = 5;
pub const MAX_REFRESH_SECS: u32 = 3600;
pub fn refresh_seconds(seconds: u32) -> u32 {
    seconds.clamp(1, MAX_REFRESH_SECS)
}

struct Entry {
    at: Instant,
    sampled_at: u64,
    wireless: bool,
    devices: Vec<Device>,
}
#[derive(Default)]
pub(super) struct Polling {
    intervals: BTreeMap<String, u32>,
    entries: BTreeMap<String, Entry>,
}
impl Polling {
    pub fn configure(&mut self, intervals: BTreeMap<String, u32>) -> bool {
        let intervals = intervals
            .into_iter()
            .map(|(id, s)| (id, refresh_seconds(s)))
            .collect();
        if self.intervals == intervals {
            return false;
        }
        self.intervals = intervals;
        true
    }
    pub fn interval(&self, id: &str, wireless: bool) -> u32 {
        if wireless {
            self.intervals
                .get(&format!("hid:{id}"))
                .copied()
                .unwrap_or(DEFAULT_REFRESH_SECS)
        } else {
            60
        }
    }
    pub fn due(&self, now: Instant) -> bool {
        self.entries.iter().any(|(id, e)| {
            now.saturating_duration_since(e.at)
                >= Duration::from_secs(self.interval(id, e.wireless).into())
        })
    }
    pub fn cached(
        &self,
        id: &str,
        wireless: bool,
        now: Instant,
        force: bool,
    ) -> Option<(Vec<Device>, Sample)> {
        let e = self.entries.get(id)?;
        let interval = self.interval(id, wireless);
        if force || now.saturating_duration_since(e.at) >= Duration::from_secs(interval.into()) {
            return None;
        }
        Some((
            e.devices.clone(),
            Sample {
                collected_at: e.sampled_at,
                interval_seconds: interval,
            },
        ))
    }
    pub fn store(
        &mut self,
        id: String,
        wireless: bool,
        devices: Vec<Device>,
        now: Instant,
        sampled_at: u64,
    ) -> Sample {
        let interval_seconds = self.interval(&id, wireless);
        self.entries.insert(
            id,
            Entry {
                at: now,
                sampled_at,
                wireless,
                devices,
            },
        );
        Sample {
            collected_at: sampled_at,
            interval_seconds,
        }
    }
    pub fn retain(&mut self, present: &[String]) {
        self.entries.retain(|id, _| present.contains(id));
    }
    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn device(id: &str) -> Device {
        Device {
            id: id.into(),
            name: "鼠标".into(),
            descriptor: Default::default(),
            protocol: "compx-17",
            transport: "2.4G",
            slot: None,
            battery: None,
            failure: None,
        }
    }
    #[test]
    fn independent_intervals_manual_refresh_and_actual_sample_age() {
        let now = Instant::now();
        let mut p = Polling::default();
        p.configure(BTreeMap::from([("hid:slow".into(), 30)]));
        p.store("fast".into(), true, vec![device("fast")], now, 100);
        p.store("slow".into(), true, vec![device("slow")], now, 100);
        assert!(!p.due(now + Duration::from_secs(4)));
        assert!(p.due(now + Duration::from_secs(5)));
        assert!(p
            .cached("fast", true, now + Duration::from_secs(5), false)
            .is_none());
        let (_, sample) = p
            .cached("slow", true, now + Duration::from_secs(5), false)
            .unwrap();
        assert_eq!(sample.collected_at, 100);
        assert_eq!(sample.interval_seconds, 30);
        assert!(p
            .cached("slow", true, now + Duration::from_secs(5), true)
            .is_none());
        assert_eq!(p.interval("standard", false), 60);
    }
    #[test]
    fn custom_intervals_change_live_and_disconnected_entries_are_removed() {
        let now = Instant::now();
        let mut p = Polling::default();
        p.store("a".into(), true, vec![device("a")], now, 100);
        p.configure(BTreeMap::from([("hid:a".into(), 7)]));
        assert!(!p.due(now + Duration::from_secs(6)));
        assert!(p.due(now + Duration::from_secs(7)));
        p.configure(BTreeMap::from([("hid:a".into(), 3600)]));
        assert!(!p.due(now + Duration::from_secs(20)));
        p.retain(&[]);
        assert!(!p.due(now + Duration::from_secs(4000)));
        assert_eq!(refresh_seconds(0), 1);
        assert_eq!(refresh_seconds(u32::MAX), 3600);
    }
}
