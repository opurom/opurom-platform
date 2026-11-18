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

## 六、验证与 CI

- 命令：`cd opurom-platform && cargo fmt --all -- --check && cargo test --all-targets`。
- 结果：2026-11-18 十七轮本地全量通过；第十七轮新增确定性 fuzz 矩阵 3 项，合计 123 个库单测 + 25 个集成测试组全部通过（关键退出码：test=0 / fmt=0 / linux-check=0 / windows-check=0 / mac-check=0）。
- 本模块新增测试文件：`tests/platform_channel.rs`（12）、`tests/native_capabilities.rs`（7）；新增模块内单测 10 项；全部随 `cargo test --all-targets` 进入既有 CI 矩阵 `opurom-platform/.github/workflows/ci.yml`。

## 七、剩余 GAP（不得自行降标，按优先级继续）

1. **设备信息/网络/权限/分享**：`Share.invoke` 框架方法名已在 `framework_channels` 冻结；设备信息/网络/权限因本地 `flutter-plugins` 快照无官方插件源码，按硬约束不自创 API，保持 typed `unimplemented`。
2. **真实宿主后端**：`PreferencesBackend`、`PathResolver`、`UrlLauncherBackend`、`ImagePickerBackend` 均已落地并接入 `CapabilityTable`；macOS 生命周期观察者已接入；各端原生 opener/picker 表面仍需 mobile/web/desktop 宿主注入实现。
3. **系统事件生产者**：纯函数生产者已落地（`event_producers.rs`）；宿主侧 OS 信号采集点（Android/iOS 原生回调）尚未接入，需各宿主注入。
4. **插件注册执行端**：`RegistrationPlan` 是冻结计划；`opurom-plugins` CLI 与 `opurom-cli` 各端工程生成尚未消费该计划。
5. **跨仓库对齐**：`opurom-runtime-adapters` 的 9 类能力码与此处 12 类契约尚未统一命名，需 adapters 侧协同（本模块不修改他仓库代码）。
