use super::*;
use crate::{bt::{Device,DeviceKind,BatteryReading,BatterySource},app::BLUETOOTH_PAGE};

fn fixture()->Vec<Device> {
    [("耳机",DeviceKind::Headphones,Some(82)),("MX Master 3S",DeviceKind::Mouse,Some(18)),("MX Keys Mini",DeviceKind::Keyboard,Some(64)),("桌面音箱",DeviceKind::Speaker,None)].into_iter().enumerate().map(|(i,(name,kind,battery))|Device{
        id:format!("device-{i}"),name:name.into(),kind,battery,reading:battery.map(|percent|BatteryReading::new(percent,BatterySource::Gatt,std::time::Instant::now(),None)),..Default::default()
    }).collect()
}
fn receiver_fixture()->Device {
    Device{id:"usb:test".into(),name:"罗技 Bolt 接收器".into(),kind:DeviceKind::Receiver24G,..Default::default()}
}

fn powered_receiver_fixture()->Device {
    Device{
        name:"VXE R1 系列无线接收器".into(),
        battery:Some(72),
        reading:Some(BatteryReading::new(72,BatterySource::UsbReceiver,std::time::Instant::now(),Some(true))),
        ..receiver_fixture()
    }
}
fn texts(h:&H)->Vec<String> {h.shapes.iter().filter_map(|s|if let egui::Shape::Text(t)=&s.shape{Some(t.galley.text().to_string())}else{None}).collect()}
fn open_page(h:&mut H) {h.app.show_page(BLUETOOTH_PAGE);h.hover(H::local(200.0,100.0),1.0);}
fn set_level(d:&mut Device,level:u8) {d.battery=Some(level);d.reading.as_mut().unwrap().percent=level;}

#[cfg(all(windows,feature="sdk-js"))]
fn plugin_fixture(h:&mut H,cached:bool) {
    let now=chrono::Utc::now().to_rfc3339();
    crate::sdk_spike::local::wireless_source_tests::ui_provider(&mut h.app.plugins,serde_json::json!([
        {"id":"controller-1","name":"Test controller","kind":"gamepad","vendorId":1,"productId":2,
        "percent":83,"fresh":!cached,"online":true,"approximate":true,
        "measuredAtUtc":now,"connectionCheckedAtUtc":now}
    ])).unwrap();
}

#[test]
#[cfg(all(windows,feature="sdk-js"))]
fn plugin_wireless_uses_native_page_source_cache_and_custom_resident() {
    let _g=ENV_LOCK.lock().unwrap_or_else(|e|e.into_inner());let mut h=H::new();
    h.app.settings.bluetooth_notifications=false;h.app.settings.bluetooth_battery_notifications=false;
    plugin_fixture(&mut h,false);open_page(&mut h);
    assert_eq!(h.app.wireless_devices[0].id,"plugin:test.battery:controller-1");
    assert!(texts(&h).iter().any(|text|text=="≈83%"));
    assert!(texts(&h).iter().any(|text|text.contains("Test external source")));
    h.app.settings.bluetooth_resident_custom=true;h.app.settings.bluetooth_pinned=vec![h.app.wireless_devices[0].id.clone()];
    plugin_fixture(&mut h,true);h.frame(vec![]);
    assert!(texts(&h).iter().any(|text|text=="旧 ≈83%"));
    assert!(texts(&h).iter().any(|text|text.starts_with("缓存 · ")));
    assert_eq!(h.app.wireless_resident().len(),1,"explicit selections retain marked cache while connected");
    h.app.settings.bluetooth_resident_custom=false;
    assert!(h.app.wireless_resident().is_empty(),"automatic selection omits cached external values");
    crate::sdk_spike::local::wireless_source_tests::ui_provider(&mut h.app.plugins,serde_json::json!([])).unwrap();h.frame(vec![]);
    assert!(h.app.wireless_devices.is_empty());assert!(h.app.wireless_resident().is_empty());
}

#[cfg(all(windows,feature="sdk-js"))]
fn congested_wireless_fixture(cached:bool)->H {
    let mut h=H::new();
    h.app.settings.visible_pages.fill(true);
    h.app.settings.show_clock=true;
    h.app.settings.clock_period=2;
    h.app.settings.clock_seconds=true;
    h.app.settings.show_weather=true;
    h.app.settings.show_performance=true;
    h.app.settings.performance_bar=crate::performance::BarStyle::Gauges;
    h.app.settings.pet=true;
    h.app.settings.browser_minimal=true;
    h.app.settings.bluetooth_notifications=false;
    h.app.settings.bluetooth_battery_notifications=false;
    h.app.settings.bluetooth_resident_custom=true;
    h.app.settings.bluetooth_resident_style=0;
    h.app.settings.show_bluetooth=true;
    h.app.water.remind=true;
    h.app.settings.sit_remind=true;
    h.app.health_pending=[true,true];
    h.app.weather.snapshot=Some(crate::weather::Snapshot{
        place:"深圳".into(),temp:16.0,day:true,at:chrono::Utc::now().timestamp(),..Default::default()
    });
    h.app.quota.config.enabled=true;
    h.app.quota.config.compact=true;
    h.quota_accounts(2);
    h.app.quota.config.accounts[1].provider=crate::quota::Provider::DeepSeek;
    h.app.quota_event(crate::quota::Event{id:"preview-1".into(),result:Ok(crate::quota::parse(
        crate::quota::Provider::DeepSeek,&serde_json::json!({"is_available":true,"balance_infos":[{
            "currency":"CNY","total_balance":"3.8","granted_balance":"0","topped_up_balance":"3.8"
        }]})).unwrap())});
    let now=chrono::Utc::now().to_rfc3339();
    let records:Vec<_>=[("mouse","Test Wireless Mouse","mouse",84),
        ("keyboard","Test Wireless Keyboard","keyboard",97)]
        .into_iter().map(|(id,name,kind,percent)|{
            serde_json::json!({"id":id,"name":name,"kind":kind,"vendorId":1,"productId":2,
                "percent":percent,"online":true,"fresh":!cached,"measuredAtUtc":now,"connectionCheckedAtUtc":now})
        }).collect();
    crate::sdk_spike::local::wireless_source_tests::ui_provider(&mut h.app.plugins,serde_json::json!(records)).unwrap();
    h.app.settings.bluetooth_pinned=vec!["plugin:test.battery:mouse".into(),"plugin:test.battery:keyboard".into()];
    h
}

fn congested_wireless_frame(h:&mut H,width:f32,monitor_width:f32)->Rect {
    congested_wireless_frame_at_browser(h,width,monitor_width,false)
}

fn congested_wireless_frame_at_browser(h:&mut H,width:f32,monitor_width:f32,browser_at_top:bool)->Rect {
    h.t+=1.0/60.0;
    let screen=Rect::from_min_size(egui::pos2((width-monitor_width)/2.0,0.0),egui::vec2(monitor_width,1080.0));
    let env=crate::app::Env{t:h.t,clock:"21:48:00".into(),today:"2026-11-25".into(),date:"11月25日 周三".into(),
        bt_ready:true,receivers:h.receivers.clone(),screen:Some(screen),browser_at_top:Some(browser_at_top),..Default::default()};
    let raw=egui::RawInput{screen_rect:Some(Rect::from_min_size(Pos2::ZERO,egui::vec2(width+320.0,WIN_H))),time:Some(h.t),focused:true,..Default::default()};
    let app=&mut h.app;
    let output=h.ctx.run(raw,|ctx|{egui::CentralPanel::default().frame(egui::Frame::new()).show(ctx,|ui|{
        app.frame(ui,Rect::from_min_size(ui.max_rect().min,egui::vec2(width,WIN_H)),&env);
    });});
    h.shapes=output.shapes;
    screen
}

#[test]
#[cfg(all(windows,feature="sdk-js"))]
fn wireless_selected_devices_survive_busy_resident_bar_and_cached_labels() {
    let _g=ENV_LOCK.lock().unwrap_or_else(|e|e.into_inner());
    for cached in [false,true] {
        let mut h=congested_wireless_fixture(cached);
        let mut width=WIN_W;
        let mut screen=Rect::NOTHING;
        for _ in 0..90 {
            screen=congested_wireless_frame(&mut h,width,1920.0);
            width=((h.app.requested_horizontal_width().max(width)/64.0).ceil()*64.0).min(1920.0);
        }
        assert!(width>WIN_W,"native wireless selections must request the extra window room they need");
        assert!(h.app.resident.target.x[crate::bar_layout::BLUETOOTH].is_some(),"a crowded bar must not silently discard the selected device group");
        for level in [84,97] {
            let expected=format!("{}{level}%",if cached{"旧 "}else{""});
            assert!(h.shapes.iter().any(|shape|matches!(&shape.shape,egui::Shape::Text(text)
                if text.galley.text()==expected&&shape.clip_rect.contains(text.visual_bounding_rect().center())&&screen.contains(text.visual_bounding_rect().center()))),"selected battery must be visible: {expected}");
        }
        let layout=&h.app.resident.target;
        let mut boxes:Vec<_>=layout.x.iter().enumerate().filter_map(|(i,x)|x.map(|x|(x-layout.widths[i]/2.0,x+layout.widths[i]/2.0))).collect();
        boxes.sort_by(|a,b|a.0.total_cmp(&b.0));
        assert!(boxes.windows(2).all(|pair|pair[1].0-pair[0].1>=6.9),"wireless, quota, weather and performance slots cannot overlap");
    }
}

#[test]
#[cfg(all(windows,feature="sdk-js"))]
fn wireless_selected_devices_fit_a_narrow_monitor_without_being_removed() {
    let _g=ENV_LOCK.lock().unwrap_or_else(|e|e.into_inner());
    let mut h=congested_wireless_fixture(true);
    h.app.settings.performance_bar=crate::performance::BarStyle::Dashboard;
    let mut screen=Rect::NOTHING;
    for _ in 0..90 {screen=congested_wireless_frame(&mut h,WIN_W,WIN_W);}
    assert!(h.app.resident.target.width>h.app.resident_limit(),"fixture must exercise the constrained layout");
    assert!(h.app.requested_horizontal_width()<=WIN_W,"transparent-window expansion stays within the monitor");
    assert!(h.app.resident.target.x[crate::bar_layout::BLUETOOTH].is_some());
    for expected in ["旧 84%","旧 97%"] {
        assert!(h.shapes.iter().any(|shape|matches!(&shape.shape,egui::Shape::Text(text)
            if text.galley.text()==expected&&shape.clip_rect.contains(text.visual_bounding_rect().center())&&screen.contains(text.visual_bounding_rect().center()))),"fitted selected device must remain visible: {expected}");
    }
}

#[test]
#[cfg(all(windows,feature="sdk-js"))]
fn wireless_resident_respects_browser_collapse_and_returns_afterward() {
    let _g=ENV_LOCK.lock().unwrap_or_else(|e|e.into_inner());
    let mut h=congested_wireless_fixture(false);
    h.app.health_pending=[false,false];
    h.app.water.remind=false;
    h.app.settings.sit_remind=false;
    let pins=h.app.settings.bluetooth_pinned.clone();
    for _ in 0..90 {congested_wireless_frame_at_browser(&mut h,960.0,1920.0,true);}
    assert!(h.app.settings.browser_minimal,"the user's browser preference is preserved");
    assert!(h.app.effective_minimal(),"a browser at the top still activates the collapsed strip");
    assert!(!texts(&h).iter().any(|text|text=="84%"||text=="97%"),"the collapsed strip does not draw resident batteries");
    for _ in 0..90 {congested_wireless_frame_at_browser(&mut h,960.0,1920.0,false);}
    assert!(!h.app.effective_minimal());
    assert_eq!(h.app.settings.bluetooth_pinned,pins,"collapse cannot clear selected devices");
    for expected in ["84%","97%"] {
        assert!(texts(&h).iter().any(|text|text==expected),"the selected battery returns after leaving the browser: {expected}");
    }
}


#[test]
fn wireless_custom_resident_persists_from_detail_toggle(){
    let _g=ENV_LOCK.lock().unwrap_or_else(|e|e.into_inner());let mut h=H::new();h.bt=fixture();h.app.settings.bluetooth_notifications=false;
    h.app.settings.bluetooth_battery_notifications=false;open_page(&mut h);
    assert_eq!(h.app.wireless_devices.len(),4);assert!(h.app.notice.is_none());
    // 自动模式：只挑有电量读数的设备，电量低的在前，最多两台。
    assert_eq!(h.app.wireless_resident().iter().map(|d|d.id.as_str()).collect::<Vec<_>>(),["device-1","device-2"]);
    let tile=h.ctx.read_response(egui::Id::new(("card",true,("wireless-device","device-0")))).unwrap();
    h.click(tile.rect.center());h.frame(vec![]);
    let pin=h.ctx.read_response(egui::Id::new(("card",true,("wireless-pin","device-0")))).expect("详情应提供常驻显示开关");
    h.click(pin.rect.center());
    assert_eq!(h.app.wireless_focus.as_deref(),Some("device-0"));
    assert!(h.app.settings.bluetooth_resident_custom);
    assert_eq!(h.app.settings.bluetooth_pinned,vec!["device-0"]);
    let saved=crate::model::Settings::load();
    assert!(saved.bluetooth_resident_custom);assert_eq!(saved.bluetooth_pinned,vec!["device-0"]);
    assert_eq!(h.app.wireless_resident().iter().map(|d|d.id.as_str()).collect::<Vec<_>>(),["device-0"]);
    h.app.settings.show_bluetooth=false;h.frame(vec![]);assert!(h.app.wireless_resident().is_empty());
}


#[test]
fn wireless_auto_resident_can_be_turned_off_from_detail() {
    let _g=ENV_LOCK.lock().unwrap_or_else(|e|e.into_inner());
    let mut h=H::new();
    h.bt=fixture();h.app.settings.bluetooth_notifications=false;
    h.app.settings.bluetooth_battery_notifications=false;
    open_page(&mut h);
    assert_eq!(h.app.wireless_resident().iter().map(|d|d.id.as_str()).collect::<Vec<_>>(),["device-1","device-2"]);
    let tile=h.ctx.read_response(egui::Id::new(("card",true,("wireless-device","device-1")))).unwrap();
    h.click(tile.rect.center());h.frame(vec![]);
    let pin=h.ctx.read_response(egui::Id::new(("card",true,("wireless-pin","device-1")))).unwrap();
    h.click(pin.rect.center());
    assert_eq!(h.app.wireless_resident().iter().map(|d|d.id.as_str()).collect::<Vec<_>>(),["device-2"]);
    h.app.settings=crate::model::Settings::load();
    h.frame(vec![]);
    assert!(h.app.settings.bluetooth_resident_custom);
    assert_eq!(h.app.settings.bluetooth_pinned,["device-2"]);
    h.click(pin.rect.center());
    assert!(h.app.wireless_resident().iter().any(|d|d.id=="device-1"));
}

#[test]
fn wireless_pin_can_restore_resident_from_the_island() {
    let _g=ENV_LOCK.lock().unwrap_or_else(|e|e.into_inner());
    let mut h=H::new();
    h.bt=vec![fixture().remove(0)];
    h.app.settings.show_bluetooth=false;
    h.app.settings.bluetooth_notifications=false;
    open_page(&mut h);
    assert!(h.app.wireless_resident().is_empty());
    let pin=h.ctx.read_response(egui::Id::new(("card",true,("wireless-pin","device-0")))).unwrap();
    h.click(pin.rect.center());
    assert_eq!(h.app.wireless_resident()[0].id,"device-0");
    h.click(pin.rect.center());
    assert!(h.app.wireless_resident().is_empty());
}

#[test]
fn wireless_history_does_not_suppress_disconnect_notifications(){
    let _g=ENV_LOCK.lock().unwrap_or_else(|e|e.into_inner());let mut h=H::new();h.bt=fixture();h.frame(vec![]);
    h.bt[0].connected=false;h.t+=3.0;h.frame(vec![]);
    assert!(h.app.notice.as_ref().is_some_and(|a|a.bluetooth==Some((DeviceKind::Headphones,false))));
    assert_eq!(h.app.wireless_devices.len(),4);assert_eq!(h.app.bt_shown.len(),3);
}

#[test]
fn wireless_rings_show_unknown_compact_and_paging(){
    let _g=ENV_LOCK.lock().unwrap_or_else(|e|e.into_inner());let mut h=H::new();h.bt=fixture();h.app.settings.bluetooth_notifications=false;
    for i in 4..6 {let mut d=h.bt[1].clone();d.id=format!("device-{i}");d.name=format!("第 {i} 台");h.bt.push(d);}
    open_page(&mut h);
    let t=texts(&h);assert!(t.iter().any(|s|s=="···"),"未上报电量用小点占位");assert!(!t.iter().any(|s|s.contains(" / ")),"6 台以内一页放下");
    let boxes:Vec<_>=(0..6).map(|i|h.ctx.read_response(egui::Id::new(("card",true,("wireless-device",format!("device-{i}"))))).unwrap().rect).collect();
    assert!(boxes.windows(2).all(|p|(p[0].top()-p[1].top()).abs()<0.1&&p[0].right()<=p[1].left()+0.1),"紧凑排法仍在同一行且不重叠");
    for id in ["nav-l","nav-r"] {
        let arrow=h.ctx.read_response(egui::Id::new(("card",true,id))).unwrap().interact_rect;
        assert!(boxes.iter().all(|tile|!tile.intersect(arrow).is_positive()),"无线设备的点击区域不能盖住翻页箭头");
    }
    let mut d=h.bt[1].clone();d.id="device-6".into();h.bt.push(d);h.frame(vec![]);h.frame(vec![]);
    assert!(texts(&h).iter().any(|s|s=="1 / 2"),"7 台以上每页 4 台");
    h.click(H::local(348.0,175.0));assert_eq!(h.app.wireless_offset,1);
    h.bt.truncate(2);h.frame(vec![]);assert_eq!(h.app.wireless_offset,0);
}

#[test]
fn wireless_detail_explains_which_battery_data_is_missing() {
    let _g=ENV_LOCK.lock().unwrap_or_else(|e|e.into_inner());
    let mut h=H::new();
    h.bt=vec![fixture().remove(0)];
    h.app.settings.bluetooth_notifications=false;
    open_page(&mut h);
    assert!(texts(&h).iter().any(|s|s=="82%"));
    assert!(texts(&h).iter().any(|s|s=="充电状态未知"));

    h.bt[0].reading.as_mut().unwrap().charging=Some(false);
    h.frame(vec![]);
    assert!(!texts(&h).iter().any(|s|matches!(s.as_str(),"充电状态未知"|"充电中")));

    h.bt[0].battery=None;
    h.bt[0].reading=None;
    h.frame(vec![]);
    assert!(texts(&h).iter().any(|s|s=="正在读取电量"),"刚连上、还没读到电量时显示正在读取");
    assert!(!texts(&h).iter().any(|s|s=="充电状态未知"));
    // 超过读取宽限期仍没有电量：按已连接显示，不再是灰色的“正在读取”。
    h.t+=10.0;h.frame(vec![]);
    assert!(texts(&h).iter().any(|s|s=="正在读取电量"),"15 秒内仍按正在读取显示");
    h.t+=6.0;h.frame(vec![]);
    let t=texts(&h);
    assert!(t.iter().any(|s|s=="已连接")&&t.iter().any(|s|s=="设备没有提供电量"));
    assert!(!t.iter().any(|s|s=="正在读取电量"));
}

#[test]
fn wireless_receiver_shows_presence_and_stays_out_of_resident_controls() {
    let _g=ENV_LOCK.lock().unwrap_or_else(|e|e.into_inner());
    let mut h=H::new();
    h.bt=vec![fixture().remove(0)];
    h.app.settings.bluetooth_resident_custom=true;
    h.app.settings.bluetooth_pinned=vec!["usb:test".into(),"device-0".into()];
    h.receivers=vec![receiver_fixture()];
    open_page(&mut h);
    let t=texts(&h);
    assert!(t.iter().any(|s|s=="已连接")&&t.iter().any(|s|s=="2.4G"),"接收器一插上就是已连接，并带 2.4G 角标");
    assert!(!t.iter().any(|s|s=="···"));
    assert_eq!(h.app.wireless_resident().iter().map(|d|d.id.as_str()).collect::<Vec<_>>(),["device-0"]);

    let tile=h.ctx.read_response(egui::Id::new(("card",true,("wireless-device","usb:test")))).unwrap();
    h.click(tile.rect.center());
    h.frame(vec![]);
    assert_eq!(h.app.wireless_focus.as_deref(),Some("usb:test"));
    let labels=texts(&h);
    for label in ["罗技 Bolt 接收器","已连接","2.4G","2.4G 接收器 · 电量未知"] {
        assert!(labels.iter().any(|s|s==label),"缺少 {label}");
    }
    assert!(!labels.iter().any(|s|matches!(s.as_str(),"常驻显示"|"充电状态未知"|"正在读取电量")));

    h.t+=1.0/60.0;
    let out=h.ctx.run(egui::RawInput{time:Some(h.t),screen_rect:Some(Rect::from_min_size(Pos2::ZERO,egui::vec2(640.0,390.0))),..Default::default()},|ctx|{
        egui::CentralPanel::default().show(ctx,|ui|h.app.wireless_resident_settings(ui));
    });
    h.shapes=out.shapes;
    let settings_labels=texts(&h);
    assert!(settings_labels.iter().any(|s|s.contains("耳机 · 82%")));
    assert!(!settings_labels.iter().any(|s|s.contains("Bolt")));

    h.receivers.clear();
    h.frame(vec![]);
    assert!(h.app.wireless_focus.is_none());
    assert!(!texts(&h).iter().any(|s|s=="2.4G"));
}

#[test]
fn wireless_receiver_shows_valid_battery_and_falls_back_to_unknown() {
    let _g=ENV_LOCK.lock().unwrap_or_else(|e|e.into_inner());
    let mut h=H::new();
    h.receivers=vec![powered_receiver_fixture()];
    open_page(&mut h);
    assert!(texts(&h).iter().any(|s|s=="72%"));
    h.app.wireless_focus=Some("usb:test".into());
    h.frame(vec![]);
    let labels=texts(&h);
    for label in ["72%","充电中","2.4G 接收器"] {
        assert!(labels.iter().any(|s|s==label),"缺少 {label}");
    }
    h.receivers[0]=receiver_fixture();
    h.frame(vec![]);
    let labels=texts(&h);
    assert!(labels.iter().any(|s|s=="2.4G 接收器 · 电量未知"));
    assert!(!labels.iter().any(|s|s=="0%"||s=="72%"));
}

#[test]
fn wireless_page_tracks_receiver_extensions_without_losing_bluetooth_or_builtin_receivers() {
    let _g=ENV_LOCK.lock().unwrap_or_else(|e|e.into_inner());
    let mut h=H::new();
    let bluetooth=fixture().remove(0);
    let builtin=receiver_fixture();
    let extension=Device{id:"usb:extension".into(),name:"扩展接收器".into(),..builtin.clone()};
    h.bt=vec![bluetooth.clone()];
    h.receivers=vec![builtin.clone()];
    open_page(&mut h);
    let check=|h:&mut H,enabled:bool|{
        // USB 工作线程始终返回内置型号，仅在启用时追加兼容包识别出的型号。
        h.receivers=vec![builtin.clone()];
        if enabled {
            h.receivers.push(extension.clone());
        }
        h.frame(vec![]);
        let expected:Vec<_>=std::iter::once(&bluetooth).chain(h.receivers.iter()).collect();
        assert_eq!(h.app.wireless_devices.iter().collect::<Vec<_>>(),expected);
        let labels=texts(h);
        assert!(labels.iter().any(|s|s=="82%"));
        assert_eq!(labels.iter().filter(|s|s.as_str()=="2.4G").count(),h.receivers.len());
    };
    check(&mut h,false);
    check(&mut h,true);
    h.app.wireless_focus=Some(extension.id.clone());
    check(&mut h,false);
    assert!(h.app.wireless_focus.is_none());
}

#[test]
fn wireless_detail_marks_charging_with_text_only_when_fresh(){
    let _g=ENV_LOCK.lock().unwrap_or_else(|e|e.into_inner());let mut h=H::new();h.bt=fixture();h.app.settings.bluetooth_notifications=false;
    h.bt[2].reading.as_mut().unwrap().charging=Some(true);
    open_page(&mut h);h.app.wireless_focus=Some("device-2".into());h.frame(vec![]);
    assert!(texts(&h).iter().any(|s|s=="充电中"));
    let reading=h.bt[2].reading.as_mut().unwrap();
    reading.source=BatterySource::Windows;
    reading.charging=None;
    reading.value_since=reading.observed-std::time::Duration::from_secs(240);
    h.frame(vec![]);
    assert!(texts(&h).iter().any(|s|s=="充电中"));
    assert!(texts(&h).iter().any(|s|s=="读数已保持 4 分钟"));
    assert_eq!(crate::bluetooth_ui::color(&h.bt[2],None),crate::bluetooth_ui::level_color(64));
    h.bt[2].reading.as_mut().unwrap().observed-=std::time::Duration::from_secs(181);h.frame(vec![]);
    assert!(!texts(&h).iter().any(|s|s=="充电中"),"过期读数不能继续显示充电状态");
}

#[test]
fn wireless_rings_stay_centered_with_two_to_four_devices(){
    let _g=ENV_LOCK.lock().unwrap_or_else(|e|e.into_inner());
    for count in 2..=4 {
        let mut h=H::new();h.bt=fixture();h.bt.truncate(count);h.app.settings.bluetooth_notifications=false;
        open_page(&mut h);
        let boxes:Vec<_>=(0..count).map(|i|h.ctx.read_response(egui::Id::new(("card",true,("wireless-device",format!("device-{i}"))))).unwrap().rect).collect();
        let center=(boxes.first().unwrap().left()+boxes.last().unwrap().right())/2.0;
        assert!((center-H::local(PW/2.0,0.0).x).abs()<0.5,"{count} 台设备应整组居中");
    }
    let mut h=H::new();h.bt=fixture();h.bt.truncate(1);h.app.settings.bluetooth_notifications=false;open_page(&mut h);
    assert!(h.ctx.read_response(egui::Id::new(("card",true,("wireless-pin","device-0")))).is_some(),"只有一台时直接显示详情");
    assert!(h.ctx.read_response(egui::Id::new(("card",true,"wireless-back"))).is_none(),"单台详情不需要返回");
}

#[test]
fn wireless_device_opens_and_returns_inside_island(){
    let _g=ENV_LOCK.lock().unwrap_or_else(|e|e.into_inner());
    let mut h=H::new();h.bt=fixture();open_page(&mut h);
    let tile=h.ctx.read_response(egui::Id::new(("card",true,("wireless-device","device-1")))).unwrap();
    h.click(tile.rect.center());
    assert_eq!(h.app.wireless_focus.as_deref(),Some("device-1"));
    h.frame(vec![]);
    let back=h.ctx.read_response(egui::Id::new(("card",true,"wireless-back"))).unwrap();
    h.click(back.rect.center());
    assert!(h.app.wireless_focus.is_none());
    assert_eq!(h.app.page(),BLUETOOTH_PAGE);

    for point in [H::local(112.0,100.0),H::local(212.0,101.0)] {
        h.frame(vec![]);
        h.click(tile.rect.center());
        assert_eq!(h.app.wireless_focus.as_deref(),Some("device-1"));
        h.click(point);
        assert!(h.app.wireless_focus.is_none(),"再点详情中的电量环或读数应返回列表");
        assert_eq!(h.app.page(),BLUETOOTH_PAGE);
    }
}

#[test]
fn ring_only_resident_shows_numbers_for_low_or_charging(){
    let _g=ENV_LOCK.lock().unwrap_or_else(|e|e.into_inner());let mut h=H::new();h.bt=fixture();h.app.settings.bluetooth_notifications=false;
    h.app.settings.bluetooth_resident_custom=true;h.app.settings.bluetooth_pinned=vec!["device-0".into(),"device-1".into()];
    h.app.settings.bluetooth_resident_style=1;set_level(&mut h.bt[1],15);
    h.frame(vec![]);
    let t=texts(&h);assert!(t.iter().any(|s|s=="15%"),"偏低的设备显示数字");assert!(!t.iter().any(|s|s=="82%"),"电量充足的只显示环");
    let narrow=h.app.wireless_bar_width();
    h.app.settings.bluetooth_resident_style=0;h.frame(vec![]);
    assert!(texts(&h).iter().any(|s|s=="82%"));assert!(h.app.wireless_bar_width()>narrow);
}

#[test]
fn battery_events_notify_once_and_respect_setting(){
    let _g=ENV_LOCK.lock().unwrap_or_else(|e|e.into_inner());let mut h=H::new();h.bt=fixture();h.app.settings.bluetooth_notifications=false;
    h.bt[2].reading.as_mut().unwrap().charging=Some(false);
    h.bt[0].reading.as_mut().unwrap().charging=Some(false);
    let sent=|h:&H,body:&str|h.app.history.iter().filter(|a|a.body==body).count();
    set_level(&mut h.bt[1],21);h.frame(vec![]);
    set_level(&mut h.bt[1],19);h.t+=1.0;h.frame(vec![]);
    assert_eq!(sent(&h,"MX Master 3S · 电量偏低 · 19%"),1,"跌入偏低时提示");
    assert!(h.app.wireless_fx.get("device-1").is_some_and(|fx|fx.nudge.is_some()&&fx.morph.is_some_and(|m|m.0==21)));
    set_level(&mut h.bt[1],21);h.t+=3.0;h.frame(vec![]);
    set_level(&mut h.bt[1],19);h.t+=3.0;h.frame(vec![]);
    assert_eq!(sent(&h,"MX Master 3S · 电量偏低 · 19%"),1,"10 分钟内不重复提示");
    h.bt[2].reading.as_mut().unwrap().charging=Some(true);h.t+=1.0;h.frame(vec![]);
    assert_eq!(sent(&h,"MX Keys Mini · 开始充电 · 64%"),1);
    h.app.settings.bluetooth_battery_notifications=false;
    h.bt[0].reading.as_mut().unwrap().charging=Some(true);h.t+=1.0;h.frame(vec![]);
    assert_eq!(sent(&h,"耳机 · 开始充电 · 82%"),0,"关闭电量提示后不再提示");
}

#[test]
fn wireless_native_preview(){
    let Some(dest)=std::env::var_os("DAOZAI_BLUETOOTH_PREVIEW_DIR") else{return;};
    let _g=ENV_LOCK.lock().unwrap_or_else(|e|e.into_inner());let dest=std::path::PathBuf::from(dest);std::fs::create_dir_all(&dest).unwrap();
    for (name,skin) in [("native-dark.png",crate::model::Skin::Classic),("native-cream.png",crate::model::Skin::Cream),("native-resident.png",crate::model::Skin::Classic),("native-detail.png",crate::model::Skin::Classic),("native-detail-unknown.png",crate::model::Skin::Classic),
            ("native-receiver-overview.png",crate::model::Skin::Classic),("native-receiver-detail.png",crate::model::Skin::Classic),
            ("native-receiver-battery.png",crate::model::Skin::Classic)]{
        let mut h=H::new();h.app.settings.skin=skin;h.app.settings.bluetooth_notifications=false;h.bt=fixture();h.bt[2].reading.as_mut().unwrap().charging=Some(true);
        h.bt[0].kind=DeviceKind::EarbudsStemTips;h.bt[0].name="AirPods Pro".into();
        let resident=name=="native-resident.png";
        if resident {h.app.settings.bluetooth_resident_custom=true;h.app.settings.bluetooth_pinned=vec!["device-1".into(),"device-2".into()];}
        if !resident {h.app.show_page(BLUETOOTH_PAGE);}
        if name.starts_with("native-detail") {h.app.wireless_focus=Some(h.bt[2].id.clone());}
        if name=="native-detail-unknown.png" {h.bt[2].reading.as_mut().unwrap().charging=None;}
        if name.starts_with("native-receiver") {
            h.receivers.push(if name=="native-receiver-battery.png" {powered_receiver_fixture()} else {receiver_fixture()});
        }
        if matches!(name,"native-receiver-detail.png"|"native-receiver-battery.png") {h.app.wireless_focus=Some(h.receivers[0].id.clone());}
        let height=if resident{50}else{210};
        let mut raster=crate::preview_raster::Raster::new(640,height);
        for frame in 0..65 {
            let env=crate::app::Env{t:frame as f64/60.0,clock:"10:24:00".into(),date:"11月18日 周三".into(),bt:h.bt.clone(),bt_ready:true,receivers:h.receivers.clone(),cursor:if resident{None}else{Some(H::local(200.0,100.0))},..Default::default()};
            let out=h.ctx.run(egui::RawInput{screen_rect:Some(Rect::from_min_size(Pos2::ZERO,egui::vec2(640.0,height as f32))),..Default::default()},|ctx|{egui::CentralPanel::default().frame(egui::Frame::new()).show(ctx,|ui|{h.app.frame(ui,Rect::from_min_size(Pos2::ZERO,egui::vec2(WIN_W,WIN_H)),&env);});});
            raster.update_textures(&out.textures_delta);
            if frame==64 {raster.buf.fill([0.15,0.17,0.21,1.0]);raster.draw(&h.ctx.tessellate(out.shapes,1.0),1.0);}
        }
        let mut bytes=vec![];raster.to_rgba8(&mut bytes);image::save_buffer(dest.join(name),&bytes,640,height as u32,image::ColorType::Rgba8).unwrap();
    }
}

#[test]
fn wireless_unknown_charge_preserves_transitions_and_full() {
    let _g=ENV_LOCK.lock().unwrap_or_else(|e|e.into_inner());
    let mut h=H::new();h.bt=vec![fixture().remove(0)];
    h.bt[0].reading.as_mut().unwrap().charging=Some(true);
    h.frame(vec![]);
    for charge in [None,Some(true),None] {
        h.t+=610.0;
        let r=h.bt[0].reading.as_mut().unwrap();
        r.source=if charge.is_none(){BatterySource::Windows}else{BatterySource::Gatt};
        r.charging=charge;h.frame(vec![]);
        assert!(h.app.wireless_fx.get("device-0").is_none_or(|fx|fx.plug.is_none()&&fx.unplug.is_none()));
    }
    set_level(&mut h.bt[0],100);h.frame(vec![]);
    assert!(!h.app.history.iter().any(|a|a.body.contains("已充满")));
    let r=h.bt[0].reading.as_mut().unwrap();r.source=BatterySource::Gatt;r.charging=Some(false);
    h.frame(vec![]);
    let fx=&h.app.wireless_fx["device-0"];
    assert!(fx.unplug.is_some()&&fx.full.is_some());
    assert!(h.app.history.iter().any(|a|a.body.contains("已充满")));
}

#[test]
fn wireless_first_low_and_disabled_module() {
    let _g=ENV_LOCK.lock().unwrap_or_else(|e|e.into_inner());
    let mut h=H::new();h.bt=vec![fixture().remove(0)];
    h.app.settings.bluetooth_notifications=false;
    let reading=h.bt[0].reading.take();h.bt[0].battery=None;h.frame(vec![]);
    h.bt[0].reading=reading;set_level(&mut h.bt[0],8);h.frame(vec![]);
    assert!(h.app.alerts.iter().chain(h.app.notice.iter()).any(|a|a.body.contains("即将耗尽")));
    h.t+=610.0;h.frame(vec![]);
    assert_eq!(h.app.history.iter().filter(|a|a.body.contains("即将耗尽")).count(),1);
    h.app.settings.bluetooth_notifications=true;
    h.app.settings.visible_pages[BLUETOOTH_PAGE]=false;
    set_level(&mut h.bt[0],25);h.frame(vec![]);set_level(&mut h.bt[0],19);h.frame(vec![]);
    assert!(!h.app.history.iter().any(|a|a.body.contains("电量偏低")));
}

#[test]
fn wireless_cross_source_threshold_keeps_color_and_events_stable() {
    let _g=ENV_LOCK.lock().unwrap_or_else(|e|e.into_inner());
    for threshold in [10,20,50] {
        let mut h=H::new();h.bt=vec![fixture().remove(0)];
        h.app.settings.bluetooth_notifications=false;
        let previous=crate::bluetooth_ui::level_color(threshold);
        for (source,level) in [(BatterySource::Windows,threshold),(BatterySource::Gatt,threshold-1),
                (BatterySource::Gatt,threshold-1),(BatterySource::Windows,threshold),(BatterySource::Gatt,threshold-1)] {
            set_level(&mut h.bt[0],level);
            h.bt[0].reading.as_mut().unwrap().source=source;
            h.t+=3.0;h.frame(vec![]);
            assert_eq!(h.app.wireless_devices[0].battery,Some(level));
            assert_eq!(crate::bluetooth_ui::color(&h.bt[0],h.app.wireless_fx.get("device-0")),previous);
            let warnings=h.app.history.iter().filter(|a|a.body.contains("即将耗尽")||a.body.contains("电量偏低")).count();
            assert_eq!(warnings,usize::from(threshold==10));
        }
        set_level(&mut h.bt[0],threshold-2);h.frame(vec![]);
        assert_eq!(crate::bluetooth_ui::color(&h.bt[0],h.app.wireless_fx.get("device-0")),crate::bluetooth_ui::level_color(threshold-2));
    }
}

#[test]
fn wireless_legacy_pins_migrate_unless_mode_is_explicit() {
    let _g=ENV_LOCK.lock().unwrap_or_else(|e|e.into_inner());
    let _h=H::new();
    let dir=crate::model::config_dir();
    std::fs::create_dir_all(&dir).unwrap();
    let pins=r#"bluetooth_pinned=["device-1"]"#;
    for (contents,custom) in [(pins.to_string(),true),(format!("{pins}\nbluetooth_resident_custom=0"),false),
            (format!("bluetooth_resident_custom=0\n{pins}"),false),("bluetooth_pinned=[]".into(),false)] {
        std::fs::write(dir.join("settings.txt"),&contents).unwrap();
        let s=crate::model::Settings::load();
        assert_eq!(s.bluetooth_resident_custom,custom);
        assert_eq!(s.bluetooth_pinned.len(),usize::from(contents.contains("device-1")));
    }
}

fn wireless_settings_capture(h:&mut H,height:f32)->std::sync::Arc<Mutex<Option<egui::FullOutput>>> {
    h.app.open_settings();
    h.ctx.set_embed_viewports(false);
    let output=std::sync::Arc::new(Mutex::new(None));
    let capture=output.clone();
    egui::Context::set_immediate_viewport_renderer(move|ctx,mut viewport|{
        let id=viewport.ids.this;
        let raw=egui::RawInput{
            viewport_id:id,screen_rect:Some(Rect::from_min_size(Pos2::ZERO,egui::vec2(690.0,height))),
            viewports:std::iter::once((id,egui::ViewportInfo::default())).collect(),..Default::default()
        };
        *capture.lock().unwrap()=Some(ctx.run(raw,|ctx|(viewport.viewport_ui_cb)(ctx)));
    });
    output
}

#[test]
fn wireless_settings_controls_are_in_their_own_page() {
    let _g=ENV_LOCK.lock().unwrap_or_else(|e|e.into_inner());
    let mut h=H::new();
    h.app.wireless_devices=fixture();
    let output=wireless_settings_capture(&mut h,560.0);
    let mut raster=crate::preview_raster::Raster::new(690,560);
    for (tab,name,visible,hidden) in [
        (10,"wireless-page-management",vec!["无线设备电量"],vec!["常驻样式","常驻设备","电量提醒","安装兼容包…"]),
        (crate::extras::SETTINGS_TAB_WIRELESS,"wireless-settings",vec!["岛上常驻","常驻样式","常驻设备","电量提醒"],vec!["蓝牙设备连接","设备兼容","反馈设备适配","本地插件"]),
        (6,"wireless-notifications",vec!["蓝牙设备连接"],vec!["无线设备电量","电量提醒"]),
    ] {
        h.app.settings_tab=tab;
        for _ in 0..3 {
            let _=h.ctx.run(Default::default(),|ctx|h.app.settings_ui(ctx,false));
            let frame=output.lock().unwrap().take().unwrap();
            raster.update_textures(&frame.textures_delta);
            h.shapes=frame.shapes;
        }
        let labels=texts(&h);
        for label in visible {assert!(labels.iter().any(|s|s==label),"缺少 {label}");}
        for label in hidden {assert!(!labels.iter().any(|s|s==label),"意外显示 {label}");}
        if let Some(dest)=std::env::var_os("DAOZAI_BLUETOOTH_PREVIEW_DIR") {
            let dest=std::path::PathBuf::from(dest);
            std::fs::create_dir_all(&dest).unwrap();
            raster.buf.fill([0.10,0.10,0.11,1.0]);
            raster.draw(&h.ctx.tessellate(h.shapes.clone(),1.0),1.0);
            let mut bytes=vec![];raster.to_rgba8(&mut bytes);
            image::save_buffer(dest.join(format!("{name}.png")),&bytes,690,560,image::ColorType::Rgba8).unwrap();
        }
    }
}

#[test]
#[cfg(all(windows, feature="sdk-js"))]
fn wireless_compat_settings_follow_install_and_switch() {
    let _g=ENV_LOCK.lock().unwrap_or_else(|e|e.into_inner());
    let mut h=H::new();
    h.app.settings.settings_theme=crate::settings_theme::SettingsTheme::Dark;
    h.app.settings.visible_pages[BLUETOOTH_PAGE]=true;
    h.app.open_settings();
    h.ctx.set_embed_viewports(false);
    let output=std::sync::Arc::new(Mutex::new(None));
    let capture=output.clone();
    let events=std::sync::Arc::new(Mutex::new(Vec::new()));
    let input=events.clone();
    let switch_id=egui::Id::new(("settings-plugin-switch","daozai.wireless-receivers"));
    let preview=std::env::var_os("DAOZAI_BLUETOOTH_PREVIEW_DIR").map(|dest|{
        let dest=std::path::PathBuf::from(dest);
        std::fs::create_dir_all(&dest).unwrap();
        (dest,std::cell::RefCell::new(crate::preview_raster::Raster::new(690,920)))
    });
    egui::Context::set_immediate_viewport_renderer(move|ctx,mut viewport|{
        let id=viewport.ids.this;
        let raw=egui::RawInput{
            viewport_id:id,focused:true,events:std::mem::take(&mut *input.lock().unwrap()),
            screen_rect:Some(Rect::from_min_size(Pos2::ZERO,egui::vec2(690.0,920.0))),
            viewports:std::iter::once((id,egui::ViewportInfo{focused:Some(true),..Default::default()})).collect(),
            ..Default::default()
        };
        let mut switch=None;
        let frame=ctx.run(raw,|ctx|{
            (viewport.viewport_ui_cb)(ctx);
            switch=ctx.read_response(switch_id).map(|response|response.rect.center());
        });
        *capture.lock().unwrap()=Some((frame,switch));
    });
    let wireless=crate::extras::SETTINGS_TAB_WIRELESS;
    let frame=|h:&mut H,tab|{
        h.app.settings_tab=tab;
        let _=h.ctx.run(Default::default(),|ctx|h.app.settings_ui(ctx,false));
        let (frame,switch)=output.lock().unwrap().take().unwrap();
        if let Some((_,raster))=&preview {raster.borrow_mut().update_textures(&frame.textures_delta);}
        h.shapes=frame.shapes;
        switch
    };
    let check=|h:&mut H,tab,version:Option<&str>|{
        for _ in 0..3 {frame(h,tab);}
        let labels=texts(h);
        let expected=version.is_some();
        for label in ["设备兼容","启用兼容包","反馈设备适配"] {
            assert_eq!(labels.iter().any(|s|s==label),expected,"设置页 {tab} 的 {label}");
        }
        assert_eq!(labels.iter().find(|s|s.starts_with("无线设备兼容包 v")).map(String::as_str),
            version,"设置页 {tab} 显示的兼容包版本");
        assert_eq!(labels.iter().any(|s|s.starts_with("兼容接收器")),expected,"设置页 {tab} 的名单入口");
        assert!(!labels.iter().any(|s|matches!(s.as_str(),"本地插件"|"安装兼容包…"|"卸载")));
        let title=h.shapes.iter().find_map(|shape|match &shape.shape {
            egui::Shape::Text(text) if text.galley.text()=="启用兼容包"=>Some(text.visual_bounding_rect()),
            _=>None,
        });
        let has_plugin_tag=title.is_some_and(|title|h.shapes.iter().any(|shape| {
            matches!(&shape.shape,egui::Shape::Text(text) if text.galley.text()=="插件"
                && text.visual_bounding_rect().left()>=title.right()
                && (text.visual_bounding_rect().center().y-title.center().y).abs()<8.0)
        }));
        assert_eq!(has_plugin_tag,expected,"设置页 {tab} 的兼容包标题旁应有插件小标");
    };
    let click=|h:&mut H,pos:Pos2|{
        for pressed in [true,false] {
            *events.lock().unwrap()=vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton{pos,button:egui::PointerButton::Primary,pressed,modifiers:Default::default()},
            ];
            frame(h,wireless);
        }
    };
    let switch=|h:&mut H|{
        let pos=frame(h,wireless).expect("已安装兼容包应显示启停开关");
        click(h,pos);
    };
    let toggle_receivers=|h:&mut H|{
        frame(h,wireless);
        let pos=h.shapes.iter().find_map(|shape|match &shape.shape {
            egui::Shape::Text(text) if text.galley.text().starts_with("兼容接收器")=>{
                Some(text.visual_bounding_rect().center())
            }
            _=>None,
        }).expect("已安装兼容包应显示名单入口");
        click(h,pos);
        for _ in 0..16 {frame(h,wireless);}
    };
    let check_receiver=|h:&H,name:&str,id:&str,visible:bool|{
        let labels=texts(h);
        for label in [name,id] {
            assert_eq!(labels.iter().any(|s|s==label),visible,"接收器名单的 {label}");
        }
    };
    let save_preview=|h:&H,name:&str|{
        if let Some((dest,raster))=&preview {
            let mut raster=raster.borrow_mut();
            raster.buf.fill([0.10,0.10,0.11,1.0]);
            raster.draw(&h.ctx.tessellate(h.shapes.clone(),1.0),1.0);
            let mut bytes=vec![];
            raster.to_rgba8(&mut bytes);
            image::save_buffer(dest.join(name),&bytes,690,920,image::ColorType::Rgba8).unwrap();
        }
    };
    check(&mut h,wireless,None);
    h.app.plugins.test_market_install_wireless_compat().unwrap();
    let store=h.app.wireless_compat_store();
    assert!(store.package().is_none(),"安装后的插件默认停用");
    check(&mut h,wireless,Some("无线设备兼容包 v1.0.0"));
    check_receiver(&h,"罗技 Nano 接收器","046D:C542",false);
    save_preview(&h,"wireless-compat-collapsed.png");
    check(&mut h,0,None);
    toggle_receivers(&mut h);
    check_receiver(&h,"罗技 Nano 接收器","046D:C542",true);
    assert!(texts(&h).iter().any(|s|s.contains("1 个 2.4G USB 接收器")));
    toggle_receivers(&mut h);
    check_receiver(&h,"罗技 Nano 接收器","046D:C542",false);
    toggle_receivers(&mut h);
    switch(&mut h);
    assert_eq!(store.package().unwrap().version(),"1.0.0");
    for _ in 0..16 {frame(&mut h,wireless);}
    let pos=frame(&mut h,wireless).expect("开关应仍可见");
    assert!(h.shapes.iter().any(|shape|matches!(&shape.shape,egui::Shape::Rect(rect)
        if rect.fill==egui::Color32::from_rgb(140,127,208) && rect.rect.center().distance(pos)<0.1)),
        "已启用的兼容包开关应为紫色");
    switch(&mut h);
    assert!(store.package().is_none(),"停用后收回兼容数据");
    check(&mut h,wireless,Some("无线设备兼容包 v1.0.0"));
    check_receiver(&h,"罗技 Nano 接收器","046D:C542",true);
    check_receiver(&h,"VXE 蜻蜓 R1 无线接收器","373B:10C9",false);

    h.app.plugins.test_market_update_wireless_compat().unwrap();
    assert!(store.package().is_none(),"更新后保留停用状态");
    check(&mut h,wireless,Some("无线设备兼容包 v1.0.1"));
    check_receiver(&h,"罗技 Nano 接收器","046D:C542",true);
    check_receiver(&h,"VXE 蜻蜓 R1 无线接收器","373B:10C9",true);
    assert!(texts(&h).iter().any(|s|s.contains("2 个 2.4G USB 接收器")));
    save_preview(&h,"wireless-compat-expanded.png");
    h.app.plugins.test_uninstall_wireless_compat().unwrap();
    check(&mut h,wireless,None);
    check_receiver(&h,"罗技 Nano 接收器","046D:C542",false);
    check_receiver(&h,"VXE 蜻蜓 R1 无线接收器","373B:10C9",false);
}

#[test]
fn reading_age_switches_to_hours_and_days(){
    use crate::bluetooth_ui::duration_text;
    for (secs,text) in [(0,"0 秒"),(59,"59 秒"),(60,"1 分钟"),(3599,"59 分钟"),(3600,"1 小时"),(5*3600+7*60,"5 小时 7 分钟"),(86399,"23 小时 59 分钟"),(86400,"1 天"),(3*86400+5*3600,"3 天")] {
        assert_eq!(duration_text(secs),text,"{secs} 秒");
    }
}
