//! QQ 系统通知的 payload：点开时要跳到的会话，以及发送方头像的文件路径。
//!
//! QQ 把「点这条通知打开哪个聊天」写在 toast 的 `launch` 里
//! （`ntqq-notification://externalCommand?subcmd=openExternalWindow;peerUid=…;chatType=2;…`），
//! 该协议在系统里注册给了 QQ 自己的处理程序，所以调用它等价于用户点了一下系统通知。
//! 头像则是 `<image placement="appLogoOverride" src="…">` 指向的本地缓存文件
//! （群消息是群头像，私聊是发送者头像），路径由 QQ 自己填，不能按群号推算。

use std::path::PathBuf;

#[derive(Clone,Debug,Default,PartialEq,Eq)]
pub struct QqToast{
    /// 直达会话的协议地址；缺失或格式变化时保持 `None`，调用方退回托盘点击。
    pub launch:Option<String>,
    /// 头像文件；QQ 没有附带图片时为空。
    pub avatar:Option<PathBuf>,
    /// `1` 是私聊、`2` 是群聊；只用于诊断。
    pub chat_type:Option<u8>,
}

pub fn parse(payload:&str)->QqToast{
    let launch=attribute(payload,"launch").map(|value|unescape(&value));
    QqToast{
        avatar:image_source(payload).map(PathBuf::from),
        chat_type:launch.as_deref().and_then(|uri|query(uri,"chatType")).and_then(|value|value.parse().ok()),
        launch,
    }
}

/// 头像必须是存在且大小合理的普通文件：路径来自外部数据，不能直接交给解码器。
pub fn usable_avatar(path:&std::path::Path)->bool{
    std::fs::metadata(path).is_ok_and(|meta|meta.is_file() && meta.len()>0 && meta.len()<=4*1024*1024)
}

fn image_source(payload:&str)->Option<String>{
    let start=payload.find("<image")?;
    let tag=&payload[start..];
    let end=tag.find('>')?;
    attribute(&tag[..end],"src").filter(|src|!src.is_empty())
}

fn attribute(xml:&str,name:&str)->Option<String>{
    let key=format!("{name}=\"");
    let start=xml.find(&key)?+key.len();
    let rest=&xml[start..];
    Some(rest[..rest.find('"')?].to_string())
}

fn query(uri:&str,key:&str)->Option<String>{
    let needle=format!("{key}=");
    let start=uri.find(&needle)?+needle.len();
    let rest=&uri[start..];
    let end=rest.find([';','&']).unwrap_or(rest.len());
    (!rest[..end].is_empty()).then(||rest[..end].to_string())
}

fn unescape(value:&str)->String{
    value.replace("&quot;","\"").replace("&#39;","'").replace("&lt;","<").replace("&gt;",">").replace("&amp;","&")
}

#[cfg(test)]
mod tests{
    use super::*;
    // 真实群消息 payload（截取首尾，保留 launch 与 image 两个属性）。
    const GROUP:&str=r#"<toast launch="ntqq-notification://externalCommand?subcmd=openExternalWindow;peerUid=161832939;peerUin=161832939;peerName=ACC FULLPUSH 神力科莎;chatType=2;fuin=1905308858;windowName=MainWindow;scene=aio;" activationType="protocol"><visual><binding template="ToastGeneric"><text>ACC FULLPUSH</text><image placement="appLogoOverride" hint-crop="circle" src="C:\Users\Microsoft\Documents\Tencent Files\1905308858\nt_qq\nt_data\avatar\group\88\s_88db29dcbe27695c75f509c3e9a01596"/></binding></visual></toast>"#;
    #[test]
    fn group_toast_yields_launch_uri_and_avatar_file(){
        let toast=parse(GROUP);
        assert_eq!(toast.launch.as_deref(),Some("ntqq-notification://externalCommand?subcmd=openExternalWindow;peerUid=161832939;peerUin=161832939;peerName=ACC FULLPUSH 神力科莎;chatType=2;fuin=1905308858;windowName=MainWindow;scene=aio;"));
        assert_eq!(toast.chat_type,Some(2));
        assert_eq!(toast.avatar.as_deref(),Some(std::path::Path::new(r"C:\Users\Microsoft\Documents\Tencent Files\1905308858\nt_qq\nt_data\avatar\group\88\s_88db29dcbe27695c75f509c3e9a01596")));
    }
    #[test]
    fn private_toast_is_recognised_and_xml_entities_are_decoded(){
        let payload=r#"<toast launch="ntqq-notification://externalCommand?subcmd=openExternalWindow;peerUid=u_123&amp;chatType=1;&#39;"><visual><binding template="ToastGeneric"><image src="C:\avatar\user\ab\s_ab"/></binding></visual></toast>"#;
        let toast=parse(payload);
        assert_eq!(toast.chat_type,Some(1));
        assert!(toast.launch.is_some_and(|uri|uri.contains("&amp;")==false && uri.contains("&chatType")));
        assert_eq!(toast.avatar.as_deref(),Some(std::path::Path::new(r"C:\avatar\user\ab\s_ab")));
    }
    #[test]
    fn payload_without_image_or_launch_degrades_to_empty(){
        assert_eq!(parse("<toast><visual><binding><text>只有文字</text></binding></visual></toast>"),QqToast::default());
        assert_eq!(parse(""),QqToast::default());
        // XML 属性顺序变化时同样要认得出来（image 不再是第一个属性）
        let reordered=parse(r#"<image hint-crop="circle" placement="appLogoOverride" src="C:\avatar\group\00\s_00"/>"#);
        assert_eq!(reordered.avatar.as_deref(),Some(std::path::Path::new(r"C:\avatar\group\00\s_00")));
    }
    #[test]
    fn unusable_avatar_paths_are_rejected(){
        let missing=std::env::temp_dir().join("daozai-avatar-missing");
        assert!(!usable_avatar(&missing));
        assert!(!usable_avatar(&std::env::temp_dir()));
    }
}
