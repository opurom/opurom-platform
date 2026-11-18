# 平台桥接体系 Flutter 对标实现记录（2026-11-18）

责任模块：模块五「原生能力桥接体系」。
Flutter 版本：**3.50-candidate**，engine `371f77202a73e8dff57a8b3bff75bca55c090aee`（本地 `flutter-golden`）。
插件快照：`flutter-plugins` @ `557d3284ac9dda32a1106bb75be8a23bdccd2f96`。
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

## 五、验证与 CI

- 命令：`cd opurom-platform && cargo fmt --all -- --check && cargo test --all-targets`。
- 结果：2026-11-18 全量所有测试组通过（最后一组统计：79 个单元测试 + 各集成测试组，共 24 组 `test result: ok`），rustfmt 无漂移。
- 本模块新增测试文件：`tests/platform_channel.rs`（12）、`tests/native_capabilities.rs`（7）；模块内单测位于对应 `src/*.rs`；全部随 `cargo test --all-targets` 进入既有 CI 矩阵 `opurom-platform/.github/workflows/ci.yml`。

## 六、剩余 GAP（不得自行降标，按优先级继续）

1. **设备信息/网络/权限/分享**：本地 `flutter-plugins` 快照无官方插件源码，按硬约束不能自创 API；需先取到 pinned 官方源码再实现。已用 typed `unimplemented` 占位，不伪造成功。
2. **真实宿主后端**：`preferences`/`path_provider`/`url_launcher`/`image_picker` 目前只有方法契约，未接各端原生实现；需与 mobile/web/desktop 各宿主对接。
3. **系统事件生产者**：`SystemEventDispatcher` 只做分发；宿主侧键盘/旋转/亮度/省电/刷新率的 OS 信号采集尚未接入。
4. **插件注册执行端**：`RegistrationPlan` 是冻结计划；`opurom-plugins` CLI 与 `opurom-cli` 各端工程生成尚未消费该计划。
5. **跨仓库对齐**：`opurom-runtime-adapters` 的 9 类能力码与此处 12 类契约尚未统一命名，需 adapters 侧协同（本模块不修改他仓库代码）。
