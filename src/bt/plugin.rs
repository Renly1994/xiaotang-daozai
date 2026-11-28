//! Native presentation for bounded external device providers.
//! Only consumes authorized SDK snapshots and the existing USB worker's inventory.
use super::{BatteryReading,BatterySource,Device,DeviceKind};
use crate::sdk_spike::local::PluginWirelessDevice;
use chrono::{DateTime,Utc};
use std::{collections::BTreeSet,time::{Duration,Instant}};

const KEEP_FOR:Duration=Duration::from_secs(600);
const MAX_DEVICES:usize=64;

fn age(value:Option<&str>,now:DateTime<Utc>)->Option<Duration> {
    let at=DateTime::parse_from_rfc3339(value?).ok()?;
    let millis=now.signed_duration_since(at).num_milliseconds();
    if millis < -5_000 {return None;}
    Some(Duration::from_millis(millis.max(0) as u64))
}
fn kind(value:&str)->Option<DeviceKind> {Some(match value {
    "mouse"=>DeviceKind::Mouse,"keyboard"=>DeviceKind::Keyboard,
    "gamepad"=>DeviceKind::Gamepad,"headset"=>DeviceKind::Headphones,_=>return None,
})}
fn valid_text(value:&str,max:usize)->bool {
    !value.trim().is_empty()&&value.chars().count()<=max&&!value.chars().any(char::is_control)
}
fn usb_hardware(instance:&str)->Option<&str> {
    let mut parts=instance.split('\\');
    if !parts.next()?.eq_ignore_ascii_case("USB") {return None;}
    let hardware=parts.next()?;
    if parts.next()?.is_empty()||parts.next().is_some() {return None;}
    Some(hardware)
}
fn physical_instance<'a>(record:&PluginWirelessDevice,instances:&'a [String])->Option<&'a str> {
    let hardware=format!("VID_{:04X}&PID_{:04X}",record.vendor_id,record.product_id);
    let mut matches=instances.iter().filter(|instance| {
        usb_hardware(instance).is_some_and(|found|found.eq_ignore_ascii_case(&hardware))
            &&record.usb_instance.as_ref().is_none_or(|exact|exact.eq_ignore_ascii_case(instance))
    });
    let first=matches.next()?;
    matches.next().is_none().then_some(first.as_str())
}
fn native_instance(device:&Device)->Option<&str> {
    device.id.strip_prefix("usb:").map(|id|id.split('#').next().unwrap_or(id))
}
fn same_model(a:&str,b:&str)->bool {
    let clean=|name:&str|name.chars().filter(|c|c.is_alphanumeric()).flat_map(char::to_lowercase).collect::<String>();
    let (a,b)=(clean(a),clean(b));
    a.chars().count().min(b.chars().count())>=5&&(a==b||a.contains(&b)||b.contains(&a))
}

/// Called on UI snapshots, so source age expires even between 60-second JS refreshes.
/// Each logical device remains in its plugin namespace. Never borrow a reading
/// from an ambiguous shared receiver merely because its VID/PID matches.
pub(crate) fn merge<'a>(devices:&mut Vec<Device>,sources:impl IntoIterator<Item=(&'a str,&'a str,&'a [PluginWirelessDevice])>,
    instances:&[String],utc:DateTime<Utc>,now:Instant) {
    // The currently authorized sources are authoritative, including an empty
    // set after unload or grant revocation. Never retain a previous plugin card.
    devices.retain(|device|!device.id.starts_with("plugin:"));
    let native_count=devices.len();
    let mut hidden=BTreeSet::new();
    let mut added=Vec::new();
    let mut ids=BTreeSet::new();
    for (plugin_id,plugin_name,records) in sources {
        if !valid_text(plugin_id,128)||!valid_text(plugin_name,80)||records.len()>MAX_DEVICES {continue;}
        for record in records {
            if added.len()>=MAX_DEVICES {break;}
            let Some(kind)=kind(&record.kind) else {continue};
            if !valid_text(&record.id,128)||!valid_text(&record.name,80)||record.vendor_id==0||record.product_id==0
                ||record.percent.is_some_and(|level|level>100)||!(30..=180).contains(&record.fresh_for_seconds) {continue;}
            let id=format!("plugin:{plugin_id}:{}",record.id);
            if !ids.insert(id.clone()) {continue;}
            let budget=Duration::from_secs(record.fresh_for_seconds as u64);
            let connection_age=age(record.connection_checked_at_utc.as_deref(),utc);
            let measurement_age=age(record.measured_at_utc.as_deref(),utc);
            // Keep a clearly marked recent history, not permanent entries from a disk cache.
            if ![connection_age,measurement_age].into_iter().flatten().any(|age|age<=KEEP_FOR) {continue;}
            let instance=physical_instance(record,instances);
            let connected=instance.is_some()&&record.online&&connection_age.is_some_and(|age|age<=budget);
            let reading=record.percent.zip(measurement_age).filter(|(_,age)|*age<=KEEP_FOR).and_then(|(percent,age)| {
                let observed=now.checked_sub(age)?;
                let charging=if connected&&age<=budget
                    &&self::age(record.charging_checked_at_utc.as_deref(),utc).is_some_and(|age|age<=budget) {
                    record.charging
                } else {None};
                let mut reading=BatteryReading::new(percent,BatterySource::Plugin,observed,charging);
                reading.fresh=connected&&record.fresh&&age<=budget;
                reading.fresh_for=budget;
                reading.approximate=record.approximate;
                reading.plugin_label=Some(plugin_name.to_owned());
                Some(reading)
            });
            let mut output=Device{id,name:record.name.clone(),kind,connected,
                battery:reading.as_ref().map(|reading|reading.percent),reading,charging_only:None};
            if let Some(instance)=instance {
                let candidates:Vec<_>=devices[..native_count].iter().enumerate().filter(|(index,native)| {
                    !hidden.contains(index)&&native_instance(native).is_some_and(|native|native.eq_ignore_ascii_case(instance))
                        &&native.connected&&!native.stale_at(now)&&native.reading.is_some()
                        &&native.reading.as_ref().is_some_and(|r|r.source!=BatterySource::Plugin)
                        &&(same_model(&native.name,&record.name)||native.kind==kind)
                }).collect();
                if candidates.len()==1 {
                    let (index,native)=candidates[0];
                    output.connected=true;
                    output.battery=native.battery;
                    output.reading=native.reading.clone();
                    output.charging_only=native.charging_only;
                    hidden.insert(index);
                }
                if output.connected {
                    for (index,native) in devices[..native_count].iter().enumerate() {
                        if native.kind==DeviceKind::Receiver24G&&native.reading.is_none()&&native.charging_only.is_none()
                            &&native_instance(native).is_some_and(|native|native.eq_ignore_ascii_case(instance)) {
                            hidden.insert(index);
                        }
                    }
                }
            }
            added.push(output);
        }
    }
    let mut index=0;
    devices.retain(|_|{let keep=!hidden.contains(&index);index+=1;keep});
    devices.extend(added);
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn fixture()->(PluginWirelessDevice,Vec<String>,DateTime<Utc>,Instant) {
        let utc="2026-11-28T04:00:00Z".parse().unwrap();
        let record=serde_json::from_value(json!({"id":"keyboard-1","name":"Example Wireless Keyboard","kind":"keyboard",
            "vendorId":2821,"productId":6862,"percent":95,"online":true,"fresh":true,"approximate":true,
            "measuredAtUtc":"2026-11-28T04:00:00Z","connectionCheckedAtUtc":"2026-11-28T04:00:00Z",
            "freshForSeconds":150})).unwrap();
        (record,vec![r"USB\VID_0B05&PID_1ACE\RECEIVER".into()],utc,Instant::now())
    }
    fn snapshot(record:&PluginWirelessDevice,instances:&[String],utc:DateTime<Utc>,now:Instant)->Vec<Device> {
        let mut devices=vec![];merge(&mut devices,[("example.battery","Example source",std::slice::from_ref(record))],instances,utc,now);devices
    }
    #[test]
    fn plugin_source_preserves_identity_source_time_estimate_and_unknown_charge() {
        let (record,instances,utc,now)=fixture();let devices=snapshot(&record,&instances,utc,now);
        let device=&devices[0];assert_eq!(device.id,"plugin:example.battery:keyboard-1");
        assert!(device.connected&&!device.stale_at(now));assert_eq!(device.kind,DeviceKind::Keyboard);
        let r=device.reading.as_ref().unwrap();assert_eq!(r.observed,now);assert_eq!(r.charging,None);
        assert!(r.approximate);assert_eq!(r.label(),"Example source");assert_eq!(r.source,BatterySource::Plugin);
    }
    #[test]
    fn plugin_explicit_cache_and_source_expiry_are_never_renewed() {
        let (mut record,instances,utc,now)=fixture();record.fresh=false;
        assert!(snapshot(&record,&instances,utc,now)[0].stale_at(now));record.fresh=true;
        record.measured_at_utc=Some("2026-11-28T03:57:29Z".into());
        let devices=snapshot(&record,&instances,utc,now);assert!(devices[0].connected&&devices[0].stale_at(now));
        assert_eq!(devices[0].reading.as_ref().unwrap().observed,now-Duration::from_secs(151));
        record.measured_at_utc=Some("2026-11-28T03:49:59Z".into());
        assert!(snapshot(&record,&instances,utc,now)[0].reading.is_none());
    }
    #[test]
    fn plugin_unplug_and_ambiguous_receiver_invalidate_connection() {
        let (mut record,instances,utc,now)=fixture();assert!(!snapshot(&record,&[],utc,now)[0].connected);
        let mut two=instances.clone();two.push(r"USB\VID_0B05&PID_1ACE\SECOND".into());
        assert!(!snapshot(&record,&two,utc,now)[0].connected);
        record.usb_instance=Some(instances[0].clone());assert!(snapshot(&record,&two,utc,now)[0].connected);
        record.usb_instance=Some(r"USB\VID_0B05&PID_1ACE&MI_00\INTERFACE".into());
        assert!(!snapshot(&record,&instances,utc,now)[0].connected);
    }
    #[test]
    fn plugin_invalid_and_future_measurements_do_not_become_percentages() {
        let (mut record,instances,utc,now)=fixture();record.percent=Some(101);
        assert!(snapshot(&record,&instances,utc,now).is_empty());record.percent=Some(95);
        record.measured_at_utc=Some("2026-11-28T04:00:06Z".into());
        assert!(snapshot(&record,&instances,utc,now)[0].reading.is_none());
    }
    #[test]
    fn plugin_uses_fresh_native_reading_only_for_a_proven_same_model() {
        let (record,instances,utc,now)=fixture();
        let native=Device{id:format!("usb:{}#device-a",instances[0]),name:record.name.clone(),kind:DeviceKind::Receiver24G,
            battery:Some(91),reading:Some(BatteryReading::new(91,BatterySource::UsbReceiver,now,Some(false))),..Default::default()};
        let mut devices=vec![native];merge(&mut devices,[("example.battery","Example source",std::slice::from_ref(&record))],&instances,utc,now);
        assert_eq!(devices.len(),1);assert_eq!(devices[0].id,"plugin:example.battery:keyboard-1");
        assert_eq!(devices[0].battery,Some(91));assert_eq!(devices[0].reading.as_ref().unwrap().source,BatterySource::UsbReceiver);
        assert_eq!(devices[0].kind,DeviceKind::Keyboard);
    }
    #[test]
    fn plugin_shared_receiver_does_not_borrow_an_ambiguous_native_level() {
        let (record,instances,utc,now)=fixture();
        let native=Device{id:format!("usb:{}",instances[0]),name:"Example shared receiver".into(),kind:DeviceKind::Receiver24G,
            battery:Some(45),reading:Some(BatteryReading::new(45,BatterySource::UsbReceiver,now,None)),..Default::default()};
        let mut devices=vec![native];merge(&mut devices,[("example.battery","Example source",std::slice::from_ref(&record))],&instances,utc,now);
        assert_eq!(devices.len(),2);assert_eq!(devices[1].battery,Some(95));assert_eq!(devices[1].reading.as_ref().unwrap().source,BatterySource::Plugin);
    }
    #[test]
    fn plugin_snapshot_removed_on_disable_does_not_leave_native_cards() {
        let (record,instances,utc,now)=fixture();let mut devices=snapshot(&record,&instances,utc,now);
        assert_eq!(devices.len(),1);
        merge(&mut devices,std::iter::empty::<(&str,&str,&[PluginWirelessDevice])>(),&instances,utc,now);
        assert!(devices.is_empty());
    }
    #[test]
    fn plugin_shared_receiver_keeps_distinct_logical_devices() {
        let (keyboard,instances,utc,now)=fixture();let mut mouse=keyboard.clone();
        mouse.id="mouse-1".into();mouse.name="Example Wireless Mouse".into();mouse.kind="mouse".into();
        let records=[keyboard,mouse];let mut devices=vec![];
        merge(&mut devices,[("example.battery","Example source",records.as_slice())],&instances,utc,now);
        assert_eq!(devices.len(),2);assert_ne!(devices[0].id,devices[1].id);
        assert!(devices.iter().all(|device|device.connected&&!device.stale_at(now)));
    }
    #[test]
    fn usb_inventory_expires_and_clears_without_extra_hardware_queries() {
        let (_,instances,_,now)=fixture();let store=super::super::compat::Store::default();
        store.set_usb_instances(instances.clone(),now);assert_eq!(store.usb_instances_at(now),instances);
        assert!(store.usb_instances_at(now+Duration::from_secs(7)).is_empty());
        store.clear_usb_instances();assert!(store.usb_instances_at(now).is_empty());
    }
}
