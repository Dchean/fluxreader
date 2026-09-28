# TASK-094 交互取证报告（REQ-107，修后 bundle + mock_backend_094.py）

- 环境：Chrome headless CDP + Tauri IPC shim（前端走真实 IPC 代码路径），视口 1440×1400。
- fixture：审计第三轮忠实假后端（与基线 s7b B1 同源、同形：稀疏布局条目分散在全局较老位置）。
- 各布局后端条目数：{"article":1268,"image":44,"notification":20,"podcast":119,"social":1554}。
- 修前复现（同后端 + HEAD bundle）：见 prefix-repro.json / prefix-gallery-b1.png
  （画廊 5/44、播客 6/119、通知 1/20 且容器不可滚动，与基线记录一致）。
- 页面错误：[]

## 逐项结论

| id | 名称 | 结论 | 观察 |
| --- | --- | --- | --- |
| R4.article-first-page | 文章布局首屏：请求带 layout=article / offset=0，容器可滚动，哨兵「滚动加载更多」 | PASS | scrollHeight=2426 clientHeight=1324 可滚动=true 哨兵="滚动加载更多" 卡片(虚拟窗口)=18；IPC[0]={"feed_id":null,"folder_id":null,"newest_first":true,"layout":"article","limit":500,"offset":0,"with_content":false} |
| R4.article-page2 | 文章布局滚动翻页：取页 offset 500 步进连续（游标不跳不重），窗口内无重复 id | PASS | 本窗口 article offsets=[1000]（步进 500 连续=true）；窗口卡片 27 个、去重后 27；哨兵="没有更多了" |
| R1.gallery-reach-all | 画廊可达全部：渲染卡片数 == 后端条目数（44），首批 offset=0 | PASS | IPC layout=image offset=0；DOM 卡片=44 后端=44；scrollHeight=2030 clientHeight=1324 哨兵="没有更多了" |
| R2.podcast-reach-all | 播客可达全部：列表模型 119 条（首批 500 全量覆盖），滚到底末条 data-index=118 | PASS | IPC layout=podcast offset=0；滚动后 maxIndex=118（=后端 119 - 1）；scrollHeight=12152 clientHeight=1324 哨兵="没有更多了" |
| R3.notification-reach-all | 通知可达全部：列表模型 20 条，滚到底末条 data-index=19 | PASS | IPC layout=notification offset=0；maxIndex=19（=后端 20 - 1）；scrollHeight=2214 clientHeight=1324 哨兵="没有更多了" |
| R7.layout-switch-cursor | 切布局游标不串：画廊/播客/通知首批均 offset=0（各记各的桶）；切回文章快照首屏一致、调用从 offset=0 起 500 步进连续（不重复不跳页） | PASS | 切回后首屏前 10 id 与离开前一致=true；本窗口 article offsets 去重排序=[0,500]（从 0 连续步进=true）；窗口去重 27/27 |
| R6.empty-state-consistent | 真无内容：只显示既有空态文案，不出现承诺性的加载提示（哨兵整体不渲染） | PASS | 空态文案="暂无收藏内容"；.timeline-load-more=不存在 |
| R4.social-page2 | 社交布局滚动翻页不回退：滚动触发的取页 offset 500 步进连续、窗口无重复 | PASS | 本窗口 social offsets=[500]（步进 500 连续=true）；窗口去重 15/15 |
| R5.auto-refill-cap | 不足一屏自动续拉有上限：可见 0 条时自动续拉恰 8 次后停手（未到底：offset 4500 处还剩 185 条未取） | PASS | feed6 首批 offset=0；自动续拉 8 次（上限 8）offsets=[500,1000,1500,2000,2500,3000,3500,4000]；可见=0；scrollHeight=1324==clientHeight=1324；哨兵="加载更多"（原生 button=true） |
| R5.hint-executable | 不可滚动哨兵为可点击「加载更多」按钮：键盘可聚焦、键盘激活触发一次真实加载（31 条未读可达） | PASS | 聚焦元素={"tag":"BUTTON","cls":"toggle-action-btn load-more-btn"}；键盘激活（Enter）前 IPC 13 → 后 14（恰一页 offset=4500）；滚动到底 maxIndex=30（31 条未读 = 0..30）；哨兵="没有更多了" |
| R5.button-visible | 不可滚动 + 未到底：哨兵渲染为原生 button（toggle-action-btn 体系，含「加载更多」文案） | PASS | 按钮文本="加载更多"；可见=0；哨兵="加载更多" |
| R5.click-loads | 鼠标点击「加载更多」真实加载：一次点击取回末页（31 条未读可达）并显示「没有更多了」 | PASS | 点击后滚到底 maxIndex=30（31 条未读 = 0..30）；哨兵="没有更多了"；scrollHeight=4695 clientHeight=1324 |
| R1.gallery-light | 浅色主题画廊同样可达全部（44 张） | PASS | data-theme=light；卡片=44 后端=44 |

## 布局可达性口径说明

- 画廊（image）布局**不虚拟化**：DOM 卡片数即列表全量，直接断言 == 后端条目数。
- 文章/社交/播客/通知为**虚拟滚动**（只渲染视口 + overscan，与本修复无关的既有设计）：
  可达性以「列表模型条目数（首批 IPC limit=500 全量覆盖该布局）+ 滚到底后末条
  data-index == N-1 + 哨兵『没有更多了』」三重证据判定；DOM 同帧卡片数为虚拟窗口大小，
  非列表全量。滚到底采用多轮 scrollBottom 迭代（每轮渲染窗口前进约 9 条，探针校准见
  probe_094.mjs）。
- 文章布局首屏可能有一次**有界的急切预取**（续拉判定在虚拟器首次测量前读到「未撑满」，
  自动多取一页后收敛）：表现为 offsets 从 0 起 500 步进连续，游标不跳不重，有上限 8 兜底。
