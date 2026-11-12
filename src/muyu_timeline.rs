//! Read Muyu's saved playback position when its SMTC session has no timeline.
use super::{MediaKind,NowPlaying};
use serde::Deserialize;
use std::{fs::File,io::Read,path::{Path,PathBuf},time::{Duration,Instant,SystemTime}};

const MAX_BYTES:u64=2*1024*1024;
const MAX_AGE:Duration=Duration::from_secs(5);

#[derive(Deserialize)]
struct Document {playback:Playback}
#[derive(Deserialize)]
struct Playback {v:u32,index:usize,position:f64,queue:Vec<serde_json::Value>}
#[derive(Deserialize)]
struct Song {
    title:String,artists:Vec<Artist>,album:Option<Album>,
    #[serde(rename="durationSeconds")]
    duration:f64,
}
#[derive(Deserialize)]
struct Artist {name:String}
#[derive(Deserialize)]
struct Album {title:String}

#[derive(Clone)]
struct Snapshot {title:String,artist:String,album:String,position:f64,duration:f64}
fn parse(bytes:&[u8])->Option<Snapshot>{
    if bytes.len() as u64>MAX_BYTES{return None;}
    let saved:Document=serde_json::from_slice(bytes).ok()?;
    let playback=saved.playback;
    if playback.v!=1{return None;}
    let song:Song=serde_json::from_value(playback.queue.get(playback.index)?.clone()).ok()?;
    if !playback.position.is_finite() || !song.duration.is_finite() || !(0.0..86400.0).contains(&song.duration)
        || song.duration==0.0 || playback.position<0.0 || playback.position>song.duration{return None;}
    Some(Snapshot{title:song.title.clone(),artist:song.artists.iter().map(|a|a.name.as_str()).collect::<Vec<_>>().join("/"),
        album:song.album.map(|album|album.title).unwrap_or_default(),position:playback.position,duration:song.duration})
}

struct Saved {path:PathBuf,modified:SystemTime,bytes:Vec<u8>,snapshot:Snapshot}
#[derive(Default)]
pub(super) struct Reader {paths:Vec<PathBuf>,scan_at:Option<Instant>,saved:Option<Saved>,paused_at:Option<SystemTime>}
impl Reader {
    pub(super) fn apply(&mut self,media:&mut NowPlaying){
        if !media.source.eq_ignore_ascii_case("Muyu.exe") || media.kind()!=MediaKind::Music || media.sampled_at.is_some(){return;}
        let at=Instant::now();let now=SystemTime::now();
        if !media.playing{self.paused_at=Some(now);}
        if self.scan_at.is_none_or(|last|at.duration_since(last)>=Duration::from_secs(5)){
            self.paths=discover();self.scan_at=Some(at);
        }
        for path in self.paths.clone(){
            if let Some((snapshot,modified))=self.load(&path){
                if self.accept(media,&snapshot,modified,now,at){break;}
            }
        }
    }

    fn load(&mut self,path:&Path)->Option<(Snapshot,SystemTime)>{
        let file=File::open(path).ok()?;
        let meta=file.metadata().ok()?;let modified=meta.modified().ok()?;
        if meta.len()>MAX_BYTES{return None;}
        let mut bytes=Vec::new();file.take(MAX_BYTES+1).read_to_end(&mut bytes).ok()?;
        if let Some(saved)=&self.saved {
            if saved.path==path && saved.modified==modified && saved.bytes==bytes{return Some((saved.snapshot.clone(),modified));}
        }
        let snapshot=parse(&bytes)?;
        self.saved=Some(Saved{path:path.to_path_buf(),modified,bytes,snapshot:snapshot.clone()});
        Some((snapshot,modified))
    }

    fn accept(&mut self,media:&mut NowPlaying,snapshot:&Snapshot,modified:SystemTime,now:SystemTime,at:Instant)->bool{
        if !crate::lyrics::same_song(&media.title,&media.artist,&snapshot.title,&snapshot.artist)
            || !media.album.is_empty() && !snapshot.album.is_empty() && !media.album.trim().eq_ignore_ascii_case(snapshot.album.trim()) {return false;}
        let Ok(age)=now.duration_since(modified)else{return false;};
        // A saved pause position predates resuming. Start from it without adding time spent paused.
        let resuming=media.playing && self.paused_at.is_some_and(|paused|modified<=paused
            && now.duration_since(paused).is_ok_and(|elapsed|elapsed<=MAX_AGE));
        if media.playing && age>MAX_AGE && !resuming{return false;}
        let elapsed=if media.playing && !resuming{age}else{Duration::ZERO};
        media.position=(snapshot.position+elapsed.as_secs_f64()).min(snapshot.duration);media.duration=snapshot.duration;
        media.sampled_at=Some(at);
        if media.playing && !resuming{self.paused_at=None;}
        true
    }
}

fn discover()->Vec<PathBuf>{
    use windows::{core::PWSTR,Win32::{Foundation::{CloseHandle,HANDLE},System::{
        Diagnostics::ToolHelp::{CreateToolhelp32Snapshot,Process32FirstW,Process32NextW,PROCESSENTRY32W,TH32CS_SNAPPROCESS},
        Threading::{OpenProcess,QueryFullProcessImageNameW,PROCESS_NAME_WIN32,PROCESS_QUERY_LIMITED_INFORMATION}}}};
    struct Handle(HANDLE);
    impl Drop for Handle{fn drop(&mut self){unsafe{let _=CloseHandle(self.0);}}}
    unsafe {
        let Ok(snapshot)=CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS,0)else{return Vec::new();};let snapshot=Handle(snapshot);
        let mut entry=PROCESSENTRY32W{dwSize:std::mem::size_of::<PROCESSENTRY32W>() as u32,..Default::default()};
        let mut paths=Vec::new();let mut next=Process32FirstW(snapshot.0,&mut entry);
        while next.is_ok(){
            let end=entry.szExeFile.iter().position(|c|*c==0).unwrap_or(entry.szExeFile.len());
            if String::from_utf16_lossy(&entry.szExeFile[..end]).eq_ignore_ascii_case("Muyu.exe"){
                if let Ok(process)=OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION,false,entry.th32ProcessID){
                    let process=Handle(process);let mut buffer=[0u16;32768];let mut len=buffer.len() as u32;
                    if QueryFullProcessImageNameW(process.0,PROCESS_NAME_WIN32,PWSTR(buffer.as_mut_ptr()),&mut len).is_ok(){
                        let exe=PathBuf::from(String::from_utf16_lossy(&buffer[..len as usize]));
                        if let Some(folder)=exe.parent(){paths.push(folder.join("independent-data").join("native-ui.json"));}
                    }
                }
            }
            next=Process32NextW(snapshot.0,&mut entry);
        }
        paths
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(position:f64)->serde_json::Value{
        serde_json::json!({"playback":{"v":1,"index":0,"position":position,"queue":[{
            "title":"Track","artists":[{"name":"Artist"}],"album":{"title":"Album"},"durationSeconds":180.0}]}})
    }
    fn sample(position:f64)->Snapshot{parse(&serde_json::to_vec(&fixture(position)).unwrap()).unwrap()}
    fn media()->NowPlaying{NowPlaying{source:"Muyu.exe".into(),title:"Track".into(),artist:"Artist".into(),album:"Album".into(),
        playing:true,playback_type:MediaKind::Music,..Default::default()}}

    #[test]
    fn saved_progress_follows_forward_backward_and_zero_seeks(){
        let mut reader=Reader::default();let at=Instant::now();let now=SystemTime::now();let mut media=media();
        let lyrics=crate::lyrics::parse_lrc("[00:00]Start\n[00:30]Middle\n[02:00]End");
        for (position,line) in [(135.0,2),(42.0,1),(0.0,0)]{
            assert!(reader.accept(&mut media,&sample(position),now-Duration::from_millis(300),now,at));
            let current=media.position+at.duration_since(media.sampled_at.unwrap()).as_secs_f64();
            assert!((current-position-0.3).abs()<0.001);
            assert_eq!(lyrics.index_at(current as f32),Some(line));assert_eq!(media.duration,180.0);
            assert_eq!(media.song_id,None,"Huawei IDs must not become NetEase song IDs");
        }
    }

    #[test]
    fn rejects_wrong_song_artist_album_and_stale_playing_data(){
        let mut reader=Reader::default();let at=Instant::now();let now=SystemTime::now();
        for field in ["title","artist","album"]{
            let mut media=media();match field{"title"=>media.title="Other".into(),"artist"=>media.artist="Other".into(),_=>media.album="Other".into()}
            assert!(!reader.accept(&mut media,&sample(10.0),now,now,at));assert!(media.sampled_at.is_none());
        }
        for modified in [now-Duration::from_secs(6),now+Duration::from_secs(1)]{
            let mut media=media();assert!(!reader.accept(&mut media,&sample(10.0),modified,now,at));assert_eq!(media.duration,0.0);
        }
    }

    #[test]
    fn pause_and_resume_do_not_count_time_spent_paused(){
        let mut reader=Reader::default();let at=Instant::now();let now=SystemTime::now();let mut media=media();
        media.playing=false;reader.paused_at=Some(now);
        assert!(reader.accept(&mut media,&sample(42.0),now-Duration::from_secs(30),now,at));
        assert_eq!(media.sampled_at,Some(at));assert_eq!(media.position,42.0);
        media.playing=true;
        assert!(reader.accept(&mut media,&sample(42.0),now-Duration::from_secs(30),now+Duration::from_secs(1),at+Duration::from_secs(1)));
        assert_eq!(media.sampled_at,Some(at+Duration::from_secs(1)));
        assert!(reader.accept(&mut media,&sample(43.0),now+Duration::from_secs(2),now+Duration::from_secs(3),at+Duration::from_secs(3)));
        assert_eq!(media.sampled_at,Some(at+Duration::from_secs(3)));assert_eq!(media.position,44.0);assert!(reader.paused_at.is_none());
    }

    #[test]
    fn seeking_to_the_same_saved_position_resets_elapsed_time(){
        let mut reader=Reader::default();let at=Instant::now();let now=SystemTime::now();let mut media=media();
        assert!(reader.accept(&mut media,&sample(42.0),now-Duration::from_secs(2),now,at));
        assert_eq!(media.position,44.0);
        assert!(reader.accept(&mut media,&sample(42.0),now,now,at));
        assert_eq!(media.position,42.0,"A new file timestamp must make an otherwise identical seek observable");
    }

    #[test]
    fn rejects_invalid_schema_and_out_of_range_positions(){
        for position in [-1.0,180.1]{assert!(parse(&serde_json::to_vec(&fixture(position)).unwrap()).is_none());}
        for (field,value) in [("v",serde_json::json!(2)),("index",serde_json::json!(-1)),("index",serde_json::json!(4))]{
            let mut data=fixture(10.0);data["playback"][field]=value;assert!(parse(&serde_json::to_vec(&data).unwrap()).is_none());
        }
        let mut data=fixture(10.0);data["playback"]["queue"][0]["durationSeconds"]=serde_json::json!(0);
        assert!(parse(&serde_json::to_vec(&data).unwrap()).is_none());
        let mut data=fixture(10.0);data["playback"]["queue"].as_array_mut().unwrap().push(serde_json::json!({"album":null}));
        assert!(parse(&serde_json::to_vec(&data).unwrap()).is_some(),"Unrelated incomplete queue entries must not invalidate the current song");
        assert!(parse(b"{incomplete").is_none());assert!(parse(&vec![b' ';MAX_BYTES as usize+1]).is_none());
    }

    #[test]
    fn accepts_a_current_song_without_album_metadata(){
        let mut data=fixture(42.0);data["playback"]["queue"][0].as_object_mut().unwrap().remove("album");
        let snapshot=parse(&serde_json::to_vec(&data).unwrap()).unwrap();
        let now=SystemTime::now();let mut reader=Reader::default();let mut media=media();
        assert!(reader.accept(&mut media,&snapshot,now,now,Instant::now()));assert_eq!(media.position,42.0);
    }

    #[test]
    fn file_updates_with_equal_size_and_timestamp_still_realign_lyrics(){
        let folder=std::env::temp_dir().join(format!("daozai-muyu-timeline-{}-{}",std::process::id(),SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(&folder).unwrap();let path=folder.join("native-ui.json");
        let mut reader=Reader::default();let mut media=media();let now=SystemTime::now();let at=Instant::now();
        let lyrics=crate::lyrics::parse_lrc("[00:00]Start\n[00:30]Middle");
        let mut lengths=Vec::new();
        for (position,line) in [(42.0,1),(12.0,0)]{
            let bytes=serde_json::to_vec(&fixture(position)).unwrap();lengths.push(bytes.len());
            std::fs::write(&path,&bytes).unwrap();
            File::options().write(true).open(&path).unwrap().set_times(std::fs::FileTimes::new().set_modified(now)).unwrap();
            let (snapshot,modified)=reader.load(&path).unwrap();
            assert!(reader.accept(&mut media,&snapshot,modified,now,at));
            assert_eq!(lyrics.index_at(media.position as f32),Some(line));
        }
        assert_eq!(lengths[0],lengths[1]);std::fs::remove_dir_all(folder).unwrap();
    }

    #[test]
    fn keeps_existing_smtc_timeline_and_ignores_other_sources(){
        let mut reader=Reader::default();let mut media=media();media.sampled_at=Some(Instant::now());media.position=32.0;
        reader.apply(&mut media);assert_eq!(media.position,32.0);assert!(reader.scan_at.is_none());
        media.sampled_at=None;media.source="other.exe".into();reader.apply(&mut media);assert!(reader.scan_at.is_none());
    }

    #[test]
    #[ignore="Read-only check requiring an active Muyu SMTC session and its native-ui.json"]
    fn live_muyu_saved_timeline(){
        let _apartment=super::super::init_winrt();
        let manager=super::super::media_result(windows::Media::Control::GlobalSystemMediaTransportControlsSessionManager::RequestAsync).unwrap();
        let sessions=manager.GetSessions().unwrap();
        let session=sessions.into_iter().find(|s|s.SourceAppUserModelId().is_ok_and(|id|id.to_string().eq_ignore_ascii_case("Muyu.exe"))).expect("Muyu session required");
        let mut cache=super::super::ArtworkCache::default();
        for path in discover(){
            let file=File::open(&path).unwrap();let meta=file.metadata().unwrap();let mut bytes=Vec::new();file.take(MAX_BYTES+1).read_to_end(&mut bytes).unwrap();
            println!("saved file bytes={} age={:?} parsed={} schema_error={:?}",meta.len(),meta.modified().unwrap().elapsed(),parse(&bytes).is_some(),serde_json::from_slice::<Document>(&bytes).err());
        }
        let started=Instant::now();
        for _ in 0..3{
            let media=super::super::read_session(&session,&mut cache).unwrap();
            println!("source={} title={} position={:.3} duration={} playing={}",media.source,media.title,media.position,media.duration,media.playing);
            assert!(media.duration>0.0 && media.position>=0.0 && media.position<=media.duration && media.sampled_at.is_some());
            std::thread::sleep(Duration::from_millis(500));
        }
        assert!(started.elapsed()<Duration::from_secs(10));
    }
}
