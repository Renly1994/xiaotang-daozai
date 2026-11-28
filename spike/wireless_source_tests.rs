use super::*;
use std::thread;

const ID: &str="example.wireless-test";
const TODAY: &str="2026-11-28";
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Result<Self> {
        let unique=std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_nanos();
        let root=std::env::temp_dir().join(format!("daozai-wireless-{}-{unique}",std::process::id()));
        fs::create_dir(&root)?;Ok(Self(root))
    }
    fn plugin(&self,id:&str,owner:&str,capabilities:Value,permissions:Value,code:&str)->Result<PathBuf> {
        let dir=self.0.join(id);fs::create_dir(&dir)?;
        fs::write(dir.join(COVER_FILE),include_bytes!("examples/payday/cover.png"))?;
        fs::write(dir.join("main.js"),code)?;
        let path=dir.join("plugin.json");
        fs::write(&path,serde_json::to_vec(&json!({"id":id,"name":"测试外部电量","author":"测试",
            "version":"1.0.0","api":1,"runtime":"js","entry":"main.js","owner":owner,
            "cover":COVER_FILE,"capabilities":capabilities,"dataVersion":1,"permissions":permissions,"settings":[]}))?)?;
        Ok(path)
    }
    fn source(&self)->Result<PathBuf> {
        let path=self.0.join("device.json");
        let now=chrono::Utc::now().to_rfc3339();
        fs::write(&path,serde_json::to_vec(&json!({"wireless":[{"id":"keyboard-1","name":"Test keyboard",
            "kind":"keyboard","vendorId":2821,"productId":6862,"percent":95,"online":true,"fresh":true,
            "measuredAtUtc":now,"connectionCheckedAtUtc":now,"freshForSeconds":150}]}))?)?;
        Ok(path)
    }
}
impl Drop for Fixture { fn drop(&mut self){let _=fs::remove_dir_all(&self.0);} }
fn settle(host:&mut LocalPlugins,on:bool) {
    let deadline=Instant::now()+Duration::from_secs(6);
    loop {
        host.set_wireless_enabled(on);host.tick_features(false,false,false,TODAY);
        if host.jobs.is_empty(){return;}
        assert!(Instant::now()<deadline,"wireless source job timed out");
        thread::sleep(Duration::from_millis(2));
    }
}
const SCRIPT:&str="function refresh(i){if(i.source.status!=='ready')return {wireless:[]};return JSON.parse(i.source.text);}";

// UI regressions use the actual package, source grant and asynchronous JS path.
// This helper is compiled only in tests and never populates outputs directly.
pub(crate) fn ui_provider(host:&mut LocalPlugins,records:Value) -> Result<()> {
    const UI_ID:&str="test.battery";
    let dir=host.dir.join("ui-provider");fs::create_dir_all(&dir)?;
    let manifest=dir.join("plugin.json");
    if !manifest.exists() {
        fs::write(dir.join(COVER_FILE),include_bytes!("examples/payday/cover.png"))?;
        fs::write(dir.join("main.js"),SCRIPT)?;
        fs::write(&manifest,serde_json::to_vec(&json!({"id":UI_ID,"name":"Test external source","author":"Test",
            "version":"1.0.0","api":1,"runtime":"js","entry":"main.js","owner":"wireless",
            "cover":COVER_FILE,"capabilities":["wireless.devices","timer"],"dataVersion":1,
            "permissions":{"source.read":"file"},"settings":[]}))?)?;
        host.install(&manifest)?;host.enable(UI_ID,true)?;
    }
    let source=dir.join("source.json");
    fs::write(&source,serde_json::to_vec(&json!({"wireless":records}))?)?;
    host.set_source_grant(UI_ID,Some(source_read::grant_file(&source)?))?;
    host.next.remove(UI_ID);settle(host,true);
    host.wireless_store().set_usb_instances(vec!["USB\\VID_0001&PID_0002\\UI-TEST".into()],Instant::now());
    Ok(())
}

#[test]
fn wireless_source_follows_grant_module_enable_and_restart() -> Result<()> {
    let f=Fixture::new()?;
    let path=f.plugin(ID,"wireless",json!(["wireless.devices","timer"]),json!({"source.read":"file"}),SCRIPT)?;
    let file=f.source()?;
    let dir=f.0.join("installed");let mut host=LocalPlugins::open(dir.clone());
    host.install(&path)?;host.enable(ID,true)?;settle(&mut host,true);
    assert_eq!(host.wireless_sources().count(),0,"enable is not a source grant");
    host.set_source_grant(ID,Some(source_read::grant_file(&file)?))?;settle(&mut host,true);
    let records=host.wireless_sources().next().expect("authorized provider result").2;
    assert_eq!(records[0].percent,Some(95));assert!(records[0].fresh);
    let measured=records[0].measured_at_utc.clone();
    settle(&mut host,false);assert_eq!(host.wireless_sources().count(),0);
    assert!(host.jobs.is_empty());
    settle(&mut host,true);assert_eq!(host.wireless_sources().next().unwrap().2[0].measured_at_utc,measured);
    host.enable(ID,false)?;settle(&mut host,true);assert_eq!(host.wireless_sources().count(),0);
    host.enable(ID,true)?;settle(&mut host,true);assert_eq!(host.wireless_sources().count(),1);
    drop(host);
    let mut host=LocalPlugins::open(dir);settle(&mut host,true);
    assert_eq!(host.wireless_sources().count(),1,"restart reads authorized source, not stored device output");
    host.set_source_grant(ID,None)?;assert_eq!(host.wireless_sources().count(),0);
    settle(&mut host,true);assert_eq!(host.wireless_sources().count(),0);
    host.uninstall(ID,true)?;host.install(&path)?;host.enable(ID,true)?;settle(&mut host,true);
    assert_eq!(host.wireless_sources().count(),0,"reinstall cannot restore source permission");
    Ok(())
}

#[test]
fn wireless_source_failure_and_revocation_clear_cached_outputs() -> Result<()> {
    let f=Fixture::new()?;let path=f.plugin(ID,"wireless",json!(["wireless.devices","timer"]),json!({"source.read":"file"}),SCRIPT)?;
    let file=f.source()?;let mut host=LocalPlugins::open(f.0.join("installed"));
    host.install(&path)?;host.enable(ID,true)?;host.set_source_grant(ID,Some(source_read::grant_file(&file)?))?;
    settle(&mut host,true);assert_eq!(host.wireless_sources().count(),1);
    fs::rename(&file,f.0.join("offline.json"))?;host.next.remove(ID);settle(&mut host,true);
    assert_eq!(host.wireless_sources().count(),0,"offline data cannot leave an old live provider result");
    fs::rename(f.0.join("offline.json"),&file)?;host.next.remove(ID);settle(&mut host,true);
    assert_eq!(host.wireless_sources().count(),1);
    host.next.remove(ID);host.tick_features(false,false,false,TODAY);
    assert!(host.jobs.contains_key(ID));host.set_source_grant(ID,None)?;settle(&mut host,true);
    assert_eq!(host.wireless_sources().count(),0,"late in-flight response after revocation is ignored");
    Ok(())
}

#[test]
fn wireless_manifest_requires_explicit_local_source_and_exclusive_native_output() -> Result<()> {
    let f=Fixture::new()?;
    for (id,owner,caps,permissions) in [
        ("example.no-grant","wireless",json!(["wireless.devices"]),json!({})),
        ("example.remote","wireless",json!(["wireless.devices"]),json!({"source.read":"https"})),
        ("example.foreign","status",json!(["wireless.devices","status.page"]),json!({"source.read":"file"})),
        ("example.mixed","wireless",json!(["wireless.devices","island.pinned"]),json!({"source.read":"file"}))] {
        let path=f.plugin(id,owner,caps,permissions,SCRIPT)?;
        assert!(Package::read(&path).is_err(),"{id}");
    }
    let path=f.plugin("example.loopback","wireless",json!(["wireless.devices","timer"]),json!({"source.read":"loopback"}),SCRIPT)?;
    assert!(Package::read(&path).is_ok());
    Ok(())
}

#[test]
fn wireless_payload_rejects_undeclared_output_and_future_evidence() -> Result<()> {
    let f=Fixture::new()?;
    let future=(chrono::Utc::now()+chrono::Duration::seconds(30)).to_rfc3339();
    let payload=json!({"wireless":[{"id":"test","name":"Keyboard","kind":"keyboard","vendorId":1,"productId":2,
        "percent":95,"measuredAtUtc":future}]}).to_string();
    let path=f.plugin("example.future","wireless",json!(["wireless.devices"]),json!({"source.read":"file"}),&format!("function refresh(){{return {payload};}}"))?;
    assert!(Package::read(&path)?.render(&Values::new(),&Value::Null,TODAY).is_err());
    let path=f.plugin("example.undeclared","tasks",json!(["card"]),json!({}),"function refresh(){return {card:'Test',wireless:[]};}")?;
    assert!(Package::read(&path)?.render(&Values::new(),&Value::Null,TODAY).is_err());
    Ok(())
}
