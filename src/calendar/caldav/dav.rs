//! 独立 CalDAV 协议层。调用者在数据锁之外执行网络请求，并只提交完整成功的结果。
//!
//! 不解析 ICS、不依赖 organizer；所有地址必须属于构造时指定的 origin。
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt;
use std::io::Read;
use std::time::Duration;

use base64::Engine;
use url::Url;

const DAV: &str = "DAV:";
const CAL: &str = "urn:ietf:params:xml:ns:caldav";
const MAX_BODY: u64 = 32 * 1024 * 1024;
const DISCOVERY: &str = r#"<d:propfind xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav"><d:prop><d:resourcetype/><d:displayname/><d:current-user-principal/><c:calendar-home-set/><c:supported-calendar-component-set/><d:current-user-privilege-set/><d:supported-report-set/></d:prop></d:propfind>"#;
const LISTING: &str =
    r#"<d:propfind xmlns:d="DAV:"><d:prop><d:resourcetype/><d:getetag/></d:prop></d:propfind>"#;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Component {
    Event,
    Todo,
}
impl Component {
    pub fn name(self) -> &'static str {
        match self {
            Self::Event => "VEVENT",
            Self::Todo => "VTODO",
        }
    }
}

/// 未返回权限属性时保持 unknown；只读探测可确认 read，不推断写入授权。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Privileges {
    pub known: bool,
    pub read: bool,
    pub write_content: bool,
    pub bind: bool,
    pub unbind: bool,
}
impl Privileges {
    pub fn writable(&self) -> bool {
        self.known && self.read && self.write_content && self.bind && self.unbind
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Collection {
    pub url: String,
    pub display_name: Option<String>,
    pub components: BTreeSet<String>,
    /// 包括 RFC 4791 §5.2.3 在属性缺失时规定的默认能力。
    pub components_known: bool,
    pub privileges: Privileges,
    pub supports_sync: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResourceRef {
    pub href: String,
    pub etag: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CalendarResource {
    pub href: String,
    pub etag: String,
    pub ics: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResourceListing {
    pub collection_url: String,
    pub resources: Vec<ResourceRef>,
}

/// full=true 时 resources 是完整清单；否则只有变更和显式删除。
/// token=None 表示全量回退，不能保留旧 token。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyncResult {
    pub collection_url: String,
    pub full: bool,
    pub resources: Vec<ResourceRef>,
    pub deleted: Vec<String>,
    pub token: Option<String>,
}

/// 包括空正文的 201/204；保留 HTTP 返回信息，供调用者处理服务端规范化和冲突。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DavResponse {
    pub status: u16,
    pub etag: Option<String>,
    pub location: Option<String>,
    pub final_url: String,
    pub body: String,
}

#[derive(Clone, Debug)]
pub struct DavError {
    pub message: String,
    pub status: Option<u16>,
    pub url: Option<String>,
    pub response: Option<Box<DavResponse>>,
}
impl DavError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            status: None,
            url: None,
            response: None,
        }
    }
    fn http(response: DavResponse) -> Self {
        Self {
            message: format!("CalDAV 请求失败（HTTP {}）", response.status),
            status: Some(response.status),
            url: Some(response.final_url.clone()),
            response: Some(Box::new(response)),
        }
    }
    fn resource(message: &str, status: u16, href: &str) -> Self {
        Self {
            message: message.into(),
            status: Some(status),
            url: Some(href.into()),
            response: None,
        }
    }
}
impl fmt::Display for DavError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}
impl std::error::Error for DavError {}

/// namespace URI 和 local name 分开保存，不能按前缀或 local name 猜测属性。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct XmlProperty {
    pub namespace: String,
    pub name: String,
    pub text: String,
    pub children: Vec<XmlProperty>,
}
impl XmlProperty {
    fn is(&self, ns: &str, name: &str) -> bool {
        self.namespace == ns && self.name == name
    }
    fn child(&self, ns: &str, name: &str) -> Option<&Self> {
        self.children.iter().find(|p| p.is(ns, name))
    }
    fn contains(&self, ns: &str, name: &str) -> bool {
        self.is(ns, name) || self.children.iter().any(|p| p.contains(ns, name))
    }
    fn text_only(&self) -> Result<&str, DavError> {
        if !self.children.is_empty() {
            return Err(DavError::new("CalDAV 文本属性包含非文本 XML，不能完整读取"));
        }
        Ok(&self.text)
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DavPropstat {
    pub status: u16,
    pub properties: Vec<XmlProperty>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DavItem {
    pub href: String,
    pub status: Option<u16>,
    pub propstats: Vec<DavPropstat>,
}
impl DavItem {
    fn ensure_success(&self) -> Result<(), DavError> {
        if let Some(code) = self.status {
            if !(200..300).contains(&code) {
                return Err(DavError::resource("CalDAV 资源读取失败", code, &self.href));
            }
        }
        Ok(())
    }
    fn property(&self, ns: &str, name: &str) -> Result<Option<&XmlProperty>, DavError> {
        self.ensure_success()?;
        let mut found = None;
        let mut failure = None;
        for stat in &self.propstats {
            for property in &stat.properties {
                if !property.is(ns, name) {
                    continue;
                }
                if (200..300).contains(&stat.status) {
                    if found.replace(property).is_some() {
                        return Err(DavError::new("CalDAV 返回了重复属性"));
                    }
                } else if failure.replace(stat.status).is_some() {
                    return Err(DavError::new("CalDAV 返回了重复失败属性"));
                }
            }
        }
        if let Some(code) = failure {
            if found.is_some() {
                return Err(DavError::new("CalDAV 同一属性的状态相互矛盾"));
            }
            if code != 404 {
                return Err(DavError::resource("CalDAV 属性读取失败", code, &self.href));
            }
        }
        Ok(found)
    }
    fn required(&self, ns: &str, name: &str) -> Result<&XmlProperty, DavError> {
        self.property(ns, name)?
            .ok_or_else(|| DavError::new(format!("CalDAV 缺少必要属性：{name}")))
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DavMultistatus {
    pub responses: Vec<DavItem>,
    pub sync_token: Option<String>,
}

fn element(node: roxmltree::Node<'_, '_>, ns: &str, name: &str) -> bool {
    node.is_element() && node.tag_name().namespace() == Some(ns) && node.tag_name().name() == name
}
fn status(text: &str) -> Result<u16, DavError> {
    let mut parts = text.split_whitespace();
    let protocol = parts.next().unwrap_or_default();
    let code = parts.next().and_then(|v| v.parse::<u16>().ok());
    match code {
        Some(code @ 100..=599) if protocol.starts_with("HTTP/") => Ok(code),
        _ => Err(DavError::new("CalDAV XML 包含无效 HTTP 状态")),
    }
}
fn xml_property(node: roxmltree::Node<'_, '_>) -> XmlProperty {
    let mut children: Vec<_> = node
        .children()
        .filter(|n| n.is_element())
        .map(xml_property)
        .collect();
    // comp 的 name 是 XML attribute；用内部独立节点保存，避免与 namespace 属性混淆。
    if element(node, CAL, "comp") {
        if let Some(name) = node.attribute("name") {
            children.push(XmlProperty {
                namespace: String::new(),
                name: "name".into(),
                text: name.into(),
                children: vec![],
            });
        }
    }
    XmlProperty {
        namespace: node.tag_name().namespace().unwrap_or_default().into(),
        name: node.tag_name().name().into(),
        text: node
            .children()
            .filter(|n| n.is_text())
            .filter_map(|n| n.text())
            .collect(),
        children,
    }
}

fn xml_text(node: roxmltree::Node<'_, '_>) -> Result<String, DavError> {
    if node.children().any(|n| n.is_element()) {
        return Err(DavError::new("CalDAV 文本字段包含嵌套 XML"));
    }
    Ok(node
        .children()
        .filter(|n| n.is_text())
        .filter_map(|n| n.text())
        .collect())
}

pub fn parse_multistatus(xml: &str) -> Result<DavMultistatus, DavError> {
    let doc =
        roxmltree::Document::parse(xml).map_err(|_| DavError::new("CalDAV 返回的 XML 无效"))?;
    let root = doc.root_element();
    if !element(root, DAV, "multistatus") {
        return Err(DavError::new("CalDAV 未返回 DAV multistatus"));
    }
    // RFC 6578 可在 207 中携带 collection 级 507 和新的 token；这是截断页，不能提交。
    if root.descendants().any(|n| {
        element(n, DAV, "limit")
            || element(n, DAV, "number-of-matches-within-limits")
            || element(n, CAL, "number-of-matches-within-limits")
    }) {
        return Err(DavError::new(
            "CalDAV 返回了分页或 limit 条件，不能确认完整清单",
        ));
    }
    for node in root.descendants().filter(|n| element(*n, DAV, "status")) {
        if status(&xml_text(node)?)? == 507 {
            return Err(DavError::resource(
                "CalDAV 返回截断的 507 清单，不能推进 token",
                507,
                "",
            ));
        }
    }
    let mut responses = Vec::new();
    let mut token = None;
    for node in root.children().filter(|n| n.is_element()) {
        if element(node, DAV, "sync-token") {
            if token.is_some() {
                return Err(DavError::new("CalDAV 返回了重复 sync-token"));
            }
            let text = xml_text(node)?;
            let value = text.trim();
            if value.is_empty() {
                return Err(DavError::new("CalDAV 返回了空 sync-token"));
            }
            token = Some(value.to_owned());
        } else if element(node, DAV, "response") {
            let hrefs: Vec<_> = node
                .children()
                .filter(|n| element(*n, DAV, "href"))
                .collect();
            if hrefs.len() != 1 {
                return Err(DavError::new("CalDAV response 必须包含唯一 href"));
            }
            let href = xml_text(hrefs[0])?.trim().to_owned();
            if href.is_empty() {
                return Err(DavError::new("CalDAV 返回了空资源地址"));
            }
            let mut response_status = None;
            let mut propstats = Vec::new();
            for child in node.children().filter(|n| n.is_element()) {
                if element(child, DAV, "status") {
                    if response_status
                        .replace(status(child.text().unwrap_or_default())?)
                        .is_some()
                    {
                        return Err(DavError::new("CalDAV response 状态重复"));
                    }
                } else if element(child, DAV, "propstat") {
                    let statuses: Vec<_> = child
                        .children()
                        .filter(|n| element(*n, DAV, "status"))
                        .collect();
                    let props: Vec<_> = child
                        .children()
                        .filter(|n| element(*n, DAV, "prop"))
                        .collect();
                    if statuses.len() != 1 || props.len() != 1 {
                        return Err(DavError::new("CalDAV propstat 缺少唯一 prop/status"));
                    }
                    propstats.push(DavPropstat {
                        status: status(statuses[0].text().unwrap_or_default())?,
                        properties: props[0]
                            .children()
                            .filter(|n| n.is_element())
                            .map(xml_property)
                            .collect(),
                    });
                }
            }
            if response_status.is_some() && !propstats.is_empty() {
                return Err(DavError::new("CalDAV response 同时包含资源状态和属性状态"));
            }
            if response_status.is_none() && propstats.is_empty() {
                return Err(DavError::new("CalDAV response 缺少状态"));
            }
            responses.push(DavItem {
                href,
                status: response_status,
                propstats,
            });
        } else if element(node, DAV, "limit")
            || element(node, DAV, "number-of-matches-within-limits")
        {
            return Err(DavError::new("CalDAV 返回了不完整的分页清单"));
        }
    }
    Ok(DavMultistatus {
        responses,
        sync_token: token,
    })
}

pub struct DavClient {
    server: Url,
    authorization: String,
    agent: ureq::Agent,
}
impl DavClient {
    pub fn new(server: &str, username: &str, password: &str) -> Result<Self, DavError> {
        let server =
            Url::parse(server.trim()).map_err(|_| DavError::new("CalDAV 服务器地址无效"))?;
        validate_url(&server)?;
        if username.contains(':') || username.chars().any(char::is_control) {
            return Err(DavError::new("CalDAV 用户名不能包含冒号或控制字符"));
        }
        let authorization = format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD.encode(format!("{username}:{password}"))
        );
        Ok(Self {
            server,
            authorization,
            agent: ureq::AgentBuilder::new()
                .redirects(0)
                .timeout_connect(Duration::from_secs(10))
                .timeout_read(Duration::from_secs(20))
                .timeout_write(Duration::from_secs(20))
                .timeout(Duration::from_secs(30))
                .user_agent(concat!("Daozai/", env!("CARGO_PKG_VERSION")))
                .build(),
        })
    }

    /// 相对路径按传入 base（发现后的最终 URL）解析，保留 URL 编码和 query。
    pub fn resolve_href(&self, base: &str, href: &str) -> Result<String, DavError> {
        let base = Url::parse(base).map_err(|_| DavError::new("CalDAV 基准地址无效"))?;
        self.trust(&base)?;
        if href.trim().is_empty() || href.chars().any(char::is_control) || href.contains('\\') {
            return Err(DavError::new("CalDAV 资源地址无效"));
        }
        let target = base
            .join(href)
            .map_err(|_| DavError::new("CalDAV 资源地址无效"))?;
        self.trust(&target)?;
        Ok(target.into())
    }
    fn trust(&self, target: &Url) -> Result<(), DavError> {
        validate_url(target)?;
        if target.origin() != self.server.origin() {
            return Err(DavError::new("CalDAV 返回了跨源地址，已阻止发送凭据"));
        }
        Ok(())
    }
    fn resolve(&self, href: &str) -> Result<String, DavError> {
        self.resolve_href(self.server.as_str(), href)
    }
    fn send(
        &self,
        method: &str,
        href: &str,
        depth: Option<&str>,
        body: &str,
        condition: Option<(&str, &str)>,
    ) -> Result<DavResponse, DavError> {
        let mut target = self.resolve(href)?;
        let mut visited = BTreeSet::new();
        for _ in 0..6 {
            if !visited.insert(target.clone()) {
                return Err(DavError::new("CalDAV 重定向形成循环"));
            }
            let parsed = Url::parse(&target).map_err(|_| DavError::new("CalDAV 请求地址无效"))?;
            self.trust(&parsed)?;
            let mut request = self
                .agent
                .request(method, &target)
                .set("Authorization", &self.authorization)
                .set(
                    "Accept",
                    if method == "GET" {
                        "text/calendar"
                    } else {
                        "application/xml, text/xml"
                    },
                );
            if let Some(depth) = depth {
                request = request.set("Depth", depth);
            }
            if let Some((name, value)) = condition {
                request = request.set(name, value);
            }
            if method == "PUT" {
                request = request.set("Content-Type", "text/calendar; charset=utf-8");
            } else if !body.is_empty() {
                request = request.set("Content-Type", "application/xml; charset=utf-8");
            }
            let response = match if body.is_empty() {
                request.call()
            } else {
                request.send_bytes(body.as_bytes())
            } {
                Ok(response) | Err(ureq::Error::Status(_, response)) => response,
                Err(ureq::Error::Transport(_)) => {
                    return Err(DavError {
                        message: "CalDAV 网络请求失败，操作结果可能未知".into(),
                        status: None,
                        url: Some(target),
                        response: None,
                    })
                }
            };
            let code = response.status();
            let etag = response.header("ETag").map(str::to_owned);
            let location = response.header("Location").map(str::to_owned);
            let incomplete = |message: &str| DavError {
                message: message.into(),
                status: Some(code),
                url: Some(target.clone()),
                response: Some(Box::new(DavResponse {
                    status: code,
                    etag: etag.clone(),
                    location: location.clone(),
                    final_url: target.clone(),
                    body: String::new(),
                })),
            };
            let mut bytes = Vec::new();
            response
                .into_reader()
                .take(MAX_BODY + 1)
                .read_to_end(&mut bytes)
                .map_err(|_| incomplete("CalDAV 响应读取失败，不能确认完整结果"))?;
            if bytes.len() as u64 > MAX_BODY {
                return Err(incomplete("CalDAV 响应超过大小限制，不能确认完整结果"));
            }
            let body =
                String::from_utf8(bytes).map_err(|_| incomplete("CalDAV 响应不是有效 UTF-8"))?;
            let response = DavResponse {
                status: code,
                etag,
                location,
                final_url: target.clone(),
                body,
            };
            if matches!(code, 301 | 302 | 303 | 307 | 308) {
                // 条件写入绑定固定 href，不跟随任何写入重定向，以免越过已选集合或重复创建。
                // 只读 303 也不能改成 GET 后冒充 PROPFIND/REPORT 成功。
                if matches!(method, "PUT" | "DELETE") || code == 303 {
                    return Err(DavError::http(response));
                }
                let location = response
                    .location
                    .as_deref()
                    .ok_or_else(|| DavError::http(response.clone()))?;
                target = self.resolve_href(&target, location).map_err(|mut error| {
                    error.status = Some(response.status);
                    error.url = Some(response.final_url.clone());
                    error.response = Some(Box::new(response.clone()));
                    error
                })?;
                continue;
            }
            if !(200..300).contains(&code) {
                return Err(DavError::http(response));
            }
            return Ok(response);
        }
        Err(DavError::new("CalDAV 重定向次数过多"))
    }
    fn propfind(
        &self,
        url: &str,
        depth: &str,
        body: &str,
    ) -> Result<(DavResponse, DavMultistatus), DavError> {
        let response = self.send("PROPFIND", url, Some(depth), body, None)?;
        let parsed = multistatus_response(&response)?;
        Ok((response, parsed))
    }

    /// 仅初始根入口的 400/403/404/405 可回退 well-known；认证或已发现地址失败立即返回。
    pub fn discover(&self) -> Result<Vec<Collection>, DavError> {
        let mut queue = VecDeque::from([(self.server.to_string(), "1", true)]);
        let mut visited = BTreeSet::new();
        let mut calendars = BTreeMap::new();
        while let Some((url, depth, initial)) = queue.pop_front() {
            if visited.len() >= 24 {
                return Err(DavError::new("CalDAV 发现地址过多"));
            }
            if !visited.insert(url.clone()) {
                continue;
            }
            let (response, parsed) = match self.propfind(&url, depth, DISCOVERY) {
                Ok(value) => value,
                Err(error)
                    if initial
                        && self.server.path() == "/"
                        && error.url.as_deref() == Some(url.as_str())
                        && matches!(error.status, Some(400 | 403 | 404 | 405)) =>
                {
                    queue.push_back((
                        self.server.join(".well-known/caldav").unwrap().into(),
                        "0",
                        false,
                    ));
                    continue;
                }
                Err(error) => return Err(error),
            };
            visited.insert(response.final_url.clone());
            for item in &parsed.responses {
                item.ensure_success()?;
                let href = self.resolve_href(&response.final_url, &item.href)?;
                if let Some(collection) = self.collection_from(item, &href)? {
                    calendars.insert(collection.url.clone(), collection);
                }
                for (ns, name, next_depth) in [
                    (DAV, "current-user-principal", "0"),
                    (CAL, "calendar-home-set", "1"),
                ] {
                    if let Some(property) = item.property(ns, name)? {
                        for link in property.children.iter().filter(|p| p.is(DAV, "href")) {
                            let next = self.resolve_href(&response.final_url, link.text.trim())?;
                            if !visited.contains(&next) {
                                queue.push_back((next, next_depth, false));
                            }
                        }
                    }
                }
            }
        }
        if calendars.is_empty() {
            return Err(DavError::new("没有发现可用的 CalDAV calendar collection"));
        }
        Ok(calendars.into_values().collect())
    }
    fn collection_from(&self, item: &DavItem, href: &str) -> Result<Option<Collection>, DavError> {
        let Some(kind) = item.property(DAV, "resourcetype")? else {
            return Ok(None);
        };
        if kind.child(CAL, "calendar").is_none() {
            return Ok(None);
        }
        if kind.child(DAV, "collection").is_none() {
            return Err(DavError::new("CalDAV calendar 缺少 collection 类型"));
        }
        let components_property = item.property(CAL, "supported-calendar-component-set")?;
        let mut components = BTreeSet::new();
        if let Some(property) = components_property {
            for comp in property.children.iter().filter(|p| p.is(CAL, "comp")) {
                if let Some(name) = comp.child("", "name") {
                    components.insert(name.text.clone());
                }
            }
        } else {
            // RFC 4791 §5.2.3：此属性未定义时，服务器 MUST 接受全部组件类型。
            components.extend(["VEVENT".into(), "VTODO".into()]);
        }
        let mut privileges = Privileges::default();
        if let Some(property) = item.property(DAV, "current-user-privilege-set")? {
            privileges.known = true;
            for privilege in property.children.iter().filter(|p| p.is(DAV, "privilege")) {
                let all = privilege.child(DAV, "all").is_some();
                let write = all || privilege.child(DAV, "write").is_some();
                privileges.read |= all || privilege.child(DAV, "read").is_some();
                privileges.write_content |=
                    write || privilege.child(DAV, "write-content").is_some();
                privileges.bind |= write || privilege.child(DAV, "bind").is_some();
                privileges.unbind |= write || privilege.child(DAV, "unbind").is_some();
            }
        }
        Ok(Some(Collection {
            url: href.into(),
            display_name: item.property(DAV, "displayname")?.map(|p| p.text.clone()),
            components,
            components_known: true,
            privileges,
            supports_sync: item
                .property(DAV, "supported-report-set")?
                .is_some_and(|p| p.contains(DAV, "sync-collection")),
        }))
    }

    /// 显式路径只验证该地址；属性无法确认时失败，不发现其他 collection 代替。
    pub fn validate_collection(
        &self,
        path: &str,
        component: Component,
        require_write: bool,
    ) -> Result<Collection, DavError> {
        let url = self.resolve(path)?;
        let (response, parsed) = self.propfind(&url, "0", DISCOVERY)?;
        let mut found = None;
        for item in &parsed.responses {
            let href = self.resolve_href(&response.final_url, &item.href)?;
            if same_collection(&href, &response.final_url) {
                if found.is_some() {
                    return Err(DavError::new("CalDAV 返回了重复 collection"));
                }
                found = self.collection_from(item, &response.final_url)?;
            } else {
                return Err(DavError::new("CalDAV 显式路径返回了其他 collection"));
            }
        }
        let mut collection =
            found.ok_or_else(|| DavError::new("指定路径不是 CalDAV calendar collection"))?;
        if !collection.components_known || !collection.components.contains(component.name()) {
            return Err(DavError::new(format!(
                "无法确认指定集合支持 {}",
                component.name()
            )));
        }
        if !collection.privileges.known {
            self.probe_read(&collection.url)?;
            // 只确认本次只读查询可执行，不推断 ACL、bind/unbind 或完整写入授权。
            collection.privileges.read = true;
        }
        if !collection.privileges.read {
            return Err(DavError::new("无法确认指定集合的读取权限"));
        }
        if require_write && !collection.privileges.writable() {
            return Err(DavError::new("无法确认指定集合的完整写入权限"));
        }
        Ok(collection)
    }

    fn probe_read(&self, collection: &str) -> Result<(), DavError> {
        let body = r#"<c:calendar-query xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav"><d:prop><c:calendar-data/></d:prop><c:filter><c:comp-filter name="VCALENDAR"/></c:filter></c:calendar-query>"#;
        let response = self.send("REPORT", collection, Some("1"), body, None)?;
        if !same_collection(collection, &response.final_url) {
            return Err(DavError::new("CalDAV 读取验证跳转至其他集合"));
        }
        let parsed = multistatus_response(&response)?;
        let mut seen = BTreeSet::new();
        for item in parsed.responses {
            item.ensure_success()?;
            let href = self.resolve_href(&response.final_url, &item.href)?;
            self.resource_in_collection(collection, &href)?;
            if !seen.insert(href) || item.required(CAL, "calendar-data")?.text_only()?.is_empty() {
                return Err(DavError::new("CalDAV 只读验证未返回完整、唯一的资源内容"));
            }
        }
        Ok(())
    }

    /// 无组件/日期过滤、无 expand。只有完整清单成功时返回，包括无日期 VTODO。
    pub fn list_resources(&self, collection: &str) -> Result<ResourceListing, DavError> {
        let (response, parsed) = self.propfind(&self.resolve(collection)?, "1", LISTING)?;
        let mut resources = BTreeMap::new();
        let mut saw_self = false;
        for item in &parsed.responses {
            item.ensure_success()?;
            let href = self.resolve_href(&response.final_url, &item.href)?;
            if same_collection(&href, &response.final_url) {
                if saw_self {
                    return Err(DavError::new("CalDAV 清单包含重复集合"));
                }
                saw_self = true;
                let kind = item.required(DAV, "resourcetype")?;
                if kind.child(DAV, "collection").is_none() {
                    return Err(DavError::new("CalDAV 清单缺少集合类型"));
                }
                continue;
            }
            self.resource_in_collection(&response.final_url, &href)?;
            let kind = item.required(DAV, "resourcetype")?;
            if kind.child(DAV, "collection").is_some() {
                return Err(DavError::new(
                    "CalDAV 日历集合内出现子集合，不能确认完整资源清单",
                ));
            }
            let etag = item
                .required(DAV, "getetag")?
                .text_only()?
                .trim()
                .to_owned();
            require_strong_etag(&etag)?;
            if resources
                .insert(href.clone(), ResourceRef { href, etag })
                .is_some()
            {
                return Err(DavError::new("CalDAV 清单包含重复资源"));
            }
        }
        if !saw_self {
            return Err(DavError::new("CalDAV 清单未确认集合本身，不能推断删除"));
        }
        Ok(ResourceListing {
            collection_url: response.final_url,
            resources: resources.into_values().collect(),
        })
    }
    fn resource_in_collection(&self, collection: &str, resource: &str) -> Result<(), DavError> {
        let collection =
            Url::parse(collection).map_err(|_| DavError::new("CalDAV 集合地址无效"))?;
        let resource = Url::parse(resource).map_err(|_| DavError::new("CalDAV 资源地址无效"))?;
        self.trust(&collection)?;
        self.trust(&resource)?;
        let prefix = format!("{}/", collection.path().trim_end_matches('/'));
        if !resource.path().starts_with(&prefix) || resource.path() == prefix {
            return Err(DavError::new("CalDAV 资源位于指定集合之外"));
        }
        // 编码的斜杠/反斜杠会改变服务器路径分段语义，拒绝而非解码重写 URL。
        let lower = resource.path().to_ascii_lowercase();
        if lower.contains("%2f") || lower.contains("%5c") {
            return Err(DavError::new("CalDAV 资源地址包含不明确的路径分隔符"));
        }
        Ok(())
    }

    /// 按 100 个 href 分批；任一缺失、额外、失败或版本变化都不返回部分资源。
    pub fn multiget(
        &self,
        collection: &str,
        requested: &[ResourceRef],
    ) -> Result<Vec<CalendarResource>, DavError> {
        let collection = self.resolve(collection)?;
        let mut wanted = BTreeMap::new();
        for resource in requested {
            let href = self.resolve_href(&collection, &resource.href)?;
            self.resource_in_collection(&collection, &href)?;
            require_strong_etag(&resource.etag)?;
            if wanted.insert(href, resource.etag.as_str()).is_some() {
                return Err(DavError::new("multiget 请求包含重复资源"));
            }
        }
        let entries: Vec<_> = wanted.iter().collect();
        let mut result = BTreeMap::new();
        for batch in entries.chunks(100) {
            let refs: String = batch
                .iter()
                .map(|(href, _)| format!("<d:href>{}</d:href>", escape_xml(href)))
                .collect();
            let body = format!(
                r#"<c:calendar-multiget xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav"><d:prop><d:getetag/><c:calendar-data/></d:prop>{refs}</c:calendar-multiget>"#
            );
            let response = self.send("REPORT", &collection, Some("1"), &body, None)?;
            if !same_collection(&response.final_url, &collection) {
                return Err(DavError::new("multiget 重定向改变了集合地址，请重新列举"));
            }
            let parsed = multistatus_response(&response)?;
            let batch_set: BTreeSet<_> = batch.iter().map(|(href, _)| href.as_str()).collect();
            for item in parsed.responses {
                item.ensure_success()?;
                let href = self.resolve_href(&response.final_url, &item.href)?;
                if !batch_set.contains(href.as_str()) {
                    return Err(DavError::new("multiget 返回了未请求的资源"));
                }
                let etag = item
                    .required(DAV, "getetag")?
                    .text_only()?
                    .trim()
                    .to_owned();
                require_strong_etag(&etag)?;
                if wanted.get(&href).copied() != Some(etag.as_str()) {
                    return Err(DavError::new(
                        "CalDAV 资源在读取期间发生变化，请重试完整同步",
                    ));
                }
                let ics = item.required(CAL, "calendar-data")?.text_only()?.to_owned();
                if ics.is_empty() {
                    return Err(DavError::new("multiget 返回了空 calendar-data"));
                }
                if result
                    .insert(href.clone(), CalendarResource { href, etag, ics })
                    .is_some()
                {
                    return Err(DavError::new("multiget 返回重复资源"));
                }
            }
            if batch
                .iter()
                .any(|(href, _)| !result.contains_key(href.as_str()))
            {
                return Err(DavError::new("multiget 未完整返回所请求资源"));
            }
        }
        Ok(result.into_values().collect())
    }

    /// RFC 6578。token 失效或服务不支持时回退完整 PROPFIND，并清除旧 token。
    /// 不接受截断/分页的 507；调用者成功读取全部变更正文后才能提交新 token。
    pub fn sync_collection(
        &self,
        collection: &str,
        token: Option<&str>,
    ) -> Result<SyncResult, DavError> {
        let collection = self.resolve(collection)?;
        if token.is_some_and(|v| v.trim().is_empty()) {
            return Err(DavError::new("不能使用空的已保存 sync-token"));
        }
        let body = format!(
            r#"<d:sync-collection xmlns:d="DAV:"><d:sync-token>{}</d:sync-token><d:sync-level>1</d:sync-level><d:prop><d:getetag/></d:prop></d:sync-collection>"#,
            escape_xml(token.unwrap_or_default())
        );
        let response = match self.send("REPORT", &collection, Some("1"), &body, None) {
            Ok(response) => response,
            Err(error) => {
                let invalid_token = error.status == Some(409)
                    || error.status == Some(403)
                        && error
                            .response
                            .as_ref()
                            .is_some_and(|r| has_dav_error(&r.body, "valid-sync-token"));
                if invalid_token || matches!(error.status, Some(404 | 405 | 501)) {
                    let listing = self.list_resources(&collection)?;
                    return Ok(SyncResult {
                        collection_url: listing.collection_url,
                        full: true,
                        resources: listing.resources,
                        deleted: vec![],
                        token: None,
                    });
                }
                return Err(error);
            }
        };
        if token.is_some() && !same_collection(&collection, &response.final_url) {
            return Err(DavError::new(
                "sync-token 对应集合发生重定向，请重新完整同步",
            ));
        }
        let parsed = multistatus_response(&response)?;
        let new_token = parsed
            .sync_token
            .ok_or_else(|| DavError::new("sync-collection 缺少新 sync-token，不能提交结果"))?;
        let mut resources = BTreeMap::new();
        let mut deleted = BTreeSet::new();
        let mut seen = BTreeSet::new();
        for item in parsed.responses {
            let href = self.resolve_href(&response.final_url, &item.href)?;
            self.resource_in_collection(&response.final_url, &href)?;
            if !seen.insert(href.clone()) {
                return Err(DavError::new("sync-collection 包含重复资源"));
            }
            if item.status == Some(404) {
                if token.is_none() {
                    return Err(DavError::new("初始完整同步出现删除记录，不能确认完整清单"));
                }
                deleted.insert(href);
                continue;
            }
            item.ensure_success()?;
            let etag = item
                .required(DAV, "getetag")?
                .text_only()?
                .trim()
                .to_owned();
            require_strong_etag(&etag)?;
            resources.insert(href.clone(), ResourceRef { href, etag });
        }
        Ok(SyncResult {
            collection_url: response.final_url,
            full: token.is_none(),
            resources: resources.into_values().collect(),
            deleted: deleted.into_iter().collect(),
            token: Some(new_token),
        })
    }

    pub fn get(&self, href: &str) -> Result<DavResponse, DavError> {
        let response = self.send("GET", href, None, "", None)?;
        if response.status != 200 {
            return Err(DavError::http(response));
        }
        Ok(response)
    }
    pub fn create(&self, href: &str, ics: &str) -> Result<DavResponse, DavError> {
        self.put(href, ics, ("If-None-Match", "*"))
    }
    pub fn update(&self, href: &str, etag: &str, ics: &str) -> Result<DavResponse, DavError> {
        require_strong_etag(etag)?;
        self.put(href, ics, ("If-Match", etag))
    }
    fn put(&self, href: &str, ics: &str, condition: (&str, &str)) -> Result<DavResponse, DavError> {
        if ics.is_empty() {
            return Err(DavError::new("不能上传空的日历资源"));
        }
        let response = self.send("PUT", href, None, ics, Some(condition))?;
        if !matches!(response.status, 200 | 201 | 204) {
            return Err(DavError::http(response));
        }
        Ok(response)
    }
    pub fn delete(&self, href: &str, etag: &str) -> Result<DavResponse, DavError> {
        require_strong_etag(etag)?;
        let response = self.send("DELETE", href, None, "", Some(("If-Match", etag)))?;
        if !matches!(response.status, 200 | 204) {
            return Err(DavError::http(response));
        }
        Ok(response)
    }
}

fn validate_url(url: &Url) -> Result<(), DavError> {
    let loopback = match url.host() {
        Some(url::Host::Domain(host)) => host.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    };
    if url.host().is_none()
        || !(url.scheme() == "https" || url.scheme() == "http" && loopback)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(DavError::new(
            "CalDAV 只允许可信 HTTPS 地址（本机回环允许 HTTP），地址不能含凭据或 fragment",
        ));
    }
    Ok(())
}
fn same_collection(a: &str, b: &str) -> bool {
    match (Url::parse(a), Url::parse(b)) {
        (Ok(a), Ok(b)) => {
            a.origin() == b.origin()
                && a.path().trim_end_matches('/') == b.path().trim_end_matches('/')
                && a.query() == b.query()
        }
        _ => false,
    }
}
pub fn require_strong_etag(etag: &str) -> Result<(), DavError> {
    let bytes = etag.as_bytes();
    if bytes.len() < 2
        || bytes[0] != b'"'
        || bytes[bytes.len() - 1] != b'"'
        || bytes[1..bytes.len() - 1]
            .iter()
            .any(|b| *b == b'"' || *b < 0x21 || *b == 0x7f)
    {
        return Err(DavError::new("缺少可用的 strong ETag，已停止条件写入"));
    }
    Ok(())
}
fn multistatus_response(response: &DavResponse) -> Result<DavMultistatus, DavError> {
    if response.status != 207 {
        return Err(DavError::http(response.clone()));
    }
    parse_multistatus(&response.body).map_err(|mut error| {
        error.url = Some(response.final_url.clone());
        error.response = Some(Box::new(response.clone()));
        error
    })
}
fn has_dav_error(xml: &str, name: &str) -> bool {
    roxmltree::Document::parse(xml).is_ok_and(|doc| {
        element(doc.root_element(), DAV, "error")
            && doc.root_element().children().any(|n| element(n, DAV, name))
    })
}
fn escape_xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::thread::{self, JoinHandle};

    struct Step {
        request: String,
        code: u16,
        headers: String,
        body: String,
        contains: Vec<String>,
    }
    fn step(request: &str, code: u16, body: impl Into<String>) -> Step {
        Step {
            request: request.into(),
            code,
            headers: String::new(),
            body: body.into(),
            contains: vec![],
        }
    }
    fn mock(steps: Vec<Step>) -> (String, JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let thread = thread::spawn(move || {
            for step in steps {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let request = read_request(&mut stream);
                assert!(request.starts_with(&step.request), "请求不匹配：{request}");
                assert!(
                    request
                        .to_ascii_lowercase()
                        .contains("authorization: basic dxnlcjpwyxnz\r\n"),
                    "缺少预期 Basic Auth"
                );
                for expected in step.contains {
                    assert!(
                        request.contains(&expected),
                        "请求缺少 {expected}：{request}"
                    );
                }
                write!(stream, "HTTP/1.1 {} Test\r\nContent-Length: {}\r\nContent-Type: application/xml; charset=utf-8\r\nConnection: close\r\n{}\r\n{}", step.code, step.body.len(), step.headers, step.body).unwrap();
            }
        });
        (base, thread)
    }
    fn read_request(stream: &mut TcpStream) -> String {
        let mut bytes = Vec::new();
        let mut buffer = [0u8; 4096];
        loop {
            let count = stream.read(&mut buffer).unwrap();
            assert!(count > 0, "请求中断");
            bytes.extend_from_slice(&buffer[..count]);
            if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&bytes[..end]);
                let length = headers
                    .lines()
                    .find_map(|line| {
                        line.split_once(':')
                            .filter(|(k, _)| k.eq_ignore_ascii_case("Content-Length"))
                            .map(|(_, v)| v.trim().parse::<usize>().unwrap())
                    })
                    .unwrap_or(0);
                if bytes.len() >= end + 4 + length {
                    return String::from_utf8(bytes).unwrap();
                }
            }
        }
    }
    fn client(base: &str) -> DavClient {
        DavClient::new(base, "user", "pass").unwrap()
    }
    fn multi(items: &str) -> String {
        format!(
            r#"<multistatus xmlns="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">{items}</multistatus>"#
        )
    }
    fn item(href: &str, props: &str) -> String {
        format!(
            r#"<response><href>{href}</href><propstat><prop>{props}</prop><status>HTTP/1.1 200 OK</status></propstat></response>"#
        )
    }
    fn collection_props(components: &str) -> String {
        format!(
            r#"<resourcetype><collection/><c:calendar/></resourcetype><c:supported-calendar-component-set>{components}</c:supported-calendar-component-set><current-user-privilege-set><privilege><read/></privilege><privilege><write/></privilege></current-user-privilege-set>"#
        )
    }
    fn listing(extra: &str) -> String {
        multi(
            &(item(
                "/cal/",
                "<resourcetype><collection/><c:calendar/></resourcetype>",
            ) + extra),
        )
    }
    fn reference(base: &str, name: &str) -> ResourceRef {
        ResourceRef {
            href: format!("{base}/cal/{name}"),
            etag: "\"v1\"".into(),
        }
    }

    #[test]
    fn namespace_and_mixed_propstats_keep_status_separate() {
        let xml = r#"<z:multistatus xmlns:z="DAV:" xmlns:e="urn:evil"><z:response><z:href>/cal/</z:href><z:propstat><z:prop><e:getetag>"wrong"</e:getetag><z:getetag>"right"</z:getetag></z:prop><z:status>HTTP/1.1 200 OK</z:status></z:propstat><z:propstat><z:prop><z:displayname/></z:prop><z:status>HTTP/1.1 404 Not Found</z:status></z:propstat></z:response></z:multistatus>"#;
        let parsed = parse_multistatus(xml).unwrap();
        let resource = &parsed.responses[0];
        assert_eq!(resource.required(DAV, "getetag").unwrap().text, "\"right\"");
        assert!(resource.property(DAV, "displayname").unwrap().is_none());
        assert_eq!(resource.propstats[1].status, 404);
        assert!(parse_multistatus("<multistatus/>").is_err());
    }
    #[test]
    fn malformed_status_duplicate_and_failed_properties_are_not_success() {
        for xml in [
            multi("<response><href>/x</href><propstat><prop/></propstat></response>"),
            multi("<response><href>/x</href><status>200</status></response>"),
            multi("<response><href>/x</href><href>/y</href><status>HTTP/1.1 200 OK</status></response>"),
            multi("<response><href>/x</href><status>HTTP/1.1 200 OK</status><propstat><prop/><status>HTTP/1.1 200 OK</status></propstat></response>"),
        ] { assert!(parse_multistatus(&xml).is_err()); }
        let xml = multi(
            r#"<response><href>/x</href><propstat><prop><getetag/></prop><status>HTTP/1.1 403 Forbidden</status></propstat></response>"#,
        );
        let parsed = parse_multistatus(&xml).unwrap();
        assert_eq!(
            parsed.responses[0]
                .property(DAV, "getetag")
                .unwrap_err()
                .status,
            Some(403)
        );
        let xml = multi(&item(
            "/x",
            "<getetag>\"a\"</getetag><getetag>\"b\"</getetag>",
        ));
        assert!(parse_multistatus(&xml).unwrap().responses[0]
            .property(DAV, "getetag")
            .is_err());
    }
    #[test]
    fn loopback_http_same_origin_and_encoded_paths() {
        for url in [
            "http://localhost:1234/",
            "http://127.0.0.2:1234/",
            "http://[::1]:1234/",
        ] {
            assert!(DavClient::new(url, "u", "p").is_ok());
        }
        for url in [
            "http://example.com/",
            "https://user@example.com/",
            "https://example.com/#secret",
            "file:///cal/",
        ] {
            assert!(DavClient::new(url, "u", "p").is_err());
        }
        let client = client("https://example.com/dav/principals/u/");
        assert_eq!(
            client
                .resolve_href(
                    "https://example.com/dav/principals/u/",
                    "../../calendars/%E4%B8%AD%E6%96%87/?id=1&v=2"
                )
                .unwrap(),
            "https://example.com/dav/calendars/%E4%B8%AD%E6%96%87/?id=1&v=2"
        );
        for href in [
            "//evil.example/x",
            "https://example.com:444/x",
            "http://example.com/x",
            "https://user@example.com/x",
            "/x#fragment",
            "\\evil\\x",
        ] {
            assert!(client.resolve_href("https://example.com/", href).is_err());
        }
        assert!(client
            .resource_in_collection("https://example.com/cal/", "https://example.com/calendar/x")
            .is_err());
        assert!(client
            .resource_in_collection(
                "https://example.com/cal/",
                "https://example.com/cal/%2F..%2Fx"
            )
            .is_err());
    }
    #[test]
    fn discover_principal_home_and_both_component_capabilities() {
        let mut redirect = step("PROPFIND / HTTP/1.1", 302, "");
        redirect.headers = "Location: /dav/\r\n".into();
        let (base, server) = mock(vec![
            redirect,
            step(
                "PROPFIND /dav/ HTTP/1.1",
                207,
                multi(&item(
                    "/dav/",
                    "<current-user-principal><href>principals/u/</href></current-user-principal>",
                )),
            ),
            step(
                "PROPFIND /dav/principals/u/ HTTP/1.1",
                207,
                multi(&item(
                    "/dav/principals/u/",
                    "<c:calendar-home-set><href>../../calendars/u/</href></c:calendar-home-set>",
                )),
            ),
            step(
                "PROPFIND /dav/calendars/u/ HTTP/1.1",
                207,
                multi(
                    &(item(
                        "/dav/calendars/u/",
                        "<resourcetype><collection/></resourcetype>",
                    ) + &item("tasks/", &collection_props("<c:comp name=\"VTODO\"/>"))
                        + &item(
                            "mixed/",
                            &collection_props("<c:comp name=\"VEVENT\"/><c:comp name=\"VTODO\"/>"),
                        )),
                ),
            ),
        ]);
        let collections = client(&base).discover().unwrap();
        server.join().unwrap();
        assert_eq!(collections.len(), 2);
        assert!(collections.iter().all(|c| c.privileges.writable()));
        assert_eq!(
            collections
                .iter()
                .find(|c| c.url.ends_with("tasks/"))
                .unwrap()
                .components,
            BTreeSet::from(["VTODO".into()])
        );
    }
    #[test]
    fn root_fallback_is_read_only_and_auth_failure_stops() {
        let (base, server) = mock(vec![
            step("PROPFIND / HTTP/1.1", 404, ""),
            step(
                "PROPFIND /.well-known/caldav HTTP/1.1",
                207,
                multi(&item(
                    "/cal/",
                    &collection_props("<c:comp name=\"VTODO\"/>"),
                )),
            ),
        ]);
        assert_eq!(client(&base).discover().unwrap().len(), 1);
        server.join().unwrap();
        let (base, server) = mock(vec![step("PROPFIND / HTTP/1.1", 401, "拒绝")]);
        assert_eq!(client(&base).discover().unwrap_err().status, Some(401));
        server.join().unwrap();
    }
    #[test]
    fn cross_origin_redirect_and_principal_never_forward_credentials() {
        let other = TcpListener::bind("127.0.0.1:0").unwrap();
        other.set_nonblocking(true).unwrap();
        let target = format!("http://{}/stolen/", other.local_addr().unwrap());
        let mut redirect = step("GET /cal/a HTTP/1.1", 307, "");
        redirect.headers = format!("Location: {target}\r\n");
        let (base, server) = mock(vec![redirect]);
        let error = client(&base).get("/cal/a").unwrap_err();
        server.join().unwrap();
        assert_eq!(error.status, Some(307));
        assert_eq!(
            error.response.unwrap().location.as_deref(),
            Some(target.as_str())
        );
        assert_eq!(
            other.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
        let (base, server) = mock(vec![step(
            "PROPFIND / HTTP/1.1",
            207,
            multi(&item(
                "/",
                &format!("<current-user-principal><href>{target}</href></current-user-principal>"),
            )),
        )]);
        assert!(client(&base).discover().is_err());
        server.join().unwrap();
        assert_eq!(
            other.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }
    #[test]
    fn explicit_validation_never_substitutes_a_todo_collection_for_event() {
        let (base, server) = mock(vec![step(
            "PROPFIND /cal/ HTTP/1.1",
            207,
            multi(&item(
                "/cal/",
                &collection_props("<c:comp name=\"VTODO\"/>"),
            )),
        )]);
        assert!(client(&base)
            .validate_collection("/cal/", Component::Event, true)
            .is_err());
        server.join().unwrap();
        let (base, server) = mock(vec![step(
            "PROPFIND /cal/ HTTP/1.1",
            207,
            multi(&item(
                "/cal/",
                &collection_props("<c:comp name=\"VTODO\"/>"),
            )),
        )]);
        assert!(client(&base)
            .validate_collection("/cal/", Component::Todo, true)
            .unwrap()
            .privileges
            .writable());
        server.join().unwrap();
        let props = "<resourcetype><collection/><c:calendar/></resourcetype><c:supported-calendar-component-set><c:comp name=\"VTODO\"/></c:supported-calendar-component-set>";
        let (base, server) = mock(vec![step(
            "PROPFIND /cal/ HTTP/1.1",
            207,
            multi(&item("/cal/", props)),
        )]);
        assert!(client(&base)
            .validate_collection("/cal/", Component::Todo, true)
            .is_err());
        server.join().unwrap();
    }
    #[test]
    fn full_listing_and_raw_multiget_include_undated_todo() {
        let refs = item(
            "no-date.ics?x=1&amp;y=2",
            "<resourcetype/><getetag>&quot;v1&quot;</getetag>",
        );
        let ics = "BEGIN:VCALENDAR\r\nBEGIN:VTODO\r\nUID:无日期\r\nX-UNKNOWN;P=值:保留\r\nEND:VTODO\r\nEND:VCALENDAR\r\n";
        let data = item(
            "/cal/no-date.ics?x=1&amp;y=2",
            &format!(
                "<getetag>&quot;v1&quot;</getetag><c:calendar-data>{}</c:calendar-data>",
                escape_xml(ics).replace('\r', "&#13;")
            ),
        );
        let mut report = step("REPORT /cal/ HTTP/1.1", 207, multi(&data));
        report.contains = vec![
            "/cal/no-date.ics?x=1&amp;y=2".into(),
            "<c:calendar-data/>".into(),
        ];
        let (base, server) = mock(vec![
            step("PROPFIND /cal/ HTTP/1.1", 207, listing(&refs)),
            report,
        ]);
        let client = client(&base);
        let list = client.list_resources("/cal/").unwrap();
        assert_eq!(list.resources.len(), 1);
        let resources = client
            .multiget(&list.collection_url, &list.resources)
            .unwrap();
        server.join().unwrap();
        assert_eq!(resources[0].ics, ics);
        assert!(resources[0].href.ends_with("no-date.ics?x=1&y=2"));
    }
    #[test]
    fn partial_listings_and_weak_etags_never_form_complete_success() {
        for extra in [
            item("a.ics", "<resourcetype/><getetag>W/&quot;v1&quot;</getetag>"),
            item("a.ics", "<resourcetype/>"),
            "<response><href>/cal/b.ics</href><status>HTTP/1.1 403 Forbidden</status></response>".into(),
            "<response><href>/cal/</href><status>HTTP/1.1 507 Insufficient Storage</status></response>".into(),
            item("/other/a.ics", "<resourcetype/><getetag>&quot;v1&quot;</getetag>"),
        ] {
            let (base, server) = mock(vec![step("PROPFIND /cal/ HTTP/1.1", 207, listing(&extra))]);
            assert!(client(&base).list_resources("/cal/").is_err());
            server.join().unwrap();
        }
        let (base, server) = mock(vec![step("PROPFIND /cal/ HTTP/1.1", 207, multi(""))]);
        assert!(client(&base).list_resources("/cal/").is_err());
        server.join().unwrap();
    }
    #[test]
    fn multiget_missing_failed_and_changed_resources_are_errors() {
        for body in [
            multi(""),
            multi("<response><href>/cal/a.ics</href><status>HTTP/1.1 404 Not Found</status></response>"),
            multi(&item("/cal/a.ics", "<getetag>&quot;v2&quot;</getetag><c:calendar-data>ICS</c:calendar-data>")),
            multi(&item("/cal/a.ics", "<getetag>&quot;v1&quot;</getetag>")),
            multi(&item("/cal/b.ics", "<getetag>&quot;v1&quot;</getetag><c:calendar-data>ICS</c:calendar-data>")),
        ] {
            let (base, server) = mock(vec![step("REPORT /cal/ HTTP/1.1", 207, body)]);
            assert!(client(&base).multiget("/cal/", &[reference(&base, "a.ics")]).is_err());
            server.join().unwrap();
        }
    }
    #[test]
    fn sync_delta_keeps_explicit_deletions_and_token() {
        let body = multi(&(item("/cal/a.ics", "<getetag>&quot;v1&quot;</getetag>") + "<response><href>/cal/deleted.ics</href><status>HTTP/1.1 404 Not Found</status></response><sync-token>token-new&amp;2</sync-token>"));
        let mut request = step("REPORT /cal/ HTTP/1.1", 207, body);
        request.contains = vec![
            "<d:sync-token>old&amp;1</d:sync-token>".into(),
            "<d:sync-level>1</d:sync-level>".into(),
        ];
        let (base, server) = mock(vec![request]);
        let result = client(&base)
            .sync_collection("/cal/", Some("old&1"))
            .unwrap();
        server.join().unwrap();
        assert!(!result.full);
        assert_eq!(result.resources.len(), 1);
        assert_eq!(result.deleted, vec![format!("{base}/cal/deleted.ics")]);
        assert_eq!(result.token.as_deref(), Some("token-new&2"));
    }
    #[test]
    fn invalid_token_and_unsupported_report_fall_back_to_complete_listing() {
        for (code, body) in [
            (403, "<error xmlns=\"DAV:\"><valid-sync-token/></error>"),
            (409, ""),
            (405, ""),
        ] {
            let (base, server) = mock(vec![
                step("REPORT /cal/ HTTP/1.1", code, body),
                step(
                    "PROPFIND /cal/ HTTP/1.1",
                    207,
                    listing(&item(
                        "a.ics",
                        "<resourcetype/><getetag>&quot;v1&quot;</getetag>",
                    )),
                ),
            ]);
            let result = client(&base)
                .sync_collection("/cal/", Some("invalid"))
                .unwrap();
            server.join().unwrap();
            assert!(result.full);
            assert_eq!(result.token, None);
            assert_eq!(result.resources.len(), 1);
        }
    }
    #[test]
    fn invalid_token_fallback_failure_does_not_produce_a_snapshot() {
        let (base, server) = mock(vec![
            step("REPORT /cal/ HTTP/1.1", 409, ""),
            step("PROPFIND /cal/ HTTP/1.1", 503, ""),
        ]);
        assert_eq!(
            client(&base)
                .sync_collection("/cal/", Some("invalid"))
                .unwrap_err()
                .status,
            Some(503)
        );
        server.join().unwrap();
    }
    #[test]
    fn sync_forbidden_truncated_and_missing_token_do_not_advance() {
        for (code, body) in [
            (403, "<error xmlns=\"DAV:\"><need-privileges/></error>".into()),
            (207, multi("<response><href>/cal/a.ics</href><status>HTTP/1.1 507 Insufficient Storage</status></response><sync-token>do-not-use</sync-token>")),
            (207, multi(&item("/cal/a.ics", "<getetag>&quot;v1&quot;</getetag>"))),
        ] {
            let (base, server) = mock(vec![step("REPORT /cal/ HTTP/1.1", code, body)]);
            assert!(client(&base).sync_collection("/cal/", Some("old")).is_err());
            server.join().unwrap();
        }
    }
    #[test]
    fn initial_sync_empty_token_produces_full_snapshot() {
        let mut request = step(
            "REPORT /cal/ HTTP/1.1",
            207,
            multi("<sync-token>initial</sync-token>"),
        );
        request.contains = vec!["<d:sync-token></d:sync-token>".into()];
        let (base, server) = mock(vec![request]);
        let result = client(&base).sync_collection("/cal/", None).unwrap();
        server.join().unwrap();
        assert!(result.full);
        assert!(result.resources.is_empty());
        assert_eq!(result.token.as_deref(), Some("initial"));
    }
    #[test]
    fn conditional_create_update_delete_preserve_status_etag_location_and_url() {
        let mut create = step("PUT /cal/a.ics HTTP/1.1", 201, "");
        create.headers = "ETag: \"v1\"\r\nLocation: /cal/canonical.ics\r\n".into();
        create.contains = vec![
            "If-None-Match: *".into(),
            "Content-Type: text/calendar; charset=utf-8".into(),
            "BEGIN:VCALENDAR".into(),
        ];
        let mut update = step("PUT /cal/a.ics HTTP/1.1", 204, "");
        update.contains = vec!["If-Match: \"v1\"".into()];
        let mut delete = step("DELETE /cal/a.ics HTTP/1.1", 204, "");
        delete.contains = vec!["If-Match: \"v2\"".into()];
        let (base, server) = mock(vec![create, update, delete]);
        let client = client(&base);
        let result = client
            .create("/cal/a.ics", "BEGIN:VCALENDAR\r\nEND:VCALENDAR\r\n")
            .unwrap();
        assert_eq!(result.status, 201);
        assert_eq!(result.etag.as_deref(), Some("\"v1\""));
        assert_eq!(result.location.as_deref(), Some("/cal/canonical.ics"));
        assert_eq!(result.final_url, format!("{base}/cal/a.ics"));
        assert_eq!(
            client.update("/cal/a.ics", "\"v1\"", "ICS").unwrap().status,
            204
        );
        assert_eq!(client.delete("/cal/a.ics", "\"v2\"").unwrap().status, 204);
        server.join().unwrap();
    }
    #[test]
    fn conflict_http_error_keeps_response_and_weak_etag_never_sends() {
        let mut request = step("PUT /cal/a.ics HTTP/1.1", 412, "版本冲突");
        request.headers = "ETag: \"server\"\r\n".into();
        let (base, server) = mock(vec![request]);
        let error = client(&base)
            .update("/cal/a.ics", "\"old\"", "ICS")
            .unwrap_err();
        server.join().unwrap();
        assert_eq!(error.status, Some(412));
        let response = error.response.unwrap();
        assert_eq!(response.body, "版本冲突");
        assert_eq!(response.etag.as_deref(), Some("\"server\""));
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let client = client(&format!("http://{}", listener.local_addr().unwrap()));
        for etag in ["", "W/\"v1\"", "*", "v1", "\"a\"\r\nHeader: x", "\"a b\""] {
            assert!(client.update("/cal/a", etag, "ICS").is_err());
            assert!(client.delete("/cal/a", etag).is_err());
        }
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }
    #[test]
    fn missing_component_property_uses_rfc4791_defaults() {
        let props = "<resourcetype><collection/><c:calendar/></resourcetype><current-user-privilege-set><privilege><all/></privilege></current-user-privilege-set>";
        for component in [Component::Event, Component::Todo] {
            let (base, server) = mock(vec![step(
                "PROPFIND /cal/ HTTP/1.1",
                207,
                multi(&item("/cal/", props)),
            )]);
            let collection = client(&base)
                .validate_collection("/cal/", component, true)
                .unwrap();
            server.join().unwrap();
            assert!(collection.components_known);
            assert!(collection.components.contains(component.name()));
        }
    }
    #[test]
    fn missing_acl_read_probe_never_grants_write_permissions() {
        for require_write in [false, true] {
            let props = "<resourcetype><collection/><c:calendar/></resourcetype>";
            let mut report = step("REPORT /cal/ HTTP/1.1", 207, multi(""));
            report.contains = vec![
                "<c:calendar-query".into(),
                "<c:comp-filter name=\"VCALENDAR\"/>".into(),
            ];
            let (base, server) = mock(vec![
                step("PROPFIND /cal/ HTTP/1.1", 207, multi(&item("/cal/", props))),
                report,
            ]);
            let result = client(&base).validate_collection("/cal/", Component::Todo, require_write);
            server.join().unwrap();
            if require_write {
                assert!(result.unwrap_err().message.contains("写入权限"));
            } else {
                let collection = result.unwrap();
                assert!(collection.privileges.read);
                assert!(!collection.privileges.known);
                assert!(!collection.privileges.writable());
            }
        }
    }
    #[test]
    fn read_probe_partial_or_forbidden_resources_do_not_validate() {
        let props = "<resourcetype><collection/><c:calendar/></resourcetype>";
        let body = multi("<response><href>/cal/secret.ics</href><propstat><prop><c:calendar-data/></prop><status>HTTP/1.1 403 Forbidden</status></propstat></response>");
        let (base, server) = mock(vec![
            step("PROPFIND /cal/ HTTP/1.1", 207, multi(&item("/cal/", props))),
            step("REPORT /cal/ HTTP/1.1", 207, body),
        ]);
        assert_eq!(
            client(&base)
                .validate_collection("/cal/", Component::Todo, false)
                .unwrap_err()
                .status,
            Some(403)
        );
        server.join().unwrap();
    }
    #[test]
    fn embedded_limits_507_and_non_text_properties_are_rejected() {
        for body in [
            "<sync-token>page-1</sync-token><error><number-of-matches-within-limits/></error>",
            "<sync-token>page-1</sync-token><error><c:number-of-matches-within-limits/></error>",
            "<response><href>/cal/</href><propstat><prop><getetag/></prop><status>HTTP/1.1 507 Insufficient Storage</status></propstat></response><sync-token>page-1</sync-token>",
            "<sync-token>prefix<fake/>suffix</sync-token>",
            "<response><href>/cal/<fake/>a</href><status>HTTP/1.1 200 OK</status></response>",
        ] { assert!(parse_multistatus(&multi(body)).is_err()); }
        let body = multi(&item(
            "/cal/a.ics",
            "<getetag>&quot;v1&quot;</getetag><c:calendar-data>BEGIN<fake/>END</c:calendar-data>",
        ));
        let (base, server) = mock(vec![step("REPORT /cal/ HTTP/1.1", 207, body)]);
        assert!(client(&base)
            .multiget("/cal/", &[reference(&base, "a.ics")])
            .is_err());
        server.join().unwrap();
    }
    #[test]
    fn multiget_second_batch_failure_discards_first_batch() {
        let first: String = (0..100)
            .map(|index| {
                item(
                    &format!("/cal/{index:03}.ics"),
                    "<getetag>&quot;v1&quot;</getetag><c:calendar-data>ICS</c:calendar-data>",
                )
            })
            .collect();
        let mut last = step("REPORT /cal/ HTTP/1.1", 503, "");
        last.contains = vec!["/cal/100.ics".into()];
        let (base, server) = mock(vec![
            step("REPORT /cal/ HTTP/1.1", 207, multi(&first)),
            last,
        ]);
        let requested: Vec<_> = (0..101)
            .map(|index| reference(&base, &format!("{index:03}.ics")))
            .collect();
        assert_eq!(
            client(&base)
                .multiget("/cal/", &requested)
                .unwrap_err()
                .status,
            Some(503)
        );
        server.join().unwrap();
    }
    #[test]
    fn interrupted_http_body_keeps_metadata_without_success() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let _ = read_request(&mut socket);
            socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 20\r\nETag: \"v1\"\r\nConnection: close\r\n\r\nshort").unwrap();
        });
        let error = client(&base).get("/cal/a.ics").unwrap_err();
        server.join().unwrap();
        assert_eq!(error.status, Some(200));
        let response = error.response.unwrap();
        assert_eq!(response.etag.as_deref(), Some("\"v1\""));
        assert_eq!(response.final_url, format!("{base}/cal/a.ics"));
    }
    #[test]
    fn write_redirect_request_count_stays_one_inside_and_outside_collection() {
        for method in ["create", "update", "delete"] {
            for code in [307, 308] {
                for target in ["/cal/other.ics", "/outside/other.ics"] {
                    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
                    listener.set_nonblocking(true).unwrap();
                    let base = format!("http://{}", listener.local_addr().unwrap());
                    let (done, finished) = std::sync::mpsc::channel();
                    let server = thread::spawn(move || {
                        let mut count = 0;
                        loop {
                            match listener.accept() {
                                Ok((mut socket, _)) => {
                                    // Windows 上非阻塞监听器接收的 socket 也可能保持非阻塞；
                                    // 先恢复阻塞，避免 read_request 偶发收到 WSAEWOULDBLOCK。
                                    socket.set_nonblocking(false).unwrap();
                                    socket
                                        .set_read_timeout(Some(Duration::from_secs(5)))
                                        .unwrap();
                                    let _ = read_request(&mut socket);
                                    count += 1;
                                    if count == 1 {
                                        write!(socket, "HTTP/1.1 {code} Redirect\r\nContent-Length: 0\r\nLocation: {target}\r\nConnection: close\r\n\r\n").unwrap();
                                    } else {
                                        socket.write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
                                    }
                                }
                                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                                    match finished.recv_timeout(Duration::from_millis(5)) {
                                        Ok(())
                                        | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                                            break
                                        }
                                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                                    }
                                }
                                Err(error) => panic!("假 DAV 接收失败：{error}"),
                            }
                        }
                        count
                    });
                    let client = client(&base);
                    let result = match method {
                        "create" => client.create("/cal/fixed.ics", "ICS"),
                        "update" => client.update("/cal/fixed.ics", "\"v1\"", "ICS"),
                        _ => client.delete("/cal/fixed.ics", "\"v1\""),
                    };
                    done.send(()).unwrap();
                    assert_eq!(server.join().unwrap(), 1, "重定向不能导致第二次写请求");
                    let error = result.unwrap_err();
                    assert_eq!(error.status, Some(code));
                    assert_eq!(error.response.unwrap().location.as_deref(), Some(target));
                }
            }
        }
    }
    #[test]
    fn same_origin_redirect_keeps_get_final_url_and_rejects_write_redirect() {
        let mut redirect = step("GET /cal/a HTTP/1.1", 302, "");
        redirect.headers = "Location: ../real/a\r\n".into();
        let mut get = step("GET /real/a HTTP/1.1", 200, "原始 ICS");
        get.headers = "ETag: \"v1\"\r\n".into();
        let mut put_redirect = step("PUT /cal/a HTTP/1.1", 307, "");
        put_redirect.headers = "Location: /real/a\r\n".into();
        let (base, server) = mock(vec![redirect, get, put_redirect]);
        let client = client(&base);
        let response = client.get("/cal/a").unwrap();
        assert_eq!(response.final_url, format!("{base}/real/a"));
        assert_eq!(response.body, "原始 ICS");
        assert_eq!(
            client
                .update("/cal/a", "\"v1\"", "ICS body")
                .unwrap_err()
                .status,
            Some(307)
        );
        server.join().unwrap();
    }
}
