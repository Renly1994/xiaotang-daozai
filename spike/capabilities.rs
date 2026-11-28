//! 插件能力表：每种能力用什么运行环境、归属到哪、输出落在哪里。
//!
//! 新开放一种能力，就在 [`CAPABILITIES`] 里加一行，再在宿主里接上它的输出位置。
//! 归属（owner）只决定插件设置放在哪个设置页，以及哪个模块关掉时整个插件暂停；
//! 一个脚本插件可以同时声明几种输出，各输出只受自己所在模块的开关影响。

/// 脚本插件的输出落在哪里。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Surface {
    /// 待办与置物里的信息卡片，读 `card`。
    TasksCard,
    /// 日历当天的标注，读 `marker`；旧版日程归属的 `card` 也落在这里。
    CalendarMarker,
    /// AI 额度面板的一个来源，读 `quota`。
    Quota,
    /// 页面管理里的独立状态页，读 `status`。
    StatusPage,
    /// 收起时由宿主轮播的短内容，读 `pinned`。
    IslandPinned,
    /// 外部只读来源的设备，进入原生无线设备列表。
    WirelessDevices,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Role {
    /// 脚本插件的一种输出。
    Output(Surface),
    /// 在同一个输出位置扩展数据格式。
    CanvasOutput,
    /// 脚本插件的辅助能力：设置、定时刷新、私有存储。
    Helper,
    /// 独占一个包的能力，包里除了设置不再有其他能力，归属固定。
    Exclusive { owner: &'static str },
}

pub(super) struct Capability {
    pub name: &'static str,
    pub runtime: &'static str,
    pub role: Role,
}

pub(super) const CAPABILITIES: &[Capability] = &[
    Capability { name: "card", runtime: "js", role: Role::Output(Surface::TasksCard) },
    Capability { name: "calendar.marker", runtime: "js", role: Role::Output(Surface::CalendarMarker) },
    Capability { name: "quota.source", runtime: "js", role: Role::Output(Surface::Quota) },
    Capability { name: "status.page", runtime: "js", role: Role::Output(Surface::StatusPage) },
    Capability { name: "status.canvas", runtime: "js", role: Role::CanvasOutput },
    Capability { name: "island.pinned", runtime: "js", role: Role::Output(Surface::IslandPinned) },
    Capability { name: "wireless.devices", runtime: "js", role: Role::Output(Surface::WirelessDevices) },
    Capability { name: "settings", runtime: "*", role: Role::Helper },
    Capability { name: "timer", runtime: "js", role: Role::Helper },
    Capability { name: "storage", runtime: "js", role: Role::Helper },
    Capability { name: "action", runtime: "js", role: Role::Helper },
    Capability { name: "event", runtime: "js", role: Role::Helper },
    Capability { name: "notice", runtime: "js", role: Role::Helper },
    Capability {
        name: "appearance.material",
        runtime: "material",
        role: Role::Exclusive { owner: "appearance" },
    },
    Capability { name: "appearance.skin", runtime: "assets", role: Role::Exclusive { owner: "appearance" } },
    Capability { name: "appearance.icon", runtime: "assets", role: Role::Exclusive { owner: "appearance" } },
    Capability { name: "appearance.pet", runtime: "assets", role: Role::Exclusive { owner: "appearance" } },
    Capability { name: "appearance.font", runtime: "assets", role: Role::Exclusive { owner: "appearance" } },
    Capability {
        name: "navigation.shortcuts",
        runtime: "assets",
        role: Role::Exclusive { owner: "pages" },
    },
    Capability { name: "mail.view", runtime: "assets", role: Role::Exclusive { owner: "mail" } },
    Capability {
        name: "wireless.receivers",
        runtime: "assets",
        role: Role::Exclusive { owner: "wireless" },
    },
];

/// 脚本插件可以选的归属，以及这个归属至少要带哪种输出（设置放在那里，总得在那里看得到东西）。
pub(super) const SCRIPT_OWNERS: &[(&str, &[Surface])] = &[
    ("tasks", &[Surface::TasksCard]),
    ("calendar", &[Surface::CalendarMarker]),
    ("quota", &[Surface::Quota]),
    ("status", &[Surface::StatusPage]),
    ("wireless", &[Surface::WirelessDevices]),
];

pub(super) fn find(name: &str) -> Option<&'static Capability> {
    CAPABILITIES.iter().find(|capability| capability.name == name)
}

/// 某个能力在这个归属下实际落在哪里。旧版「日程归属 + card」是日历标注。
pub(super) fn surface_of(name: &str, owner: &str) -> Option<Surface> {
    match find(name)?.role {
        Role::Output(Surface::TasksCard) if owner == "calendar" => Some(Surface::CalendarMarker),
        Role::Output(surface) => Some(surface),
        Role::CanvasOutput => Some(Surface::StatusPage),
        _ => None,
    }
}

/// 核对一份清单的能力组合。只看能力、运行环境和归属；各类包自己的细则由调用方另查。
pub(super) fn check(runtime: &str, owner: &str, names: &[String]) -> Result<Vec<Surface>, String> {
    if names.iter().any(|name| name == "wireless.devices")
        && (runtime != "js" || owner != "wireless" || names.iter().any(|name|
            !matches!(name.as_str(), "wireless.devices" | "timer" | "settings" | "storage"))) {
        return Err("wireless.devices 只接受 wireless 归属的只读脚本来源及 timer、settings、storage".into());
    }
    let mut seen = std::collections::BTreeSet::new();
    let mut surfaces = Vec::new();
    let mut exclusive = None;
    for name in names {
        let Some(capability) = find(name) else {
            return Err(format!("当前版本不支持能力「{name}」"));
        };
        if !seen.insert(name.as_str()) {
            return Err(format!("能力「{name}」重复声明"));
        }
        if capability.runtime != "*" && capability.runtime != runtime {
            return Err(format!("能力「{name}」需要 {} 运行环境", capability.runtime));
        }
        match capability.role {
            Role::Exclusive { owner: required } => {
                if owner != required {
                    return Err(format!("能力「{name}」的归属须为 {required}"));
                }
                exclusive = Some(name.as_str());
            }
            Role::Output(_) => {
                let surface = surface_of(name, owner).unwrap();
                if surfaces.contains(&surface) {
                    return Err("同一个输出位置只能声明一次，日程归属的 card 已经是日历标注".into());
                }
                surfaces.push(surface);
            }
            Role::CanvasOutput | Role::Helper => {}
        }
    }
    if seen.contains("action") && !seen.contains("status.page") {
        return Err("按钮和表单需要同时声明 status.page".into());
    }
    if seen.contains("status.canvas") && !seen.contains("status.page") {
        return Err("自绘画布需要同时声明 status.page".into());
    }
    if let Some(name) = exclusive {
        if names.iter().any(|other| other != name && other != "settings") {
            return Err(format!("能力「{name}」只能和 settings 一起声明"));
        }
        return Ok(surfaces);
    }
    if runtime != "js" {
        return Err(format!("{runtime} 插件需要声明对应的专属能力"));
    }
    let Some((_, home)) = SCRIPT_OWNERS.iter().find(|(key, _)| *key == owner) else {
        return Err(format!("脚本插件的归属须为 tasks、calendar、quota、status 或 wireless，当前为 {owner}"));
    };
    if !home.iter().any(|surface| surfaces.contains(surface)) {
        return Err(match owner {
            "quota" => "AI 额度归属的插件需要声明 quota.source".into(),
            "status" => "状态页归属的插件需要声明 status.page".into(),
            "calendar" => "日程归属的插件需要声明 card 或 calendar.marker".into(),
            "wireless" => "无线设备归属的脚本需要声明 wireless.devices".into(),
            _ => "待办归属的插件需要声明 card".into(),
        });
    }
    Ok(surfaces)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|name| (*name).to_owned()).collect()
    }

    #[test]
    fn table_accepts_every_shipped_combination() {
        for (runtime, owner, list) in [
            ("js", "tasks", &["card", "settings", "timer", "storage"][..]),
            ("js", "calendar", &["card", "settings"]),
            ("js", "quota", &["quota.source", "settings", "timer"]),
            ("js", "status", &["status.page", "timer", "storage"]),
            ("js", "status", &["status.page", "status.canvas"]),
            ("material", "appearance", &["appearance.material", "settings"]),
            ("assets", "pages", &["navigation.shortcuts"]),
            ("assets", "wireless", &["wireless.receivers"]),
        ] {
            assert!(check(runtime, owner, &names(list)).is_ok(), "{owner} {list:?}");
        }
    }

    #[test]
    fn one_script_can_fill_several_surfaces() {
        assert_eq!(
            check("js", "tasks", &names(&["card", "calendar.marker", "timer"])).unwrap(),
            [Surface::TasksCard, Surface::CalendarMarker]
        );
        assert_eq!(
            check("js", "status", &names(&["status.page", "card"])).unwrap(),
            [Surface::StatusPage, Surface::TasksCard]
        );
        assert_eq!(
            check("js", "calendar", &names(&["calendar.marker"])).unwrap(),
            [Surface::CalendarMarker]
        );
    }

    #[test]
    fn table_rejects_unknown_duplicate_and_mismatched_capabilities() {
        for (runtime, owner, list) in [
            ("js", "tasks", &["network", "card"][..]),
            ("js", "status", &["status.canvas"]),
            ("js", "tasks", &["card", "card"]),
            ("js", "calendar", &["card", "calendar.marker"]),
            ("js", "tasks", &["calendar.marker"]),
            ("js", "quota", &["card"]),
            ("js", "appearance", &["card"]),
            ("js", "tasks", &["card", "appearance.material"]),
            ("material", "appearance", &["appearance.material", "timer"]),
            ("material", "tasks", &["appearance.material"]),
            ("assets", "pages", &["navigation.shortcuts", "settings", "card"]),
            ("js", "tasks", &["settings"]),
        ] {
            assert!(check(runtime, owner, &names(list)).is_err(), "{owner} {list:?}");
        }
    }
}
