# 用 CI 云端打包三端安装包

本机没有 Flutter/Xcode/Android SDK，无法本地构建。改用 **GitHub Actions 云端构建**：推上去后自动产出 Windows / macOS / Android 安装包，下载即可试用。不占本机磁盘。

## 一、准备一个 GitHub 仓库

工程要放进一个 Git 仓库才能触发 CI。两种方式：

**方式 A：新建独立仓库（推荐）**

```bash
cd /Users/emma/Documents/GitHub/DynamicIsland/daozai_flutter
git init
git add .
git commit -m "岛仔跨平台骨架 + CI 打包"
# 在 GitHub 网页新建一个空仓库（不要勾选 README），然后：
git remote add origin https://github.com/<你的用户名>/daozai-flutter.git
git branch -M main
git push -u origin main
```

> 注意：CI 工作流文件在父仓库的 `.github/workflows/build.yml`。若用独立仓库，需把它复制到新仓库根目录的 `.github/workflows/build.yml`，并把工作流里所有 `daozai_flutter/` 路径前缀去掉（因为新仓库根目录就是工程本身）。见下方「独立仓库的工作流调整」。

**方式 B：直接用当前 DynamicIsland 仓库**

当前仓库已有 `.github/workflows/build.yml`，且 `paths` 监听 `daozai_flutter/**`。只要把改动推到该仓库的 `main`/`master`，即自动触发。

```bash
cd /Users/emma/Documents/GitHub/DynamicIsland
git add daozai_flutter .github/workflows/build.yml
git commit -m "新增跨平台 Flutter 骨架与三端 CI 打包"
git push
```

## 二、触发与下载

1. 推送后打开仓库的 **Actions** 标签页，会看到「构建三端安装包」工作流在跑。
2. 三个任务（Windows / macOS / Android）并行，约 5–15 分钟。
3. 跑完后点进该次运行，页面底部 **Artifacts** 区下载：
   - `daozai-windows.zip` → 解压运行 `daozai_flutter.exe`
   - `daozai-macos.zip` → 解压得到 `.app`，拖进「应用程序」
   - `daozai-android.apk` → 传到安卓手机安装（需允许「未知来源」）

也可手动触发：Actions → 「构建三端安装包」→ **Run workflow**。

## 三、各平台试用注意

- **Windows**：未签名，SmartScreen 可能提示「未知发布者」，点「仍要运行」。岛体停在屏幕顶部居中，鼠标靠近展开。
- **macOS**：未签名/公证，首次打开右键 →「打开」绕过 Gatekeeper；若提示「已损坏」，执行 `xattr -cr /Applications/岛仔.app`。天气联网权限已由 CI 注入 entitlements。
- **Android**：APK 为 debug 签名（release 未配密钥时 Flutter 默认用 debug 签名），仅供试用，不能上架。主屏长按 → 小组件 → 找到「岛仔」添加。

## 四、独立仓库的工作流调整

若用方式 A（新仓库根目录即工程），把 `.github/workflows/build.yml` 做两处修改：

1. 删除 `defaults.run.working-directory: daozai_flutter`（或改为 `.`）。
2. 删除 `on.push.paths` 里的 `daozai_flutter/` 前缀，改为监听 `lib/**`、`pubspec.yaml` 等。
3. 上传 artifact 的 `path` 去掉 `daozai_flutter/` 前缀。

或者更省事：在新仓库里保留 `daozai_flutter/` 子目录结构（把整个工程放进子目录），工作流原样可用。

## 五、常见问题

- **CI 里 `flutter test` 失败**：纯逻辑测试不依赖设备，若失败多为复刻逻辑边界问题，按报错修 `lib/core/` 对应文件。
- **macOS 构建报签名错误**：CI 默认不签名即可产出 .app；若 runner 强制签名，加 `--no-codesign`（`flutter build macos --release --no-codesign`）。
- **Android 小组件不显示数据**：小组件读 `shared_preferences` 的待办计数（只算未完成）；Flutter 每次保存经 `home_widget` 推送刷新，时钟/日期由 TextClock 自走。若计数不动，先查 home_widget 版本与 Provider 全名是否匹配 applicationId。
- **想要正式签名/上架**：需配置 Android keystore、Apple 开发者证书、Windows 代码签名证书，另行接入。

## 六、本地构建（有工具链的机器）

若你另有装好 Flutter 的机器，也可本地打包，命令见工程 `README.md` 的「构建安装包」一节。