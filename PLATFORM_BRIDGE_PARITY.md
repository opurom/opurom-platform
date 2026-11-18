# 平台桥接体系 Flutter 对标实现记录（2026-11-18）

责任模块：模块五「原生能力桥接体系」。
Flutter 版本：**3.50-candidate**，engine `371f77202a73e8dff57a8b3bff75bca55c090aee`（本地 `flutter-golden`）。
插件快照：`flutter-plugins` at `557d3284ac9dda32a1106bb75be8a23bdccd2f96`。
实现仓库：`opurom-platform`（独立 git 仓库，HEAD 起点 `f71bc18`）。

架构红线：IR 唯一真相源、编译期全量降级、运行时零解释、确定性渲染。
本模块只新增**数据契约 + 编译期表 + 纯函数算法**，不引入运行时插件对象树、不引入解释器、不触碰 IR 与渲染管线。

## 一、标准化平台通道（`src/platform_channel.rs`）

| Flutter 源码（3.50-candidate） | Opurom 对应 | 验证 |
| --- | --- | --- |
| `packages/flutter/lib/src/services/message_codecs.dart:263-562` — StandardMessageCodec 标签表 0-14、扩展尺寸、主机字节序、double/typed-list 对齐、尾随字节拒绝 | `Value` 枚举 + `encode_message`/`decode_message` | `tests/platform_channel.rs` 逐字节断言：标量边界、253/254/65536 尺寸边界、double 对齐、UTF-8、typed list、5 类畸形输入 |
| `message_codecs.dart:580-657` — MethodCall = name+args，Reply = 0 success / 1 error(code,message,details[,stacktrace]) | `MethodCall`/`Reply`/`encode_envelope`/`decode_envelope` | 成功/错误/null reply 信封往返，含可选 stacktrace 消费 |
| `platform_channel.dart:350-366,622-633` — null reply ⇒ MissingPluginException；OptionalMethodChannel ⇒ null | `Reply::MissingPlugin` + `ChannelKind::Method{optional}` | 测试断言 null reply 不被伪装成成功 |
| `platform_channel.dart:586-614` — handler 异常 ⇒ code='error' envelope | `error_codes` 表 + typed `CodecError` | dispatch 测试覆盖成功与错误 |
| `binary_messenger.dart:18-80` — 按名注册/替换 handler | `ChannelTable`：编译期构建、按名 dispatch、缺名返回 null reply | 表替换语义与缺名语义测试 |
| `engine/src/flutter/lib/ui/channel_buffers.dart:75-81,131-144,257-260,321,343-354` — 每通道 FIFO、默认容量 1、溢出丢最旧 | `ChannelBuffer<T>`：capacity=1 默认、FIFO、drop-oldest、`dropped()` 计数 | 溢出顺序与计数测试 |
| `system_channels.dart:181,385,401-404,423,430` | `system_channels` 常量（含 `flutter/system`） | 字符串常量断言 |

**ADAPT 声明**：Flutter 的 `BinaryMessenger` 是可变 handler 注册表 + async Future，Opurom 用编译期 `ChannelTable` + 同步 in-process trait 承载同一数据契约。依据 `opurom-spec/PLATFORM_CHANNEL_PARITY.md:§3 G1/G3/G5/G7` 的架构适配裁决；不引入运行时对象树。

## 二、高频原生能力统一 API（`src/native_capabilities.rs`）

| 能力 | Flutter 官方源码依据 | Opurom 契约 |
| --- | --- | --- |
| 键值持久化 | `shared_preferences/lib/shared_preferences.dart:17-158`：`flutter.` key 前缀、typed getter/setter、remove/clear/reload | `preferences_methods`（16 个方法）+ `prefixed_key`/`unprefixed_key` |
| 文件系统路径 | `path_provider/lib/path_provider.dart:54-136` + `method_channel_path_provider.dart:15-52`：`plugins.flutter.io/path_provider` 通道，7 个目录方法 | `path_methods` + 平台声明 |
| 打开链接 | `url_launcher_platform_interface/lib/src/url_launcher_platform.dart:33-96` + `method_channel_url_launcher.dart:7-55`：`canLaunch`/`launch`/`launchUrl`/`closeWebView` 与参数表 | `url_methods` + 参数键常量 |
| 图片选择 | `image_picker/lib/image_picker.dart:212-337`：`pickImage`/`pickMultiImage`/`pickVideo`、`ImageSource.camera/gallery` | `image_methods` |
| 设备信息（投影） | 无 `device_info_plus` 本地源码 | 仅列出本 crate 可计算的 4 项（display geometry/dpi scale/backend/viewport），**不宣称完整 parity** |
| 网络 / 权限 / 分享 | 本地 `flutter-plugins` 快照无对应插件源码 | `Capability::unavailable_reason()` 返回 GAP 原因，`CapabilityTable` 对缺源能力返回 typed `unimplemented`，**不伪造实现** |
| 剪贴板 / 震动 | `flutter-golden/packages/flutter/lib/src/services/clipboard.dart:46-72`、`haptic_feedback.dart:29-33` | 声明方法名；宿主后端尚未接入 |

`CapabilityTable` 复用 `platform_channel::Value`/`Reply`，保证跨通道/跨能力同样的数据与错误语义。测试：7 项（能力表、key 前缀、方法表、dispatch、缺能力、未知方法、qual/plain 等价）。

## 三、生命周期与系统事件（`src/system_events.rs`；生命周期既有实现不变）

- 生命周期五态机已在此前提交中对齐：`src/app_lifecycle.rs`，Flutter `services/binding.dart:313-381` + `scheduler/binding.dart:388-430`，`frames_enabled` 已导出；本轮不改动已闭环协议。
- Flutter 唯一的框架级通用系统事件：`services/system_channels.dart:395-405` 的 `BasicMessageChannel<Object?>('flutter/system', JSONMessageCodec())`，payload `{"type":"memoryPressure"}`，`services/binding.dart:170-186` switch 处理。
- Opurom：`SystemEvent::MemoryPressure` 的 wire 值逐字段对齐；键盘/旋转/深色/省电/刷新率在本项目中**没有 Flutter framework 公开事件**，明确标为 `host.*` 宿主适配事件，不冒充 Flutter API。
- `SystemEventDispatcher` 用编译期订阅槽替代运行时 `StreamController`，保留同 kind 去重与 reset（对应 0→1/1→0 重订阅）。

## 四、插件自动注册扩展（`src/plugin_registry.rs`）

| Flutter 源码（3.50-candidate） | Opurom 对应 |
| --- | --- |
| `packages/flutter_tools/lib/src/flutter_plugins.dart:225-292` `sortByDependencies`：依赖先于消费者、同层字母序、环按最小入度再字母序打破 | `resolve_registration()` 逐行移植该算法（BTreeMap/BTreeSet 保证确定性） |
| `flutter_plugins.dart:528-551` `GeneratedPluginRegistrant`：每插件一次 add、逐插件 try/catch 隔离 | `RegistrationPlan`/`RegistrationStep`：宿主按序调用自身注册函数，失败隔离留给宿主；本模块产出冻结计划 |
| `flutter_plugins.dart:307-350` `.flutter-plugins-dependencies` 只列插件依赖 | `PluginManifest.dependencies` 在进入图前已过滤 `UnknownDependency`（测试覆盖） |
| `flutter_plugins.dart:380-420` 各端 manifest 权限合并 | `merge_permissions()`：按目标平台取并集，纯函数、确定性 |
| Opurom 既有 `opurom-plugins` manifest 的 `implements` 接口版本一致性 | `interface` 字段 + `InterfaceMismatch` 校验 |

测试：6 项，覆盖依赖排序、确定性环打破、能力重复拒绝、非插件依赖过滤、平台过滤、权限并集。

## 五、第二轮：框架通道与真实后端（P0 自动接续）

| Flutter 源码（3.50-candidate） | Opurom 对应 | 验证 |
| --- | --- | --- |
| `services/message_codecs.dart:44-62` `StringCodec` 原始 UTF-8 | `encode_lifecycle_message` / `parse_lifecycle_message` | 五态字符串逐字对齐 `AppLifecycleState.*`，未知值拒绝 |
| `services/message_codecs.dart:88-114,120-200` `JSONMessageCodec` / `JSONMethodCodec` 的 call map、success/error envelope | `src/framework_channels.rs` 全套 JSON 编解码 | 6 项单测：形状字节、尾随 JSON、错误四元组、与标准编解码并存 |
| `services/binding.dart:448-457` 生命周期字符串 + `scheduler/binding.dart:416-430` 帧门控 | `lifecycle_message` / `lifecycle_transitions` / `lifecycle_frames_enabled` | 五态往返、中间态序列、帧开关逐态断言 |
| `services/binding.dart:174-182` 唯一 `memoryPressure` 事件 | `SYSTEM_EVENT_MEMORY_PRESSURE` | 字符串常量断言 |
| `clipboard.dart:32-68`、`haptic_feedback.dart:19-152`、`editable_text.dart:3037`、`system_navigator.dart:65`、`binding.dart:525,588`、`system_chrome.dart:651-762` | `platform_methods` 冻结方法名与参数键 | 震动八种类型归一为单一 `HapticFeedback.vibrate`，常量逐字断言 |
| `shared_preferences/lib/shared_preferences.dart:17,59-158` + `method_channel_shared_preferences.dart:11-38` | `PreferencesBackend`：`flutter.` 前缀、五类类型化值、`getKeys`/`remove`/`clear`/`commit`/`reload` | 4 项单测：五类型往返、类型不匹配拒绝、CapabilityTable 接入、键前缀 |
| `path_provider_platform_interface/lib/src/method_channel_path_provider.dart:15-78` + Linux/macOS/Windows 官方实现 | `PathResolver`：七方法 + 平台可用性 + 注入根路径 | 单测覆盖 Linux 可用集、macOS Library、typed unimplemented、CapabilityTable 接入 |

追加机制规则：`serde_json = "=1.0.154"` 与核心仓同版本锁定，仅用于 JSON 框架通道与类型化偏好值；测试断言中 `&[]` 改为 `.is_empty()` 避免 serde_json 的 `PartialEq` 歧义。

### 第三轮：url_launcher / image_picker 宿主后端

| Flutter 源码（3.50-candidate，flutter-plugins @ 557d3284） | Opurom 对应 | 验证 |
| --- | --- | --- |
| `url_launcher_platform_interface/lib/src/url_launcher_platform.dart:72-85` — `launchUrl` 的 `useWebView`/`universalLinksOnly` 布尔派生 | `UrlLauncherBackend::derive_launch_options` / `LaunchOptions` | 三组派生断言：https 默认 in-app、mailto 走系统、externalNonBrowser 仅外链 |
| `url_launcher_platform_interface/lib/method_channel_url_launcher.dart:12-52` — `canLaunch`/`launch`/`closeWebView` 与参数表 | `UrlLauncherBackend::call` + 注入 opener | 成功/失败返回、缺 url 报 invalid_argument、CapabilityTable 接入 |
| `url_launcher_platform_interface/lib/src/types.dart:11-70` — `PreferredLaunchMode` 索引与 `InAppWebViewConfiguration` 默认值 | `url_methods::MODE_*` 与 `DEFAULT_ENABLE_*` 常量 | 常量与默认值逐字对齐 |
| `image_picker_platform_interface/lib/src/method_channel/method_channel_image_picker.dart:12,75-152` — 五个通道方法与参数表 | `image_methods` 扩展：`RETRIEVE`、source/camera 索引、六个参数键、image/video 类型常量 | source 索引、imageQuality 0-100、RETRIEVE 免参数、CapabilityTable 接入 |
| `image_picker_platform_interface/lib/src/types/types.dart:15-18` — `kTypeImage`/`kTypeVideo` | `TYPE_IMAGE`/`TYPE_VIDEO` | 字符串逐字断言 |

### 第四轮：应用生命周期宿主观察者（P0 收尾）

| Flutter 源码（3.50-candidate） | Opurom 对应 | 验证 |
| --- | --- | --- |
| `services/binding.dart:448-457` 五态字符串 + `:313-381` 中间态合成 | `MacWindow::observe_lifecycle` 复用 `app_lifecycle::transitions` | 观察者直接产出生成态序列，与 wayland/win32/x11 宿主一致 |
| `services/binding.dart:296-301` 初始状态读取 | `MacWindow` 构造即 `makeKeyAndOrderFront`，游标初始为 `Resumed` | 键窗口状态与初始 `resumed` 对齐 |
| `scheduler/binding.dart:416-430` 帧门控 | 已有 `frames_enabled` 导出，观察者不改协议 | 门控回归测试保持通过 |

macOS 通知映射（`objc2-app-kit` 0.3.2 静态通知名）：
- `resumed` ← `NSApplicationDidBecomeActiveNotification` / `NSWindowDidBecomeKeyNotification`
- `inactive` ← `NSApplicationDidResignActiveNotification` / `NSWindowDidResignKeyNotification`
- `hidden` ← `NSApplicationDidHideNotification` / `NSWindowDidMiniaturizeNotification`
- `resumed` ← `NSApplicationDidUnhideNotification` / `NSWindowDidDeminiaturizeNotification`
- `detached` ← `NSWindowWillCloseNotification`

实现要点：观察者经 `NSNotificationCenter.addObserverForName:object:queue:usingBlock:` 注册在主队列，与宿主事件循环同线程；令牌由 `LifecycleObserver` 持有，drop 即撤销注册；事件先入窗口自身 `VecDeque<Lifecycle>`，由 `drain_lifecycle()` FIFO 取出，宿主再送入统一输入流。架构红线不破：无运行时解释器、无对象树、IR 未触碰。

### 第五轮：系统事件宿主生产者（P1 第一项）

| Flutter 源码（3.50-candidate） | Opurom 对应 | 验证 |
| --- | --- | --- |
| `services/system_chrome.dart:43-70` — `DeviceOrientation` 四值索引顺序 | `DeviceOrientation` 枚举 + `from_index`/`from_size` | 四索引逐字断言，越界返回 `None` 不猜测 |
| `widgets/media_query.dart:916-935` — `platformBrightnessOf` 从平台视图配置派生暗色模式 | `brightness(dark)` 纯函数生产者 | 亮暗两态 wire 往返断言 |
| `services/system_channels.dart:599` — 键盘可见性通道 | `keyboard_visibility(visible, height)` | 带高度/无高度两态断言 |
| `services/binding.dart:174-182` — 唯一框架事件 `memoryPressure` | `memory_pressure()` | payload 逐字段对齐 |

新增 `src/event_producers.rs`（237 行）：全部为纯函数（无时钟、无随机、无隐藏状态），从原始 OS 事实到封闭事件值的确定性映射；折叠姿态投影复用 `display_geometry` 的尺寸规则；DPI 换算走共享 `DPI_UNIT` 定点网格。6 项单测覆盖索引顺序、尺寸投影、姿态投影、wire 往返、payload 精确性。

### 第六轮：媒体能力补齐（P1 第二项）

| Flutter 源码（3.50-candidate，flutter-plugins @ 557d3284） | Opurom 对应 | 验证 |
| --- | --- | --- |
| `camera_platform_interface/lib/src/method_channel/method_channel_camera.dart:17,94-354` — `plugins.flutter.io/camera` 与 11 个方法、参数键 | `camera_methods` 模块 + `CameraBackend` 注入式处理器 | 2 项单测：方法全集、`cameraId` 校验、`create` 特殊路径、CapabilityTable 接入 |
| `video_player_platform_interface/lib/video_player_platform_interface.dart:42-102` + Android `VideoPlayerPlugin.java:114-210` — 11 个播放器方法与 native `position` 命名 | `video_methods` 模块 + `VideoPlayerBackend` | 2 项单测覆盖上述场景 |

`Capability` 由 12 类扩展至 14 类（新增 `Camera` / `VideoPlayer`），方法与参数均取自 pinned 源码；宿主无处理器时返回 typed `unimplemented`。架构红线不破：无运行时对象树、无解释器、IR 未触碰。

### 第七轮：插件注册执行端（P1 第三项）

| Flutter 源码（3.50-candidate） | Opurom 对应 | 验证 |
| --- | --- | --- |
| `flutter_tools/lib/src/flutter_plugins.dart:528-551` — 每插件一次 `plugins.add`，各自 try/catch 隔离失败 | `plugin_executor::execute_plan` 按冻结计划逐项调用注入函数，失败记录 typed reason 后继续 | 3 项单测：依赖顺序 + 失败隔离、缺函数 typed 失败、失败插件不合并权限 |
| `flutter_plugins.dart:225-292` — 依赖排序 | 复用 `resolve_registration` 冻结计划 | 断言顺序 `camera → path → share` |
| `flutter_plugins.dart:380-420` — 平台权限并集 | `RegistrationReport.permissions()` 只合并已注册插件 | `perm.b` 被失败插件排除 |

执行端不引入运行时插件对象树：宿主只提供 `RegistrationFn` 函数指针表，执行结果是不可变 `RegistrationReport`，IR 与渲染管线未触碰。

### 第八轮：原生视图嵌入桥接（P2 第一项）

| Flutter 源码（3.50-candidate） | Opurom 对应 | 验证 |
| --- | --- | --- |
| `services/system_channels.dart:423,430` — `flutter/platform_views` / `_2` 通道 | `platform_view_bridge` 复用既有通道名语义 | 9 个方法名常量 |
| `services/platform_views.dart:286,325,936,989,1037,1406,1442,1472,1481,1517,1617,1627` — create/touch/resize/dispose 等操作 | 九方法 facade 接入 `PlatformViewRegistry` | 2 项单测：完整生命周期、畸形参数拒绝 |

实现严守 `PlatformViewRegistry` 的单向生命周期（created→attached→resized→input-forwarded→disposed）：create 自动推进到 attached，首次 touch 前补 resize，输入后 resize 为幂等 size ack，dispose 可查询但不再接收输入。无运行时视图对象树，未触碰 IR。

### 第九轮：后台任务消息契约（P2 第二项）

| Flutter 源码（3.50-candidate） | Opurom 对应 | 验证 |
| --- | --- | --- |
| `services/binding.dart:115-122` — `rootIsolateToken` 用于后台 isolate 协调 | `RootToken` 非零 u64 不透明令牌 | 零值拒绝、初始化幂等 |
| `services/_background_isolate_binary_messenger_io.dart:15-63` — `ensureInitialized` 幂等 | `BackgroundMessenger::ensure_initialized` | 第二 token 不重置序号与 pending |
| `:76-89` — 单调递增 message identifier 与 receive-port 回复 | `send` 分配序号 + `reply`/`take_reply` 按序号关联 | 序号单调、未知 id 拒绝、回复精确取出 |
| `:92-96` — 后台 isolate 不支持 `setMessageHandler` | `set_message_handler_unsupported` typed 错误 | 断言 `unimplemented` |

GAP 说明：Flutter framework 未定义通用后台任务调度器，仅有后台 isolate 的平台通道机制；Opurom 只平移该机制，宿主原生调度器（Android WorkManager/iOS BGTask 等）按各端原生机制注入，不自创 API。

### 第十轮：深度轮次 — 边界与异常输入补全

- `framework_channels`：新增 2 项边界测试，覆盖超大 JSON 整数按 Dart 语义回落为 `double`、非有限指数 JSON 被拒、嵌套对象保真、非对象 method call 与错误信封字段类型拒绝、非 UTF-8 与空生命周期载荷拒绝。
- `native_backends`：新增 1 项畸形宿主参数测试，覆盖偏好设置缺 key、类型不匹配、launcher 非映射参数、picker source 索引类型错误。
- 全部沿用 pinned 源码语义：`message_codecs.dart:88-200`（JSON 编解码）、`binding.dart:448-457`（生命周期）、`method_channel_url_launcher.dart:12-52`、`method_channel_image_picker.dart:75-152`。

### 第十一轮：深度轮次 — 跨端契约审计

新增 `tests/cross_contract.rs`（5 项测试）：统一错误码集合唯一且稳定；14 类能力名唯一且 `from_str` 可逆；camera/video 各通道方法表内部无冲突并保留 `create`/`dispose` 官方命名；系统事件 map payload 的 `type` 字段与 `code()` 严格一致，框架事件与 `host.*` 命名空间隔离；插件 manifest 能力声明唯一。

审计结论：无硬编码错误码字面量（全部走 `error_codes` 常量）；无跨通道错误码漂移；纯契约层无跨端 API 分歧。

### 第十二轮：深度轮次 — 多平台原生视图适配补全

`src/platform_view_bridge.rs` 从 Android 最小形状扩展到 Darwin / Android / HCPP 三端共用一套冻结 wire 契约，所有键和值均来自 pinned `flutter-golden@ab2912035e`：

| Flutter 源码（3.50-candidate） | Opurom 对应 | 验证 |
| --- | --- | --- |
| `services/platform_views.dart:268-280,1672-1705` — `gestureBlockingPolicy` 四值字符串 | `view_methods::gesture_blocking_policy::{EAGER,WAIT_UNTIL_TOUCHES_ENDED,FALLBACK_TO_PLUGIN_DEFAULT,DO_NOT_BLOCK_GESTURE}`，`create` 拒绝枚举外字符串 | Darwin create 合法/非法策略两态断言，非法调用不注册 handle |
| `services/platform_views.dart:789-792,827-832,989` — Android 方向只有 LTR=0 / RTL=1 | `view_methods::direction` 冻结两值；`setDirection` 从错误的 `0..3` 修正为只接受 0/1 | RTL 成功、3 被 `invalid_argument` 拒绝 |
| `services/platform_views.dart:1359-1390` — Android create 的 `direction`/`hybrid`/`hybridFallback`/`left`/`top`/`params` | 类型化键 + 布尔/字节数组形状校验；`Uint8List` 走 `Value::Uint8List` | hybrid 字符串形式被拒绝，合法 Android create 成功 |
| `services/platform_views.dart:1405-1410,1567-1571` — `rejectGesture` 可选整数 `gestureId` | `ARG_GESTURE_ID` 整数形状校验，缺失时保持可选语义 | 整数通过、字符串拒绝；`acceptGesture` 无 id 仍通过 |
| `services/platform_views.dart:1472-1479` — offset 发送 double `top`/`left` | `ARG_TOP`/`ARG_LEFT` 要求 `F64`，修正此前错用 `x`/`y` 整数 | double 通过、整数拒绝 |
| `services/platform_views.dart:1524-1532` — `_2` 通道 `isSurfaceControlEnabled` 是 opt-in 能力查询 | `PlatformViewBridge::with_surface_control(bool)` 注入宿主能力，默认 `false` | 注入 true 返回 `Success(Bool(true))` |
| `services/platform_views.dart:78-85` — 宿主回调 `viewFocused(id)` 仅在已注册 focus handler 时分发 | `VIEW_FOCUSED` 加入方法表，校验 live view；不推进注册表单向生命周期（非输入事件） | live view 通过且生命周期保持 attached，disposed view 报错 |

新增 4 项模块单测；合计 116 库单测 + 24 个集成测试组全绿。无渲染、IR、解释器改动，默认态金标零漂移。

### 第十三轮：深度轮次 — 框架通道到事件桥接

新增 `src/channel_event_bridge.rs`（3 项单测）：`flutter/system` JSON payload 严格解码到 `SystemEvent::MemoryPressure`，未知 type 或非 map 载荷拒绝；`flutter/lifecycle` StringCodec 解码五态并复用 Flutter `binding.dart:313-381` 的中间态生成；解码后的系统消息接入 `SystemEventDispatcher`，重复事件按宿主信号去重。源码依据：message_codecs.dart:44-62,88-114；system_channels.dart:385-405；binding.dart:174-182,303-311,313-381,448-457。无渲染/IR/解释器改动，默认态零漂移。

### 第十四轮：深度轮次 — 宿主能力注入审计

新增 `src/host_injection_audit.rs`（3 项单测）：为 14 类能力生成稳定审计行（方法数、是否要求宿主 handler、GAP 原因）；空 `CapabilityTable` 对所有有方法的 capability 返回 typed `unimplemented`，绝不伪造 `Success(Null)`；qualifed method 形状保持 codec-neutral。源码依据：flutter_tools/lib/src/flutter_plugins.dart:528-551（宿主注册）、services/platform_channel.dart:350-366（MissingPluginException 语义）。GAP 类（network/permissions/share）方法表为空，按构造保持不可调用而非伪装成功。

### 第十五轮：深度轮次 — 宿主信号到分发器端到端链

`SystemEventDispatcher` 增加 `emit_host_event` 便捷入口；新增 1 项端到端测试，把 `event_producers` 的六类宿主信号（memoryPressure/keyboard/orientation/brightness/power/refreshRate）生成后逐项接入分发器，验证订阅 slot 稳定、同值同 kind 去重、同 kind 新值仍送达。源码依据：services/binding.dart:174-182（唯一框架事件）、system_channels.dart:599（键盘）、system_chrome.dart:43-70（朝向索引）、binary_messenger.dart:66-80（单 handler 语义）。不触碰渲染/IR/解释器，默认态零漂移。

### 第十六轮：深度轮次 — 生产路径无 panic 审计

对新增桥接模块逐文件审计生产路径中的 `unwrap/expect/panic/todo/unimplemented!`：唯一命中为 `platform_view_bridge` 转发输入前的 bounds 重建 `expect`，已改为复用注册时已验证的 `ViewBounds` 值，不再有生产路径 panic。测试中的 `unwrap` 保留，用于断言失败时快速暴露回归。源码依据：`services/platform_views.dart:286-1517` 的 create/resize/touch 参数形状；`platform_view.rs` 的严格生命周期。全量门禁仍绿，默认态零漂移。

### 第十七轮：深度轮次 — 确定性 fuzz 边界矩阵

新增 `tests/bridge_fuzz_matrix.rs`（3 项）：标准编解码在 0..=8 长度 × 11 个边界字节的有限矩阵上无 panic；JSON 编解码在 19 个畸形/边界文本上无 panic；14 类能力的 missing-handler reply 始终是 typed `unimplemented` 且永不伪造 `Success(Null)`。该矩阵是确定性、可重放的单元测试，不引入随机源，符合引擎确定性红线。源码依据：message_codecs.dart:88-200,310-562；platform_channel.dart:350-366。默认态零漂移。

### 第十八轮：深度轮次 — 标准与 JSON codec 跨端一致性

`bridge_fuzz_matrix` 新增 `standard_and_json_paths_agree_on_logical_values`：对 Null/bool/I32/I64/F64/String/List/Map 等共享 JSON 表示的值，验证 StandardMessageCodec 与 JSONMessageCodec 编解码往返逻辑一致；按 Dart 单一 `int` 类型（`message_codecs.dart:97,391-397`）将可落入 I32 的 I64 归一化后比较，避免把合法表示差异误判为跨端漂移。默认态零漂移，无 IR/渲染改动。

### 第十九轮：深度轮次 — 插件生态规范审计

`src/plugin_registry.rs` 新增纯函数 manifest 审计（4 项单测），只校验声明、不编辑文件、不加载插件代码、不注册运行时 handler：

| Flutter 源码（3.50-candidate） | Opurom 对应 | 验证 |
| --- | --- | --- |
| `flutter_tools/lib/src/commands/create_base.dart:731,735-804,811-813` — `[a-z_][a-z0-9_]*` 且不能是 Dart 关键字 | `is_valid_package_name` + `DART_KEYWORDS` 静态表 | 合法下划线名通过；`class`、大写、连字符、空串被拒 |
| `flutter_tools/lib/src/flutter_plugins.dart:142-145` — 插件 `environment.flutter` 版本约束 | `minimum_flutter_version` 只接受明确的 `>= X.Y.Z`，其他形状返回 `None` | 2.11.0/2.10.0 解析正确，`any` 不猜测 |
| `flutter_tools/lib/src/plugins.dart:1900-1910` + `flutter_plugins.dart:1904-1908` — 桌面自实现需要 Flutter ≥ 2.11.0 | `desktop_self_impl_allowed` + `audit_manifests` 的 linux/macos/windows 门槛 | 无约束/2.10 被拒，2.11 通过；Android 不受该门槛限制 |
| `flutter_tools/lib/src/plugins.dart:203` — `implements` 指向 app-facing 接口；`flutter_plugins.dart:1985-1995` — 候选接口必须可解析 | `with_interface` / `implements_package` + 未知接口拒绝 | 未声明的 `camera` 接口被拒 |
| 同一接口的版本一致性（Opurom manifest 的 `interface` 对） | app-facing 与实现端版本必须逐字一致 | `1.0.0` 对 `2.0.0` 被拒 |
| `flutter_plugins.dart:1950-1995` — 多候选实现需要明确选择，不能静默歧义 | 同接口同平台的第二个实现被 `AmbiguousImplementation` 拒绝 | linux 上 `camera`/`camera_alt` 冲突被拒 |

`RegistryError` 增加 `Manifest(ManifestError)` 变体；`ManifestError` 的类型化原因可显示且可作为 `std::error::Error`。无渲染、IR、解释器改动，默认态零漂移。

### 第二十轮：深度轮次 — 原生性能优化与溢出路径

| Flutter 源码（3.50-candidate） | Opurom 优化 | 验证 |
| --- | --- | --- |
| `engine/src/flutter/lib/ui/channel_buffers.dart:330,345,382-385` — `Map<String, _Channel>` 按通道名 `putIfAbsent`/查找 | `ChannelTable` 从 `Vec` 线性扫描改为 `BTreeMap<&'static str, (ChannelSpec, Handler)>` 名称索引，`spec`/`dispatch` 为 O(log n)，`names()` 给出确定性排序 | 新增 name-indexed 测试：重复注册 last-wins、`names()` 稳定为 `["alpha","zeta"]`、missing spec 为 None |
| `packages/flutter/lib/src/services/binding.dart:654-675` — `setMessageHandler` 按名替换单一 handler | `ChannelTable::with` 对同名直接覆盖，保持 Flutter 单 handler 语义 | 重复 `zeta` 注册后 kind 更新为最后一次值且 `len` 不变 |
| `engine/src/flutter/lib/ui/channel_buffers.dart:144,162-170` — `_dropOverflowMessages(capacity-1)` 丢弃所有超限旧消息 | `ChannelBuffer::push`/`push_stamped` 以 `while` 排空所有超出容量的旧项，`dropped()` 累计每一项；不再只丢一条 | 新增 burst 测试：容量 3 连推 5 条，仅保留最新 3 条且 `dropped=2` |

优化只改变内部索引与溢出循环，不变更 wire 字节、错误码或生命周期；默认态零漂移，不触碰 IR/渲染/解释器。

### 第二十一轮：深度轮次 — 热路径分配收口

- `Value::map_get_str` 统一 8 处 method-call 参数字符串键查找（`native_backends.rs` 与 `platform_view_bridge.rs`）；所有查找走同一显式键构造点，避免散落的 `Value::String(name.to_owned())`，并保留 `message_codecs.dart:310-324,580-657` 的精确键身份语义。曾试验 `Borrow<str>` + 哨兵方案，实测会与真实字符串键或数值键产生冲突，已撤回；测试覆盖空字符串键与 I32 键并存时不互相遮蔽。
- `framework_channels::unimplemented` 以预分配 `Vec` + `String::from_utf8` 构造诊断消息，替代 `format!` 的格式化 machinery；保留 `platform_channel.dart:350-366` 的 null-reply / `unimplemented` 错误语义，新增测试逐字段断言消息并做 envelope 往返。
- 新增 2 项测试（字符串键身份、unimplemented envelope 往返）。无渲染、IR、解释器改动，默认态零漂移。

### 第二十二轮：深度轮次 — 截断尺寸与递归溢出边界

`tests/bridge_fuzz_matrix.rs` 新增 `codec_rejects_truncated_sizes_and_nested_recursion_overflow`（1 项）：

| Flutter 源码（3.50-candidate） | 边界输入 | 断言 |
| --- | --- | --- |
| `message_codecs.dart:275-281,545-562` — 0xFE/0xFF 扩展尺寸头部必须完整 | 0xFE 后缺长度字节；0xFF 后仅 2 字节长度 | `decode_message` 返回 typed `Corrupt`，不 panic、不猜测尺寸 |
| `message_codecs.dart:470-476` — map 的 key/value 成对写入 | `[13, 1, 7, 1, 'a']`（有 key 无 value） | 拒绝半条 map 项 |
| `message_codecs.dart:452-457` — list 长度受 buffer 约束 | `[12, 255, 0xff, 0xff, 0xff, 0xff]` 声明 4G 长度 | 拒绝超长 list，不做容量分配 |
| `readValue` 递归深度保护（`message_codecs.dart:379-385`） | 129/140 层嵌套 List | 深度 128 通过；129 与 140 层被 typed 拒绝 |

本轮只补确定性、可重放的畸形输入测试，不改实现；默认态零漂移，不触碰 IR/渲染/解释器。

### 第二十四轮：深度轮次 — 插件生态规范：平台注入与权限合并

`src/plugin_registry.rs` 新增平台注入描述符与审计函数，补全 manifest 声明与 Flutter 生成清单之间的规范断层，配合 `host_injection_audit` 的第 2 项单测走通注入缺口：

| Flutter 源码（3.50-candidate） | Opurom 对应 | 验证 |
| --- | --- | --- |
| `flutter_tools/lib/src/platform_plugins.dart:82-96,213-229,343,457` — `NativeOrDartPlugin.hasMethodChannel/hasFfi`、`ffiPlugin`、`DarwinPlugin.sharedDarwinSource` | `PlatformInjection { method_channel, ffi, shared_darwin_source }` + `native_build()` | 三个布尔字段的注入态可审计 |
| `flutter_tools/lib/src/flutter_plugins.dart:484-497` — 按 `platformKeys` 生成插件列表；`native_build` 为 methodChannel/FFI | `audit_platform_injection` 对每一族（manifest, injection, platform）返回 typed 缺口 | 平台限制拒绝、无原生实现拒绝、sharedDarwinSource 仅在 iOS/macOS |
| 平台验证加入 `darwin` 开关约束，避免 `sharedDarwinSource` 泄露到 Windows/Linux | `InjectionGap::SharedDarwinSourceOutsideDarwin` | 纯函数审计，始终拒绝 |
| `hosts`/`permissions` 按目标平台取并集（`flutter_plugins.dart:380-420`，第十七轮已落地） | 与平台过滤的 `merge_permissions` 语义一致 | 无权限文件，确定性并集 |

新增带类型化原因的测试，无渲染、IR、解释器改动，默认态零漂移。

### 第二十三轮：深度轮次 — 多平台宿主注入点审计

`src/host_injection_audit.rs` 新增 `framework_channel_method`、`InjectionSlot` 与 `injection_slots()`，把「宿主注入点」从纯 capability 行细化为逐方法槽位；并新增 2 项单测：

| Flutter 源码（3.50-candidate） | Opurom 对应 | 验证 |
| --- | --- | --- |
| `services/clipboard.dart:36,49-51,65-67` — `Clipboard.setData/getData/hasStrings` 是 `flutter/platform` 框架方法 | `Capability::Clipboard` 三方法逐一映射到 `platform_methods::CLIPBOARD_*` | 三方法名逐字断言 |
| `services/haptic_feedback.dart:24-147` — 八种 `HapticFeedbackType.*` 全部经 `HapticFeedback.vibrate` 单方法发送 | `haptic_type_name` 把裸方法名映射到类型常量；`haptic_method` 接受完整类型串（含 `HapticFeedbackType.vibrate`）；`InjectionSlot.framework_method` 统一折叠 | 五种裸方法全部折叠到 `HapticFeedback.vibrate` |
| `services/platform_channel.dart:350-366` — 无 handler 通道返回 null reply | 每个 slot 的 `missing_reply` 必为 typed `unimplemented`，GAP 类（network/permissions/share/device_info）message 含 `snapshot` | 逐 slot 断言 code/details/message |
| `flutter_tools/lib/src/flutter_plugins.dart:528-551` — 各端 registrant 按平台逐一注入 | `injection_slots()` 覆盖 14 类能力的全部方法，顺序确定 | slot 数 = Σ各能力方法数 |

本轮修正 `haptic_method` 对完整 `HapticFeedbackType.vibrate` 类型串的接受（此前只接受八种 impact/notification 类型），未新增能力、未伪造实现。无渲染、IR、解释器改动，默认态零漂移。
### 第二十五轮：深度轮次 — 跨模块事件码字典审计

新增 `tests/event_code_dictionary_audit.rs`（8 项）：把散落在 `platform_channel` / `system_events` / `framework_channels` / `media_capabilities` 的桥接词表统一成五张可审计字典（平台事件、生命周期、通道错误码、通道命名空间、标准 codec 类型标签），逐项与 Flutter 3.50 官方常量比对，输出四类结论：`duplicate`（同一码跨命名空间或同表重复）、`gap`（审计范围内官方常量未被覆盖）、`illegal`（命名空间或形状违规）、`unused`（已声明但实现从未引用）。审计只读：解析编译期常量与 crate 源码文本，不新增运行时注册表。

| Flutter 源码（3.50-candidate） | Opurom 审计项 | 验证 |
| --- | --- | --- |
| `services/message_codecs.dart:310-324` — 封闭的类型标签表 0..=14 | `codec_tag` 字典逐项比对 `Value::tag()` 与实际编码出的标签字节 | 14 个 `Value` 变体一一对应官方标签；写出的标签集合与官方可写集合完全相等 |
| `services/message_codecs.dart:379-385` — 标签 5（larger integers）声明但 `writeValue` 从不写出 | `unused`：`5 largeInt` | Opurom 同样不写标签 5；解码标签 5 返回 `Unsupported("large integer")`，未知标签 15 返回 `Corrupt("unknown tag")` |
| `services/binding.dart:174-182` — `flutter/system` 唯一框架事件 `memoryPressure` | `system_event` 字典：官方集合仅此一项 | 6 个事件码互不重复；`memoryPressure` 覆盖官方集合，其余 5 个带 `host.` 前缀且 `is_flutter_framework_event()` 为 false |
| `services/binding.dart:448-457` — 五条 `AppLifecycleState.*` 生命周期消息 | `lifecycle` 字典 | 五态字符串与官方逐字相等，无缺口 |
| `services/message_codec.dart:108-122,205-215`、`services/platform_channel.dart:359-364`、`engine/.../systemchannels/PlatformChannel.java:56,196` — Flutter 只用自由 `code` 字符串与异常类型表达失败 | `error_code` 字典 + Flutter 失败类型映射表 | 四个错误码互不重复、均为小写 snake_case；每个码都映射到一个官方失败类型；`error` 是唯一 Flutter 真实码串 |
| `services/system_channels.dart:77,98,182,202,210,294,322,342,346,371,386,402,414,423,430,439,450,478,501,515,566,577,599,616` — 官方 `flutter/` 通道命名空间 | `channel_name` 字典 + 命名空间覆盖报告 | 桥接词表覆盖 `flutter/platform`、`flutter/lifecycle`、`flutter/system`、`flutter/platform_views`、`flutter/platform_views_2`；其余 19 条框架通道逐条列出 |
| `flutter-plugins` 快照 `plugins.flutter.io/*` 命名空间 | 插件通道命名空间形状校验 | 全部以 `flutter/` 或 `plugins.flutter.io/` 开头，无越界命名 |

审计结论（`namespace|kind|detail` 机器可读）：`duplicate` 0 项、`illegal` 0 项、审计范围内 `gap` 0 项；`unused` 7 项，逐条原因如下：

- `error_code|unused|missing_plugin` — Flutter 把「无 handler」建模为 `MissingPluginException` 异常类型而非错误码串（`message_codec.dart:205-215`），因此该拼写只作类型级标签；缺 handler 场景由结构化的 `Reply::MissingPlugin` 承担，实现中不以错误信封发出。
- `codec_tag|unused|5 largeInt` — Flutter 自身也从不写出标签 5。
- `channel_name|unused|flutter/lifecycle`、`flutter/platform`、`flutter/platform_views`、`flutter/platform_views_2`、`plugins.flutter.io/camera` — 通道名常量已声明但尚未被生产通道表引用，属接线缺口而非死代码。

审计器本身带自检：向字典注入重复值、非法值、缺失官方值与未被引用的码，四类结论都会被检出；跨命名空间重名单独归入 `cross` 命名空间。无渲染、IR、解释器改动，默认态零漂移。

### 第二十六轮：深度轮次 — 官方通道 codec 契约落地

新增 `src/channel_contract.rs`（7 项模块内单测）与 `tests/channel_codec_contract.rs`（10 项集成测试）：把 `system_channels.dart` 里「通道名 + 通道类型 + codec」的三元组冻结成编译期契约表，并让所有通道载荷按契约路由编码/解码，通道名正确但 codec 用错的情况从结构上不再可能。

| Flutter 源码（3.50-candidate） | Opurom 对应 | 验证 |
| --- | --- | --- |
| `services/system_channels.dart:385-388` — `flutter/lifecycle` 是 `BasicMessageChannel<String?>` + `StringCodec` | `ChannelContract { kind: BasicMessage, codec: String }` | 只接受裸 UTF-8；标准 codec 字节被读成不同字符串，框架 `parse_lifecycle_message` 直接拒绝 |
| `services/system_channels.dart:401-404` — `flutter/system` 是 `BasicMessageChannel<Object?>` + `JSONMessageCodec` | `ChannelContract { kind: BasicMessage, codec: Json }` | 产出 `{"type":"memoryPressure"}`，与 `channel_event_bridge::decode_system_message` 同一份字节 |
| `services/system_channels.dart:181-184` — `flutter/platform` 是 `OptionalMethodChannel` + `JSONMethodCodec` | `ChannelContract { kind: Method{optional:true}, codec: Json }` | 方法调用编码为 `{"method":…,"args":…}`；缺 handler 仍是 null reply |
| `services/system_channels.dart:423,430` + `services/platform_channel.dart:291-299` — `flutter/platform_views`/`_2` 是 `MethodChannel`，默认 `StandardMethodCodec` | `ChannelContract { kind: Method{optional:false}, codec: Standard }` | 方法调用首字节是字符串 tag；错误信封首字节是 1 |
| `services/platform_channel.dart:202-256,622-633` — 基础消息通道与可选方法通道的 null 语义 | `encode/decode_channel_message`、`encode/decode_channel_envelope` | null 消息不上线（无字节），null reply 即 `MissingPlugin`；错配通道类型返回 typed `Unsupported` |
| `services/message_codecs.dart:44-62,88-114,120-200,263-324,580-596` — 五种 codec 的线格式 | `ChannelCodec { Standard, Json, String }` | 跨 codec 互解必然失败：JSON 字节进不了标准通道，标准字节进不了 JSON 通道 |

同时修正一处字节级偏差：`JSONMethodCodec.encodeMethodCall` 在 Flutter 里构造 `{'method': method, 'args': arguments}`，`JsonUtf8Encoder` 保留插入序，字节为 `method` 在前；原实现经通用 JSON 对象写出后键序被排序，现改为按 Flutter 插入序直接写字节（`src/framework_channels.rs`），并同步更新既有单测期望。

第二十五轮审计的 `unused` 基线随之收敛：`flutter/lifecycle`、`flutter/platform`、`flutter/platform_views`、`flutter/platform_views_2` 四条通道名常量被契约表引用后不再是「已声明未接线」，`unused` 由 7 项降为 3 项。无渲染、IR、解释器改动，默认态零漂移。

### 第二十七轮：深度轮次 — 官方框架通道全量清单

新增 `src/platform_channel::system_channels` 的 19 条官方通道名常量，并把 `channel_contract` 的契约表扩成 Flutter 3.50 的完整框架通道清单（24 条），每条带通道类型、是否可选、codec、是否桥接与源码行号。清单里的每一条都逐字对照 `system_channels.dart` 的声明读取，不自创任何通道名。

| Flutter 源码（3.50-candidate） | Opurom 对应 | 验证 |
| --- | --- | --- |
| `services/system_channels.dart:76-79` — `flutter/navigation`，`OptionalMethodChannel` + `JSONMethodCodec` | 清单条目，标记 `Coverage::Declared` | 编解码路由返回 typed `Unsupported("channel has no bridge route")` |
| `services/system_channels.dart:98,210,342,450,478,501,566,599,616` — `flutter/backgesture`/`processtext`/`spellcheck`/`mousecursor`/`restoration`/`deferredcomponent`/`menu`/`keyboard`/`sensitivecontent`，`OptionalMethodChannel` 默认 `StandardMethodCodec` | 同上 | 同上 |
| `services/system_channels.dart:201-204,293-296,322,345-348,514-517,576-579` — `flutter/status_bar`/`textinput`/`scribe`/`undomanager`/`localization`/`contextmenu`，`OptionalMethodChannel` + `JSONMethodCodec` | 同上 | 同上 |
| `services/system_channels.dart:370-373,413-416` — `flutter/keyevent` 与 `flutter/accessibility` 是 `BasicMessageChannel`，分别用 `JSONMessageCodec` 与 `StandardMessageCodec` | 同上 | 同上 |
| `services/system_channels.dart:439` — `flutter/skia` 是严格 `MethodChannel` + `JSONMethodCodec` | 同上 | 同上 |
| `services/platform_channel.dart:291-299` — `MethodChannel` 的默认 codec 是 `StandardMethodCodec` | 清单中未显式传 codec 的条目一律记为 `ChannelCodec::Standard` | 清单的 codec 列与 Flutter 构造实参逐条一致 |

清单与第二十五轮的审计表互为独立来源：`tests/event_code_dictionary_audit.rs` 用自己固定的官方表逐名比对实现的 24 条清单，任何一条名称漂移都会让两边对不上。已桥接 5 条、未桥接 19 条，未桥接条目在编解码入口被 typed 拒绝，不会被误当成可用通道。无渲染、IR、解释器改动，默认态零漂移。

### 第二十八轮：深度轮次 — 插件通道清单与通道契约对齐

把插件生态规范的通道侧补齐：新增 `channel_contract::PLUGIN_CHANNELS`（6 条），逐条取自本地 `flutter-plugins` 快照（`557d3284ac9dda32a1106bb75be8a23bdccd2f96`），并让 `contract()` 同时检索框架清单与插件清单。插件通道在快照里都没有显式 codec，因此一律记为默认 `StandardMethodCodec`；相机图像流是唯一一条 `EventChannel`，`ChannelKind::Event` 由此有了真实来源。

| Flutter/插件源码（3.50-candidate） | Opurom 对应 | 验证 |
| --- | --- | --- |
| `flutter-plugins/packages/camera/camera_platform_interface/lib/src/method_channel/method_channel_camera.dart:17` — `plugins.flutter.io/camera` 是 `MethodChannel` | 插件清单条目，引用 crate 已发布的 `camera_methods::CHANNEL` | 常量与清单互为引用，`camera_methods::CHANNEL` 不再是「已声明未接线」 |
| 同文件 `:323-324` — `plugins.flutter.io/camera/imageStream` 是 `EventChannel` | `ChannelKind::Event` 条目 | 通道类型逐条比对独立固定的插件表 |
| `flutter-plugins/packages/path_provider/path_provider_platform_interface/lib/src/method_channel_path_provider.dart:16`、`url_launcher/url_launcher_platform_interface/lib/method_channel_url_launcher.dart:12`、`image_picker/image_picker_platform_interface/lib/src/method_channel/method_channel_image_picker.dart:12`、`shared_preferences/shared_preferences_platform_interface/lib/method_channel_shared_preferences.dart:12` | 其余 4 条插件通道条目 | 名称、类型、codec 与快照逐条一致 |

审计基线随之再收敛：`plugins.flutter.io/camera` 被插件清单引用后不再是「已声明未接线」，`unused` 由 3 项降为 2 项（只剩 Flutter 自身也从不写出的标签 5，以及只作类型级标签的 `missing_plugin`）。审计测试同时新增「实现插件清单 vs 审计固定插件表」逐名交叉核对 1 项。无渲染、IR、解释器改动，默认态零漂移。

### 第三十轮：深度轮次 — 平台视图按端 typed 契约

`src/platform_view_bridge.rs` 把统一的 `view_methods` 表细化为按平台族注入的 typed 操作集：新增 `ViewPlatform::{Android,Darwin}`、`PlatformViewBridge::for_platform` / `platform()`；每次调用在进入 handler 前先做平台归属检查，不属于宿主平台族的操作返回 typed `unimplemented`，不再静默成功。

| Flutter 源码（3.50-candidate，`platform_views.dart`） | Opurom 对应 | 验证 |
| --- | --- | --- |
| `:746-1346` — `AndroidViewController` 操作集：`touch` (`:936,1255`)、`setDirection` (`:989`)、`clearFocus` (`:1037`)、`resize` (`:1442`)、`offset` (`:1472`)、`isSurfaceControlEnabled` (`:1530`) | `ViewPlatform::Android` 操作表 | Android 调用保持通过，Darwin 上 `setDirection` 被拒 |
| `:1579-1642` — `DarwinPlatformViewController` 仅有 `acceptGesture` (`:1617`) / `rejectGesture` (`:1627`)；`:1600-1604` 明确方向方法尚不支持 | `ViewPlatform::Darwin` 操作表 | Darwin `rejectGesture` 通过；Android `acceptGesture` 被拒 |
| `:286,325,1386-1388` — `create`；`:1481,1517,1560,1637` — `dispose`；`:152,181,210,238,288,327` — `viewFocused` 回调 | 两族共享方法 | 共享方法在两族仍可用 |
| `:1406,1568,1627` — Android 的 `rejectGesture` 可带 `gestureId`，Darwin 只带 `id` | 统一参数校验 + 平台族方法表 | 既有 `gestureId` 整数测试与 Darwin `rejectGesture` 测试同时通过 |

默认构造仍为 Android，保持既有调用方行为；无渲染、IR、解释器改动，默认态零漂移。
### 第二十九轮：深度轮次 — 能力/插件通道/manifest 三方对齐

新增 `tests/capability_channel_alignment.rs`（4 项）：把 `native_capabilities` 的 14 类能力、`channel_contract::PLUGIN_CHANNELS` 的插件通道清单、以及 `flutter_tools` 从 app-facing `pubspec.yaml` 生成的 `.flutter-plugins-dependencies` 三方对齐，输出机器可读差异报告（格式 `class|subject|detail|source`）。四类差异：`declared_no_source`（能力声称有插件但快照里没有）、`plugin_channel_missing`（快照有通道但 Opurom 无能力认领）、`gap_with_source`（GAP 能力伪造实现）、`duplicate_channel`。干净运行四类均为 0 行。

| Flutter 源码（3.50-candidate） | Opurom 审计项 | 验证 |
| --- | --- | --- |
| `packages/flutter_tools/lib/src/flutter_plugins.dart:366-386` — 生成清单按 6 个平台键逐条展开 | 6 个 app-facing 插件的 `pubspec.yaml` 平台键冻结成表 | 每包至少一个平台键；本地快照存在时逐字比对实际 pubspec |
| `flutter_plugins.dart:478-503` — 清单条目含 name/path/native_build/dependencies | 每个插件能力的包名与通道名来自固定对齐表 | 包存在且通道在 `PLUGIN_CHANNELS` 中，否则报 `declared_no_source` |
| `flutter_plugins.dart:563-575,943-956` — 平台 registrant 从 method-channel 插件生成 | 每条插件通道必须被恰好一个能力认领 | owner count ≠ 1 报 `plugin_channel_missing` |
| `flutter-plugins/packages/*/*/pubspec.yaml:13-25` — 6 个插件的平台键 | 冻结在测试内，CI 无需额外 clone | GitHub Actions 只 clone `opurom-examples`，审计仍可独立运行 |

两处分类需要显式说明，否则会误报：

- `Camera` 的 `plugins.flutter.io/camera/imageStream` 是逐实例 `EventChannel`，名字带运行时 texture id（`camera_platform_interface/.../method_channel_camera.dart:323-324`），没有可冻结的稳定 wire 名，从 owner/duplicate 检查中排除。
- `VideoPlayer` 在快照里只有逐实例事件通道（`video_player_android/.../VideoPlayerPlugin.java:121-123`），没有稳定命令通道，记为 `PerInstanceEvent`；`DeviceInfo` 的本地 display/geometry 投影不是 `device_info_plus` parity，仍保持 typed GAP。

审计器自检覆盖「缺包」与「错误 GAP 分类」两类探针；报告行全部以 `|` 分隔四字段，可直接被机器消费。首次远端 CI 发现 `flutter-plugins` 不在本仓 clone 列表内，现改为「有快照时交叉核对、无快照时用冻结表独立审计」，并新增 CI 缺快照路径的测试。无渲染、IR、解释器改动，默认态零漂移。

### 第三十一轮：深度轮次 — 未桥接框架通道处置矩阵

新增 `tests/declared_channel_disposition.rs`（7 项）：为 `channel_contract` 中 19 条已声明未桥接的 `flutter/*` 通道逐条固化处置结论。每条通道必须落入两类之一：`ir_path`（Opurom 自有 IR 路径已承载同一机制，并在测试中验证对应源码符号存在）或 `unimplemented`（无宿主能力，继续由编解码入口 typed 拒绝）。报告格式为 `channel|codec|disposition|ir_path|source_pin`，可直接机器消费。

| 处置 | 通道 | 依据 |
| --- | --- | --- |
| `ir_path` | `flutter/backgesture`、`flutter/keyevent` | `input_bridge.rs::InputRouter` 承载 pointer/gesture 输入；Flutter 对应 `services/back_gesture.dart` 与 `services/raw_keyboard.dart:379,757` |
| `ir_path` | `flutter/textinput` | `ime.rs::ImeStatus` 承载 IME 文本输入；`system_channels.dart:293-296` |
| `ir_path` | `flutter/accessibility` | `a11y.rs::map_a11y_tree` 承载无障碍树；`system_channels.dart:413-416` |
| `ir_path` | `flutter/keyboard` | `event_producers.rs::keyboard_visibility` 承载键盘可见性；`services/hardware_keyboard.dart:603-606` |
| `unimplemented` | 其余 14 条（navigation/status_bar/processtext/scribe/spellcheck/undomanager/skia/mousecursor/restoration/deferredcomponent/localization/menu/contextmenu/sensitivecontent） | 每条均逐字取自 `system_channels.dart` 对应声明行；无 Opurom 宿主能力，不造通道 |

所有 19 条通道在三个编码/解码入口（消息、方法调用、错误信封）都返回 `Unsupported("channel has no bridge route")`；`StringCodec` 仍只属于已桥接的生命周期通道。无渲染、IR、解释器改动，默认态零漂移。

### 第二十六轮：深度轮次 — 宿主注入失败路径收敛

`tests/host_injection_failure_paths.rs` 新增 `registration_failures_are_typed_isolated_and_replayable`：重复 manifest 名无法生成注册计划；冻结计划中注入 `path` 宿主面失败时，`camera`/`share` 仍注册，权限集合只合并成功插件；重复执行得到完全一致报告，计划本身不变。源码依据 `flutter_tools/lib/src/flutter_plugins.dart:480-498,528-551`（按平台生成与逐插件 try/catch）、`:380-420`（权限并集）。无渲染、IR、解释器改动，默认态零漂移。

## 六、验证与 CI

- 命令：`cd opurom-platform && cargo fmt --all -- --check && cargo test --all-targets`。
- 结果：2026-11-18 三十一轮本地全量通过；合计 141 个库单测 + 36 个集成测试组、共 404 项测试全部通过（关键退出码：test=0 / fmt=0 / linux-check=0 / windows-check=0 / mac-check=0）。
- 本模块新增测试文件：`tests/platform_channel.rs`（12）、`tests/native_capabilities.rs`（7）、`tests/event_code_dictionary_audit.rs`（8）、`tests/channel_codec_contract.rs`（10）、`tests/capability_channel_alignment.rs`（5）、`tests/declared_channel_disposition.rs`（7）；新增模块内单测 18 项；全部随 `cargo test --all-targets` 进入既有 CI 矩阵 `opurom-platform/.github/workflows/ci.yml`。

## 七、剩余 GAP（不得自行降标，按优先级继续）

1. **设备信息/网络/权限/分享**：`Share.invoke` 框架方法名已在 `framework_channels` 冻结；设备信息/网络/权限因本地 `flutter-plugins` 快照无官方插件源码，按硬约束不自创 API，保持 typed `unimplemented`。
2. **真实宿主后端**：`PreferencesBackend`、`PathResolver`、`UrlLauncherBackend`、`ImagePickerBackend` 均已落地并接入 `CapabilityTable`；macOS 生命周期观察者已接入；各端原生 opener/picker 表面仍需 mobile/web/desktop 宿主注入实现。
3. **系统事件生产者**：纯函数生产者已落地（`event_producers.rs`）；宿主侧 OS 信号采集点（Android/iOS 原生回调）尚未接入，需各宿主注入。
4. **插件注册执行端**：`RegistrationPlan` 是冻结计划；`opurom-plugins` CLI 与 `opurom-cli` 各端工程生成尚未消费该计划。
5. **跨仓库对齐**：`opurom-runtime-adapters` 的 9 类能力码与此处 12 类契约尚未统一命名，需 adapters 侧协同（本模块不修改他仓库代码）。
6. **框架通道接线**（第二十五轮审计结论）：官方 24 条 `flutter/*` 框架通道中，桥接词表只覆盖 5 条；其余 19 条（`flutter/accessibility`、`flutter/backgesture`、`flutter/contextmenu`、`flutter/deferredcomponent`、`flutter/keyboard`、`flutter/keyevent`、`flutter/localization`、`flutter/menu`、`flutter/mousecursor`、`flutter/navigation`、`flutter/processtext`、`flutter/restoration`、`flutter/scribe`、`flutter/sensitivecontent`、`flutter/skia`、`flutter/spellcheck`、`flutter/status_bar`、`flutter/textinput`、`flutter/undomanager`）尚无 `flutter/` 同名通道。其中输入、IME、无障碍三类在本 crate 由 `input_bridge` / `ime` / `a11y` 自有 IR 路径承担，不走 Flutter 命名通道，其余需按宿主能力逐条评估后再决定是否建通道。
7. **已声明未接线的通道名常量**：第二十六轮把 `system_channels::LIFECYCLE`、`PLATFORM`、`PLATFORM_VIEWS`、`PLATFORM_VIEWS_2` 接入 `channel_contract` 的编译期通道契约表，第二十八轮把 `camera_methods::CHANNEL` 接入插件通道清单，通道名常量已全部有引用；剩下的缺口是这些通道尚未被任何生产 `ChannelTable` 路由，待宿主适配器建立通道表时接入。
