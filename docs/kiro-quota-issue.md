# 功能建议：支持读取 Kiro 额度

> 建议类型：功能新增（AI 额度模块新增 `Provider::Kiro`）

## 背景

[Kiro](https://kiro.dev/) 是 AWS 推出的 AI IDE（付费订阅制，Free / Pro / Power 三档）。它的 CLI 本身就有 `kiro-cli /usage` 命令可以查看额度，说明客户端走的是 AWS 官方 API。如果岛仔能像支持 Codex / Claude / Kimi 那样支持 Kiro，就可以在桌面上直接看到 Kiro 的月度额度消耗。

经过在本机（Windows + Kiro 桌面版，KIRO PRO 订阅）上抓包和实测，完整链路如下，可直接采用。

## 一、凭据来源（本机登录文件，只读）

Kiro 桌面版登录后会在本机生成 AWS SSO 缓存：

```
Windows:  %USERPROFILE%\.aws\sso\cache\kiro-auth-token.json
macOS/Linux:  ~/.aws/sso/cache/kiro-auth-token.json
```

关键字段（全部实测确认）：

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `accessToken` | string | Bearer 令牌，用于调 API |
| `expiresAt` | RFC3339 string | 令牌过期时间，用于提示用户重新登录 |
| `profileArn` | string? | 社交登录（Google/GitHub 等）有；**企业 IAM Identity Center（IdC）账号没有此字段** |
| `clientIdHash` | string? | 企业 IdC 账号有，且不随令牌续期变化 |
| `region` | string? | 区域，缺省按 `us-east-1` 处理 |
| `authMethod` | string | `"social"` / `"IdC"` |
| `refreshToken` | string | 建议不使用，见「设计原则」 |

这与项目现有模式（Codex 的 `auth.json`、Claude 的 `.credentials.json`）完全同构：**只读识别本机登录文件，不回写、不自动续期**，令牌过期时提示用户打开 Kiro 重新登录。

## 二、额度查询 API

```
POST https://codewhisperer.us-east-1.amazonaws.com/
```

| 项 | 值 |
| --- | --- |
| `Content-Type` | `application/x-amz-json-1.0` |
| `X-Amz-Target` | `AmazonCodeWhispererService.GetUsageLimits` |
| `Authorization` | `Bearer <accessToken>` |

**端点按 `profileArn` / `region` 的区域选择**：

- `us-east-1` → `codewhisperer.us-east-1.amazonaws.com`
- `eu-central-1` → `q.eu-central-1.amazonaws.com`
- 其他区域 → `codewhisperer.<region>.amazonaws.com`（实测 us-east-1 与文档一致；其余区域按 CLIProxyAPI 的做法泛化）

**请求体**（重要细节）：

```json
{ "profileArn": "<来自登录文件>" }
```

- 社交登录账号：传 `profileArn`。
- **企业 IdC 账号：登录文件里没有 `profileArn`，实测传空对象 `{}` 服务端就能按令牌识别身份并正确返回额度**。不要把它当错误处理（这一点容易踩坑，我是真机联调才发现的）。

### 响应示例（实测，Free/Pro/Power 的字段结构相同）

```json
{
  "usageBreakdownList": [
    {
      "resourceType": "CREDIT",
      "usageLimitWithPrecision": 1000.0,
      "currentUsageWithPrecision": 95.91,
      "currentOveragesWithPrecision": 0.0,
      "nextDateReset": 1796083200
    }
  ],
  "overageConfiguration": {
    "overageStatus": "ENABLED",
    "capForWorkloadUsage": 200.0
  },
  "subscriptionInfo": {
    "subscriptionTitle": "KIRO PRO"
  }
}
```

字段解读（均为实测确认）：

| 字段 | 说明 |
| --- | --- |
| `usageBreakdownList[].resourceType == "CREDIT"` | 套餐积分行，从列表里筛这一行 |
| `usageLimitWithPrecision` | 周期总额度（Pro 为 1000 点） |
| `currentUsageWithPrecision` | 已用量。**注意：开了超额后它是「套餐 + 超额」的总用量**，要用它减去 `currentOveragesWithPrecision` 才是套餐内的真实消耗 |
| `currentOveragesWithPrecision` | 已用超额点数 |
| `overageConfiguration.overageStatus` | `"ENABLED"` / `"DISABLED"`，只有 ENABLED 时超额额度才有意义 |
| `overageCapWithPrecision` | 超额上限；DISABLED 时服务端可能不返回或返回 0 |
| `nextDateReset` | **Unix 秒时间戳**（不是毫秒、不是字符串），下个重置日 |
| `subscriptionInfo.subscriptionTitle` | 套餐名（"KIRO PRO" 等），可直接用作显示名 |
| `usageBreakdownList[].bonuses` | 赠送/加成积分列表，非空时无法与套餐用量精确拆分 |

建议展示两个周期环：

1. **月度额度**：`(limit − planUsed) / limit`，配具体点数 `904.1 / 1000`
2. **超额额度**（overageStatus 为 ENABLED 且 cap > 0 时）：`(cap − overage) / cap`

## 三、几个容易踩的坑（实测总结）

1. **`currentUsageWithPrecision` 含超额**：超用后若不减去 overage，套餐环会显示成 0% 而实际套餐未耗尽；`bonuses` 非空时套餐与赠送无法拆分，`planUsed` 取 `min(used, limit)` 即可。
2. **IdC 账号无 `profileArn`**：见上文，空 body 即可，不要报错。
3. **账号唯一标识不能用 token 哈希**：Kiro 会自动续期令牌，token 每次都变。用 token 哈希做账号槽位会导致每次续期都被误判为「已切换账号」。正确做法是优先 `profileArn`，其次企业 IdC 的 `clientIdHash`，都缺失（少数社交账号）才退回 token 哈希。
4. **令牌过期提示**：比对 `expiresAt`，过期后提示「请打开 Kiro 重新登录」，岛仔不做刷新（与项目「不自动续期」原则一致）。
5. `nextDateReset` 是**秒**级时间戳，建议做范围校验（如 1e9..4.1e9）防脏数据。

## 四、参考实现

以下开源项目均已验证这条链路，可作为交叉参考：

- **router-for-me/CLIProxyAPI**（Go）— `internal/auth/kiro/` 完整实现了登录文件解析、区域路由与 GetUsageLimits 调用
- kiro-cli 的 `/usage` 命令（官方 CLI，同一 API）
- 本人已在 Windows + KIRO PRO 账号上完成端到端实测：解析出的剩余额度与 Kiro 客户端显示一致（95.91 / 1000，剩余 90.4%，12-01 重置）

## 五、附加建议（可选）

- 展示时用**具体点数**（`904 / 1000 点`）而非百分比，Kiro 的计费单位就是积分，用户对点数更有感知
- 图标可用 Kiro 官方的紫色圆角方 + 白色幽灵形象（`#8B47F9` 底色）

---
*如果需要，我可以提供一套基于本项目现有 `quota.rs` 架构的 Rust 参考实现（含 `fetch` / `parse` 与单元测试，已在本机跑通）。*
