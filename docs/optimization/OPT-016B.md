# OPT-016B：真实挂载组件回归

Status: verified-local
父卡OPT016的挂载测试子卡，前置OPT015（已审代码）、不依赖未来AI继承接口。可先建立实际生产组件DOM回归，后续OPT011/013补对应生命周期。

## 技术选择

使用jsdom（devDependency，选择明确支持CI Node22的已发布版本）+React act/createRoot，沿用现有tsx/ui-loader和同一Zustand store实例，Node内置test运行。不要用SSR证明effect、不要启真实Tauri IPC、不要访问真实网络；Mock仅外部IPC和Resize/Intersection等缺失浏览器API，实际组件与effects不替身。不引入完整Vitest第二配置体系或第二套应用缓存。

## 真实测试

- 同一挂载ReaderProse源码→render→source，DOM危险标签在source不创建、切回render合法元素存在，事件/样式/滚动宿主不丢；raw translated mode纯文本。
- 挂载真正App或Timeline五布局切换，store真实数据驱动，捕获console.error/React onCaughtError/onUncaughtError的componentStack，连续切换不出现getSnapshot循环；不是只有css类变化。
- CloseAskDialog实际出现时keyboard事件不修改背后selected article读/藏/导航；Escape按原规则；其它floating cases至少一条回归。
- bodycache记录更新与Reader/卡片hydrate前后同一实例，不用先unmount再render绕过effect；mock真实IPC返回不同延迟/缺行/失败可重试，避免每次都返回完美fixture。

## 写集

package.json、package-lock.json（仅dev jsdom和必要依赖，不恢复source-map漏洞）、tools/component-tests/、tools/component-regression.mjs、tools/ui-loader.mjs仅必要可测路径兼容、tsconfig.test.json仅扩编译面；.github/workflows/ci.yml增加npm run test:components（保留lint/build/旧状态测试）；旧testing Note新增挂载证据边界。

不能改生产components/store来绕过测试，发现真实bug报告主控另卡修；不得调整已过的787前端行为。

## 验收

npm ci/npm audit确认依赖与Node22兼容，旧testfrontend与build/lint仍过；testcomponents实际触发effect/keyboard/异步，并至少对一条条件变异证明失败（只tmp/内存，不改生产留变异）。成功记录组件stack捕获路径可用，不虚构桌面Windows原生测试。独立review后CI实际运行；父卡016最终拆分大文件尚未完成。

## 验收证据

Dirac R3 PASS。真实挂载6组与8个runner黑盒探针日志通过；R2晚异步错误用原反例独立复核exit1而非假绿。执行者最终组件6/6、lint0，已有前端回归787保护保留。主控采用对应交付证据，提交级CI验证Node22与整合；不默认重复同命令。
