//! 仅在已授权的系统通知缺少应用标识时，按编号和到达时间核对 Windows 记录的发送方。
use std::ffi::{c_char,c_int,c_void,CString};
use std::path::Path;
use std::ptr;

type RowCallback=unsafe extern "C" fn(*mut c_void,c_int,*mut *mut c_char,*mut *mut c_char)->c_int;
#[link(name="winsqlite3")]
extern "C" {
    fn sqlite3_open_v2(name:*const c_char,db:*mut *mut c_void,flags:c_int,vfs:*const c_char)->c_int;
    fn sqlite3_exec(db:*mut c_void,sql:*const c_char,callback:Option<RowCallback>,data:*mut c_void,error:*mut *mut c_char)->c_int;
    fn sqlite3_close(db:*mut c_void)->c_int;
}

pub(super) fn matches(id:u32,created:i64)->bool{
    let Some(local)=std::env::var_os("LOCALAPPDATA") else{return false};
    matches_in(&Path::new(&local).join("Microsoft/Windows/Notifications/wpndatabase.db"),id,created)
}

fn matches_in(path:&Path,id:u32,created:i64)->bool{
    let Ok(name)=CString::new(path.to_string_lossy().as_bytes()) else{return false};
    // 查询只包含数值参数。两个字段同时匹配，避免复用编号时串到另一条通知。
    let sql=CString::new(format!("SELECT 1 FROM Notification n JOIN NotificationHandler h ON h.RecordId=n.HandlerId WHERE n.Id={id} AND n.ArrivalTime={created} AND n.Type='toast' AND h.PrimaryId='QQ' LIMIT 1")).unwrap();
    unsafe extern "C" fn found(data:*mut c_void,_:c_int,_:*mut *mut c_char,_:*mut *mut c_char)->c_int{
        unsafe { *data.cast::<bool>()=true; } 0
    }
    let mut db=ptr::null_mut();
    let mut matched=false;
    unsafe{
        const SQLITE_OPEN_READONLY:c_int=1;
        let opened=sqlite3_open_v2(name.as_ptr(),&mut db,SQLITE_OPEN_READONLY,ptr::null())==0;
        let ok=opened && sqlite3_exec(db,sql.as_ptr(),Some(found),(&mut matched as *mut bool).cast(),ptr::null_mut())==0;
        if !db.is_null(){sqlite3_close(db);}
        ok && matched
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore="需要本机通知中心留有指定测试通知"]
    fn qq_notification_local_identity(){
        let id=std::env::var("DAOZAI_QQ_TEST_ID").unwrap().parse().unwrap();
        use windows::UI::Notifications::{Management::UserNotificationListener,NotificationKinds};
        use windows::Win32::System::WinRT::{RoInitialize,RoUninitialize,RO_INIT_MULTITHREADED};
        unsafe { RoInitialize(RO_INIT_MULTITHREADED).unwrap(); }
        let items=UserNotificationListener::Current().unwrap().GetNotificationsAsync(NotificationKinds::Toast).unwrap().get().unwrap();
        let n=(0..items.Size().unwrap()).filter_map(|i|items.GetAt(i).ok()).find(|n|n.Id().ok()==Some(id)).expect("原始测试通知已离开通知中心");
        let created=n.CreationTime().unwrap().UniversalTime;
        assert_eq!(super::super::notification_identity(&n,id,created,false),("QQ".into(),"QQ".into()));
        assert!(matches(id,created));
        drop(n);drop(items);unsafe { RoUninitialize(); }
        assert!(!matches(id,created+1));
    }
}
