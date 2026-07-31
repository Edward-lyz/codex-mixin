# Codex Mixin 互动安装卡片技术方案调研

调研日期：2026-07-31（Asia/Taipei）

## 一页结论

产品已确认最低系统可以从当前 macOS 12 提高到 **macOS 13.1**。因此结论不是
简单排除 Rive，而是按制作方式二选一：

- **代码主导、每台机器的构图差异要大：** AppKit `AboutWindow` 中嵌入
  `NSHostingView`，卡片本体使用 SwiftUI `Canvas + TimelineView + Gesture`，
  按需要再加一层 `SpriteView` 做少量粒子。这仍是默认首选。
- **设计主导、要一个精修角色/图形响应 hover、点击和拖动：**
  `SwiftUI 外壳 + Rive Apple state machine` 是重点候选。先做一次独立技术
  spike，验证当前 shell `swiftc` 打包、签名和 PNG snapshot，再决定是否迁移
  菜单 executable 到 Swift Package 构建。

这条路线同时满足：

- 仓库代码当前最低系统是 macOS 12；提高到 13.1 后，`Canvas`、`TimelineView`
  和 Rive Apple 6.22 都满足平台要求。
  [Canvas][apple-canvas] [TimelineView][apple-timeline]
- 当前菜单 App 没有 Xcode project 或 Swift Package target，而是
  `build_app.sh` 把 Swift 文件直接交给 `xcrun swiftc`，目前只链接系统
  `Cocoa` framework。[当前构建脚本][mixin-build]
- 系统 SwiftUI、SpriteKit、CryptoKit、Security 和 WebKit framework
  不需要把第三方 runtime 打进 App；对包体、签名、CI 和离线分发最友好。
- 这张卡片的核心是“确定性生成的 2D 构图 + 指针/拖动/点击反馈”，不是复杂骨骼
  动画或通用 3D 场景，`Canvas` 的能力与问题匹配。
- 静态 PNG 可以完全在本机生成并通过系统分享面板发送；不需要把标识符、首用时间
  或图片上传到服务器。

**不要读取或散列硬件序列号、MAC 地址、`IOPlatformUUID` 等真实机器特征。**
Apple 明确表示，不得从设备信号派生数据以唯一识别设备或用户；即使把这些信号
哈希，仍然是设备指纹。[Apple 设备指纹说明][apple-fingerprinting]

这里的“机器码”应重新定义为 **Mixin 本地安装身份**：

1. 首次记录时用 Foundation `UUID()` 生成一个随机 `installationID`；
2. 同时保存 `firstRecordedAt = Date()` 与 `seedVersion = 1`；
3. 用 CryptoKit `SHA256(installationID || firstRecordedAt || seedVersion)`
   得到绘图 seed；
4. 原始值只留在本机，分享图只包含渲染结果，不带 seed、UUID、精确时间或可追踪 URL。

Foundation 的 `UUID` 是通用随机标识类型，CryptoKit 提供 SHA-256；
`UserDefaults` 适合保存 app-specific settings，Keychain 适合保存小块本地数据。
[UUID][apple-uuid] [SHA256][apple-sha256] [UserDefaults][apple-userdefaults]
[Keychain Services][apple-keychain]

> 当前代码没有现成的“首次使用 Mixin”记录，因此功能上线时无法可靠还原老用户的
> 历史首次使用日期。文件创建时间、bundle 时间或配置更新时间都可能因迁移、恢复、
> 覆盖安装而变化。第一版应记录“我们第一次在本机记录到 Mixin 的时间”，文案用
> “Mixin 认识你的第 N 天”或“从 2026 年 7 月开始记录”，不要伪称安装日。

## 五条路线的决策对比

| 路线 | macOS 13.1+ / 当前 `swiftc` 构建 | 交互与表现 | 依赖 / 包体 | 导出分享 | 许可 | 结论 |
| --- | --- | --- | --- | --- | --- | --- |
| **1. SwiftUI Canvas** | **完全适配**；`Canvas`、`TimelineView` 均从 macOS 12 可用 | 最适合 seed 驱动的色块、轨道、噪声、文字与 2D 形状；SwiftUI 手势处理点击、拖动和 hover；物理效果要自己写 | 系统 framework；只增加代码和素材 | macOS 13 可用 `ImageRenderer`；macOS 12 用离屏 `NSHostingView` / `NSView` bitmap cache；系统分享面板从 macOS 10.8 可用 | Apple SDK，无第三方 runtime 许可 | **首选** |
| **2. SpriteKit / SpriteView** | 适配；`SpriteView` 从 macOS 11 可用，只需链接系统 SpriteKit | 粒子、碰撞、惯性、弹簧和连续帧比 Canvas 省事；普通文字排版和自适应 UI 不如 SwiftUI | 系统 framework；粒子纹理/音效决定资源增量 | 可冻结 scene 后截图；仍建议外层 AppKit 负责分享 | Apple SDK | **作为 Canvas 的效果层，或物理交互明显变复杂时升级** |
| **3. SceneKit / SceneView** | 运行层面适配 macOS 12，但 Apple 当前文档已把 SceneKit API 标为 macOS 26 deprecated | 真 3D、灯光、相机和模型方便；2D 卡片属于能力过剩，鼠标命中、可访问性和导出都更复杂 | 系统 framework，但 3D 素材和 GPU 成本更高 | `SCNView` 可 snapshot；动态视频仍需逐帧编码 | Apple SDK | **不用于新功能**；若以后必须真 3D，另行评估 RealityKit 与最低系统版本 |
| **4. 原生 Rive / Lottie runtime** | 提高最低系统到 13.1 后，Rive Apple 6.22 和 Lottie 都适配。两者仍会给当前裸 `swiftc` 构建引入 SwiftPM/XCFramework 链接、资源嵌入与签名工作 | **Rive 的 state machine、event、data binding 很适合设计师制作可互动角色**；Lottie 擅长 AE 导出的播放、循环、scrub 和运行时属性替换，但不是生成式布局引擎 | Lottie 官方称预编译 XCFramework 通常约 8 MB；Rive 6.22 的多平台 XCFramework 下载包约 106 MB，最终 App 只会保留目标 slice，不能用下载大小代替安装增量 | Rive view 理论上可走 AppKit view snapshot，但 GPU/renderer 路径必须实测，不能假定 `ImageRenderer` 一定捕获；分享动画仍要录制 GIF/MP4 | Rive MIT；Lottie Apache-2.0；动画美术资产另行授权 | **Rive 是与 Canvas 并列的重点候选**；Lottie 仅适合作为一次性揭幕动画 |
| **5. WKWebView + Web 动画栈** | WebKit 本身支持；HTML/JS 可作为本地资源打包。不会引入原生 SwiftPM，但要增加 JS 构建、资源复制、消息桥和导航安全策略 | 原型最快，CSS/DOM 布局成熟；Rive Web 适合状态机，Lottie Web 适合播放，Three.js 适合真 3D，Motion 适合 DOM/SVG 手势与 spring | WKWebView 是系统组件，但 JS/WASM 要随 App 分发。当前 npm 包解包大小：Motion 约 0.68 MB、Rive Canvas 约 5.15 MB、Three.js 约 23.2 MB、lottie-web 约 25.4 MB；tree-shaking 和只取一个 build 后会小得多，不能把这些数当最终包体 | `WKWebView.takeSnapshot` 可导出静态图；动画分享仍需录制；HTML 分享会引入托管与隐私问题 | Rive Web / Three.js / Motion / lottie-web 均 MIT；美术资产另行授权 | **适合 2–3 天验证视觉概念，不是当前生产首选** |

表中平台依据来自 Apple 的
[Canvas][apple-canvas]、[SpriteView][apple-spriteview]、
[SceneView][apple-sceneview]、[SCNView][apple-scnview] 和
[WKWebView][apple-wkwebview] 文档。SceneKit 的当前平台元数据显示
`deprecatedAt: macOS 26.0`。[SCNView 文档数据][apple-scnview-json]

第三方 runtime 的平台和体积依据：

- Rive 官方 README 表明 Apple runtime 支持 AppKit 和 SwiftUI；当前
  `Package.swift` 是 binary target，并把最低 macOS 写为 13.1。
  [Rive Apple README][rive-apple-readme] [Rive Package.swift][rive-package]
- Rive 6.22 的 GitHub release asset 列出
  `RiveRuntime.xcframework.zip` 为 106,104,763 bytes；这是多平台依赖下载量，
  **不是** macOS App 最终增量。[Rive 6.22 release][rive-release]
- Lottie 官方 README 表明它原生支持 macOS、SwiftUI/AppKit，可播放、scrub
  和运行时修改动画；同一 README 称发布的 XCFramework 通常约 8 MB。
  当前 package 最低 macOS 是 10.15。
  [Lottie README][lottie-readme] [Lottie Package.swift][lottie-package]
- Web 包数字来自 npm registry 当前版本元数据，只用于比较依赖上限：
  [Motion 12.43.0][npm-motion]、[Rive Canvas 2.39.1][npm-rive-canvas]、
  [Three.js 0.185.1][npm-three]、[lottie-web 5.13.0][npm-lottie-web]。
  例如 lottie-web 官方仓库同时提供约 306 KB 的 `lottie.min.js` 与约 266 KB
  的 canvas-only minified build，说明 npm 解包大小明显高于实际只选一个
  player 的交付大小。[lottie-web builds][lottie-web-builds]

## WebView 内部四种技术怎么选

如果先做 Web prototype，不应把四个库一起装：

| Web 技术 | 最适合 | 不适合 | 本项目判断 |
| --- | --- | --- | --- |
| **Rive Web** | 一个可点击、会切换状态和响应输入的吉祥物/卡片；state machine、event、data binding 由设计稿定义 | 大量原生文字排版；不想维护 WASM；完全由代码生成的几何 | Web 方案中的首选 runtime，但生产版仍不如原生 Canvas 简洁 |
| **Lottie Web** | After Effects 做好的揭幕、循环背景、彩带；按进度 scrub | 多状态互动、物理反馈、seed 改变整体构图 | 只做 1–2 秒开场动画，不承担卡片主体 |
| **Three.js** | 真正需要相机、透视、光照、3D 模型或 shader 的“可旋转纪念物” | 普通 3D tilt、2D 粒子和渐变；这些用 Canvas/CSS 更轻 | 只有视觉稿明确是 3D 时才引入 |
| **Motion（原 Framer Motion）** | DOM/SVG 卡片的 spring、drag、layout transition、hover；官方已将 Framer Motion 更名为 Motion | 自己不是绘制或资产格式；Motion for React 还会引入 React | Web 原型的布局/手势层，可配自写 Canvas；不要和 Rive 的状态机重复 |

Rive 官方把 runtime 定位为能响应状态和输入的实时互动动画，并提供 state
machines、events、data binding 文档。[Rive state machines][rive-state-machines]
[Rive data binding][rive-data-binding]
Lottie 官方说明其 JSON 动画能播放、变速、倒放、scrub 和运行时改变属性。
[Lottie README][lottie-readme] Three.js 官方定位是 JavaScript 3D library。
[Three.js repository][three-repo] Motion 官方说明它提供 gestures、springs、
layout transitions 和 timelines，并明确 “Framer Motion is now Motion”。
[Motion repository][motion-repo]

## 推荐的生产实现

### 1. 一个可重置、本地专用的身份

建议数据结构：

```swift
struct CardIdentityV1: Codable {
    let installationID: UUID
    let firstRecordedAt: Date
    let seedVersion: UInt8
}
```

确定性 seed：

```text
SHA256(
  "codex-mixin-card" ||
  installationID.uuidString ||
  floor(firstRecordedAt / 1 day) ||
  seedVersion
)
```

取整到天可以避免同一天第一次打开的具体秒数影响作品，也减少无意义的精确时间
暴露。digest 再喂给一个固定算法的 PRNG（例如项目内实现的 SplitMix64），依次
决定 palette、轨道数量、粒子初始位置、符号和文案变体。必须固定取值顺序并保留
`seedVersion`，否则代码重构会让同一用户的卡片悄悄变化。

存储选择：

- **第一版推荐 `UserDefaults`**：这不是认证秘密，代码最少，与当前 shell
  构建完全兼容。
- 如果产品要求“删除普通偏好后仍保持卡片”，只把随机 UUID 放 Keychain，
  日期和版本仍放 defaults；不要承诺卸载重装后一定不变。
- 提供“重置我的卡片”按钮，清除本地身份后重新生成；重置前显示一次确认。
- 任何上传、遥测或跨设备同步都不属于第一版范围。若未来加入，必须重新审查隐私
  告知、同意与 App Store 数据披露。

Apple App Review Guidelines 要求数据最小化、用途限定，并禁止暗中建立用户画像。
[Guideline 5.1.1][apple-review-511] [Guideline 5.1.2][apple-review-512]
Apple 也明确说明：只在用户设备上关联且不以可识别形式离开设备的数据，不属于
其列举的 tracking 情形；这支持“纯本地生成”设计，但不取消一般隐私义务。
[Apple tracking 说明][apple-tracking-local]

### 2. `Canvas` 负责画，SwiftUI 负责状态

推荐分层：

```text
AboutWindowController (现有 AppKit 窗口)
└── NSHostingView<CardExperienceView>
    ├── CardRenderModel        确定性布局，只依赖 seed / size / frozenTime
    ├── TimelineView + Canvas  连续动画
    ├── Gesture / onHover      拖动、点击、hover 与键盘 focus
    └── CardShareController    冻结状态、导出 PNG、系统分享
```

第一版互动控制在四个动作：

1. 打开时 700–1000 ms 揭幕；
2. 鼠标进入时图层有轻微视差；
3. 按住拖动时星轨/粒子跟随；
4. 点击中心符号切换一个隐藏文案或配色。

在 macOS 13.1+ 可优先使用 SwiftUI 连续 hover/gesture 取得指针位置；如果仍需
更底层的鼠标事件，再加很薄的 `NSViewRepresentable` / `NSTrackingArea`
适配层。不要因此换成 WebView。

动画只在窗口可见时运行；窗口隐藏或开启 Reduce Motion 时停止连续粒子，
保留淡入和即时状态变化。SwiftUI 提供 `accessibilityReduceMotion` 环境值。
[Reduce Motion][apple-reduce-motion]

### 3. 导出一张“冻结”的卡片

交互画面与分享图必须共用同一个 `CardRenderModel`，导出时固定：

- 逻辑尺寸，例如 1200 × 750；
- scale，例如 2x；
- `frozenTime`，避免每次截图粒子位置不同；
- 选中的配色/隐藏文案状态；
- 不包含按钮、鼠标 hover 光圈、UUID 或 debug seed。

提高最低系统到 13.1 后，可以直接用 SwiftUI `ImageRenderer`；它从 macOS 13
可用。
[ImageRenderer][apple-image-renderer]

若最终决定不提高最低系统，兼容实现是把 `CardExperienceView` 放进离屏
`NSHostingView`，使用 `NSView.bitmapImageRepForCachingDisplay(in:)` 与
`cacheDisplay(in:to:)` 得到 bitmap，再编码 PNG。Rive 由自有 renderer 承载，
仍要单独验证这条 view snapshot 路径不会得到空白帧。
[NSView bitmap cache][apple-nsview-bitmap]
[NSView cacheDisplay][apple-nsview-cache-display]

分享使用 `NSSharingServicePicker`，它从 macOS 10.8 可用；SwiftUI `ShareLink`
要到 macOS 13，因此不能作为唯一实现。
[NSSharingServicePicker][apple-sharing-picker] [ShareLink][apple-sharelink]

如果走 WebView，Apple 提供 `WKWebView.takeSnapshot`，但这只解决静态图，
不会自动得到 GIF/视频。[WKWebView snapshot][apple-wkwebview-snapshot]
动画分享可在第二阶段逐帧输出到 ImageIO GIF/APNG 或 AVFoundation MP4；它比
PNG 多出帧同步、颜色、编码时间和文件大小控制，不应阻塞第一版。

## 当前仓库约束带来的具体取舍

当前 `build_app.sh`（即使把 deployment target 提高到 13.1，以下构建约束仍在）：

- 把每个 Swift 文件逐个列给 `xcrun swiftc`；
- 最低 target 是 macOS 12.0；
- 只显式链接 `Cocoa`；
- 构建后对 CLI、菜单 executable 和整个 `.app` 逐层 codesign；
- 多个测试脚本也各自直接调用 `swiftc`。

[构建脚本][mixin-build] [AboutWindow 测试脚本][mixin-about-test]

因此：

1. 先把 `MACOSX_DEPLOYMENT_TARGET`、Info.plist 与 CI 基线统一提高到 13.1。
2. Canvas 只需新增 Swift 文件并链接系统 `SwiftUI`；SpriteKit 也只多一个
   系统 framework，二者都可保持脚本构建。
3. Lottie/Rive 的 SwiftPM 不能被当前裸 `swiftc` 命令直接消费。Rive 有两种
   可行接法：把菜单 executable 建成 Swift Package target，让 SPM 解析 binary
   target；或手工下载/链接 XCFramework，并同步处理 `-F`、rpath、Frameworks
   copy、嵌套签名、两种 CPU 架构和测试命令。前者长期更干净，后者 spike 更快。
4. WKWebView 不需要原生包管理器，但要给构建脚本增加 HTML/JS/WASM resource
   copy；所有 URL 必须是 bundle 内资源，默认拒绝远程导航，并避免 JS bridge
   暴露任意 native 操作。
5. 因此第三方 runtime 只有在能显著降低设计制作成本时才值得承担集成成本，
   不能因为 demo 动起来更快就成为生产默认。

## 建议的交付顺序

### 第一阶段：并行做两个小样，约 2 天

1. 新增 `CardIdentityStore`，只在本机保存 `UUID + firstRecordedAt + version`；
2. Canvas 小样做 3 套 seed 驱动 palette、轻量粒子和拖动反馈；
3. Rive 小样做 1 个 state machine：hover、click、drag input 与动态文字；
4. Rive spike 同时验证 arm64/x86_64 构建、Frameworks 嵌入/签名和离屏 PNG；
5. 用同一张视觉稿比较制作时间、交互一致性、最终 `.app` 增量与 CPU/GPU。

### 第二阶段：可分享版本，约 1 天

1. 冻结动画状态并用 macOS 13.1 `ImageRenderer` 导出 Canvas PNG，同时完成
   Rive view snapshot 实测；
2. 用 `NSSharingServicePicker` 分享；
3. 分享图删除精确首次时间、seed、UUID 和调试文字；
4. 增加固定 seed snapshot / 像素尺寸测试；
5. 分别在 Light/Dark、Reduce Motion、macOS 13.1 与当前 macOS 验证。

### 后续只有满足条件才升级

- 粒子、碰撞、惯性成为主角：加 SpriteKit 效果层；
- 设计师需要独立制作交互角色，Rive spike 的构建/截图通过：采用
  `SwiftUI shell + Rive state machine`；
- 视觉方案明确需要 AE 资产：只在揭幕层加 Lottie；
- 视觉方案明确需要相机/灯光/3D 模型：单独验证 Three.js 或 RealityKit；
- 需要分享动画：在静态 PNG 数据模型稳定后再做 MP4，不先做 GIF。

## 许可与隐私检查表

- Rive Apple runtime 是 MIT。[Rive license][rive-license]
- Lottie Apple runtime 是 Apache-2.0，官方 README 还声明 runtime 不收集数据并
  提供 privacy manifest。[Lottie license][lottie-license]
  [Lottie privacy][lottie-privacy]
- Three.js、Motion、Rive Web 与 lottie-web runtime 都是 MIT。
  [Three.js license][three-license] [Motion license][motion-license]
  [Rive Web license][rive-web-license] [lottie-web license][lottie-web-license]
- Runtime 许可 **不等于** Rive Community、LottieFiles、字体、插画、音效或 3D
  模型的美术许可；生产素材必须由项目自制或逐项保存授权证明。
- 不使用硬件序列号、网卡地址、浏览器配置、网络信息或多信号组合来构造 identity。
- 不把本地随机 UUID 当作分析 ID 上传；如果以后真的上传，它就从本地实现细节变成
  可识别的 user/device data，需要重新评估用途、披露、同意与删除能力。
- 分享 PNG 不写原始 identifier、精确 first-use timestamp、隐形 metadata、
  可回传服务器的 URL/QR code。
- 给用户一个可理解的说明：“图案在本机根据随机安装标识和首次记录日期生成，
  数据不会离开设备”，并提供重置入口。

## 最终推荐

**若卡片主体是“每个 seed 生成不同构图”，最稳妥的组合是：**

```text
AppKit AboutWindow
+ NSHostingView
+ SwiftUI Canvas / TimelineView / Gesture
+ Foundation UUID / UserDefaults
+ CryptoKit SHA256
+ SwiftUI ImageRenderer
+ NSSharingServicePicker
```

**若卡片主体是“同一个精修角色/图形，有很多交互状态”，组合改为：**

```text
SwiftUI / AppKit 外壳
+ Rive Apple runtime 6.22
+ .riv state machine / events / data binding
+ 本地 identity 只映射为受控的数值、布尔和文字 input
+ AppKit snapshot + NSSharingServicePicker
```

在 macOS 13.1 的新前提下，Rive 的平台阻碍已经消失；真正的门槛变成当前 shell
`swiftc` 构建集成和 renderer snapshot 可靠性。这两项应先用一天内的 spike
给出实测数字，不能凭 demo 决定。Canvas 仍更适合大范围生成式变化、零依赖和
确定性导出；Rive 更适合设计协作、复杂状态机和精修动效。SceneKit 不进入新功能，
WKWebView 只承担快速视觉验证或明确的 3D Web 方案。

[apple-canvas]: https://developer.apple.com/documentation/swiftui/canvas
[apple-timeline]: https://developer.apple.com/documentation/swiftui/timelineview
[apple-spriteview]: https://developer.apple.com/documentation/spritekit/spriteview
[apple-sceneview]: https://developer.apple.com/documentation/scenekit/sceneview
[apple-scnview]: https://developer.apple.com/documentation/scenekit/scnview
[apple-scnview-json]: https://developer.apple.com/tutorials/data/documentation/scenekit/scnview.json
[apple-wkwebview]: https://developer.apple.com/documentation/webkit/wkwebview
[apple-wkwebview-snapshot]: https://developer.apple.com/documentation/webkit/wkwebview/takesnapshot(with:completionhandler:)
[apple-image-renderer]: https://developer.apple.com/documentation/swiftui/imagerenderer
[apple-sharelink]: https://developer.apple.com/documentation/swiftui/sharelink
[apple-sharing-picker]: https://developer.apple.com/documentation/appkit/nssharingservicepicker
[apple-nsview-bitmap]: https://developer.apple.com/documentation/appkit/nsview/bitmapimagerepforcachingdisplay(in:)
[apple-nsview-cache-display]: https://developer.apple.com/documentation/appkit/nsview/cachedisplay(in:to:)
[apple-reduce-motion]: https://developer.apple.com/documentation/swiftui/environmentvalues/accessibilityreducemotion
[apple-uuid]: https://developer.apple.com/documentation/foundation/uuid
[apple-sha256]: https://developer.apple.com/documentation/cryptokit/sha256
[apple-userdefaults]: https://developer.apple.com/documentation/foundation/userdefaults
[apple-keychain]: https://developer.apple.com/documentation/security/keychain-services
[apple-fingerprinting]: https://developer.apple.com/app-store/user-privacy-and-data-use/#permission-to-track
[apple-review-511]: https://developer.apple.com/app-store/review/guidelines/#data-collection-and-storage
[apple-review-512]: https://developer.apple.com/app-store/review/guidelines/#data-use-and-sharing
[apple-tracking-local]: https://developer.apple.com/app-store/user-privacy-and-data-use/#permission-to-track
[mixin-build]: https://github.com/Edward-lyz/codex-mixin/blob/a6990ae39d48f611977fee33efdf68a9c7f6efb2/macos/build_app.sh#L1-L87
[mixin-about-test]: https://github.com/Edward-lyz/codex-mixin/blob/a6990ae39d48f611977fee33efdf68a9c7f6efb2/scripts/test_about_window.sh
[rive-apple-readme]: https://github.com/rive-app/rive-ios/blob/5767e6fa687f930f8508d4fdad21758b6871182d/README.md
[rive-package]: https://github.com/rive-app/rive-ios/blob/5767e6fa687f930f8508d4fdad21758b6871182d/Package.swift
[rive-release]: https://github.com/rive-app/rive-ios/releases/tag/6.22.0
[rive-state-machines]: https://rive.app/docs/runtimes/state-machines
[rive-data-binding]: https://rive.app/docs/runtimes/data-binding
[rive-license]: https://github.com/rive-app/rive-ios/blob/5767e6fa687f930f8508d4fdad21758b6871182d/LICENSE
[rive-web-license]: https://github.com/rive-app/rive-wasm/blob/6d99a6ec28d71846c94467f7ccbc2933d703c4df/LICENSE
[lottie-readme]: https://github.com/airbnb/lottie-ios/blob/f8ea198bdf6a588739a9256a8196e36106c988b9/README.md
[lottie-package]: https://github.com/airbnb/lottie-ios/blob/f8ea198bdf6a588739a9256a8196e36106c988b9/Package.swift
[lottie-license]: https://github.com/airbnb/lottie-ios/blob/f8ea198bdf6a588739a9256a8196e36106c988b9/LICENSE
[lottie-privacy]: https://github.com/airbnb/lottie-ios/blob/f8ea198bdf6a588739a9256a8196e36106c988b9/Sources/PrivacyInfo.xcprivacy
[lottie-web-builds]: https://github.com/airbnb/lottie-web/tree/bede03d25d232826e0c9dca1733d542d8a7754fb/build/player
[lottie-web-license]: https://github.com/airbnb/lottie-web/blob/bede03d25d232826e0c9dca1733d542d8a7754fb/LICENSE.md
[three-repo]: https://github.com/mrdoob/three.js/tree/2a005fdbad6b8503a8a70edfdd279b79c5e04b49
[three-license]: https://github.com/mrdoob/three.js/blob/2a005fdbad6b8503a8a70edfdd279b79c5e04b49/LICENSE
[motion-repo]: https://github.com/motiondivision/motion/tree/a4e4b3ab73dd64fbab2574fae27d28c0418f25cb
[motion-license]: https://github.com/motiondivision/motion/blob/a4e4b3ab73dd64fbab2574fae27d28c0418f25cb/LICENSE.md
[npm-motion]: https://registry.npmjs.org/motion/12.43.0
[npm-rive-canvas]: https://registry.npmjs.org/@rive-app/canvas/2.39.1
[npm-three]: https://registry.npmjs.org/three/0.185.1
[npm-lottie-web]: https://registry.npmjs.org/lottie-web/5.13.0
