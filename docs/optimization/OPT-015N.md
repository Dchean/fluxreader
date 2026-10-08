# OPT-015N：隔离 Windows 原生窗口恢复验收

Status: coding
父卡OPT015的实机证据补强；目的是真实窗口首次可见位置，不以mock run_sequence替代。只使用测试自己的状态文件，不启动FluxReader真实用户数据/调度/同步。

## 写集

- src-tauri/examples/window_startup_smoke.rs（新 Windows-only 原生验收驱动；非Windows编译一个明确不支持main）
- tmp/optimization-20261008/OPT-015N/ 下的runner/结果/状态样例/编译产物
- docs/optimization/OPT-015N.md 由主控写，执行者不得改要求；不得改生产lib/window_startup.rs/tauri.conf.json、依赖和锁文件

## 实现原则

Tauri示例复用真实生产window_startup.rs模块（path引入，避免为测试加public接口）和锁定插件，读取当前tauri.conf.json的main初始visible/geometry配置；不调用app_lib::run，不注册数据库、定时同步、AI或真实账号配置。插件with_filename使用本任务临时绝对路径，或Context identifier改随机隔离值且确认状态根在本任务目录，绝不能读/写正式com.fluxreader.app数据。

创建真实Window/WebView，不用MockRuntime。每个场景独立进程/EventLoop，脚本runner汇总。必须防已有用户实例影响：此示例不启生产single-instance/tray。无网络页面，载入本地最小HTML/data而不是生产App会触发IPC/抓取。Windows进程避免可见console。

## 首帧证据

在调用真实restore_and_show之前断言窗口不可见；使用窗口事件/Win32 subclass等同步观测首次SHOW/窗口位置变动，在首次可见边界记录坐标和flags，并断言与恢复目标相符；只有show之后查询一次坐标不够（那会漏掉本问题的跳动）。若受限无法获取可靠first-visible证据，明确BLOCKED/不足，不写PASS。可以手写示例内最小Win32 FFI，不新增windows crate features依赖（当前windows core已有）。不可注入/控制别的用户窗口。

场景：无记录默认居中；保存非居中可见屏幕内位置；保存visible=false仍显示；损坏JSON仍显示；保存离开所有现有显示器的坐标回退可达位置；最大化状态恢复（需要首次可见状态/位置观测）；每个场景固定输入和观测JSON。多物理显示器移除/DPI无法模拟时如实列未验。状态结构取锁定plugin2.4.1，不猜字段。

## 运行

示例必须--help/无授权标志只打印说明，不意外弹窗口；仅--run-smoke配合绝对output路径才运行。主控允许本任务临时窗口用于验收，不要操作现有FluxReader/系统设置。设置全程有限timeout，失败进程退出非0，清理只自己创建的文件/窗口，停止后不留后台线程。

运行cargo run --manifest-path src-tauri/Cargo.toml --example window_startup_smoke -- <明确参数>，如有共享代码WIP编译错误报告，不改他人文件。输出逐场景证据/实际窗口是否显示/退出码，严禁只模拟日志。以可执行真实观测为目标，工程实现困难也不能把mock改名native。

## 交付边界

此卡只验证窗口，不代替真实SMTC音频整链、多显示器拔插或WebView复杂App交互。独立review驱动可信度后主控复跑，才能写父卡相关native证据。不得写个人memory、Git、真实app状态、安装新依赖或访问网络。
