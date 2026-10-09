// tools/frontend-tests/timeline-and-playback.mjs
// 领域模块：时间流交互：播放器状态、同集切换、快捷键让路、滚动标读判据
// OPT-016C 拆分自 tools/frontend-regression.mjs（旧行区间 1300-1587）；
// 断言名称/条件文本原样迁移，仅做路径深度适配（import.meta.url 与动态 import 深一层）与
// 共享可变状态的 S. 归属重写。数据所有权：共享假后端/夹具/记录归 harness（见 harness.mjs 头注），
// 本模块不 import 第二份 store；域内自带夹具（大行集等）仍在本模块内独立构造与复位。
export const id = 'timeline-and-playback';

export async function run(ctx) {
  const { store, checkNew, bootFixture } = ctx;
  await ctx.useMainBackend();

  /* ============================================================
     (j) 播放器状态（激活/播放/进度/seek 夹取/结束）
     ============================================================ */
  await bootFixture();
  store.setState({ toasts: [], playerExpanded: true, activeArticleId: null });
  store.getState().playPodcastEpisode('无音频', '节目', '', '');
  checkNew('(j) 无音频地址时不进入播放态，只给提示',
    store.getState().player.isActive === false && store.getState().player.isPlaying === false
    && store.getState().toasts.some((t) => t.text === '该剧集没有可播放的音频地址'));
  store.getState().playPodcastEpisode('第 1 集', '节目名', 'https://cover/1.jpg', 'https://audio/1.mp3', '104');
  const jPlay = store.getState().player;
  checkNew('(j) 播放剧集：激活 + 播放中 + 位置/时长归零 + seek 清空 + 标题信息写入',
    jPlay.isActive === true && jPlay.isPlaying === true && jPlay.title === '第 1 集' && jPlay.showName === '节目名'
    && jPlay.audioUrl === 'https://audio/1.mp3' && jPlay.positionSec === 0 && jPlay.durationSec === 0
    && jPlay.seekToSec === null && jPlay.cover === 'https://cover/1.jpg');
  checkNew('(j) 点播放即视为已读（与打开文章同语义）：104 标读 + 该源未读 -1 + 播放 toast',
    store.getState().entries.find((a) => a.id === '104')?.isRead === true
    && store.getState().feedCounts.get('12')?.unread === 1
    && store.getState().toasts.some((t) => t.text === '正在播放：第 1 集'));
  store.getState().togglePlayerPlay();
  const jPaused = store.getState().player.isPlaying;
  store.getState().togglePlayerPlay();
  checkNew('(j) togglePlayerPlay 双向切换且不动其他播放器字段',
    jPaused === false && store.getState().player.isPlaying === true
    && store.getState().player.title === '第 1 集' && store.getState().player.durationSec === 0);
  store.getState().syncPlayerProgress(100, 300);
  store.getState().skipPlayer(-250);
  checkNew('(j) skipPlayer 负向越界夹取到 0（seekToSec 与 positionSec 同步下发）',
    store.getState().player.positionSec === 0 && store.getState().player.seekToSec === 0);
  store.getState().skipPlayer(50);
  checkNew('(j) skipPlayer 正向按当前位置推进（0 → 50）',
    store.getState().player.positionSec === 50 && store.getState().player.seekToSec === 50);
  store.getState().seekPlayer(-5);
  checkNew('(j) seekPlayer 负数夹取为 0（不把负时间写进 audio.currentTime）',
    store.getState().player.seekToSec === 0 && store.getState().player.positionSec === 0);
  store.getState().seekPlayer(42.5);
  checkNew('(j) seekPlayer 正常值原样落到 seekToSec + positionSec',
    store.getState().player.seekToSec === 42.5 && store.getState().player.positionSec === 42.5);
  store.getState().playerEnded();
  const jEnded = store.getState().player;
  checkNew('(j) playerEnded 停止播放并归零位置/清 seek，但保留剧集信息与时长（可重播）',
    jEnded.isPlaying === false && jEnded.positionSec === 0 && jEnded.seekToSec === null
    && jEnded.isActive === true && jEnded.durationSec === 300 && jEnded.title === '第 1 集');

  /* ---------- P3[F3]（TASK-081）：同集再点 = 播放/暂停切换，不从头重播 ----------
     判据是 src/store/selectors.ts 导出的纯函数 `podcastClickAction`。
     **证据分两层，边界如实说明**（审查 FINDING TASK-081-F1 / R2-F1）：
       ① 本组断言直接驱动该纯函数，并有变异取证（改回无条件 play → 恰 2 条失败）；
       ② 但纯函数有牙 ≠ 组件真的调用它：只断言纯函数时，「删掉组件的守卫」依然全绿。
          故紧随其后另加**源码形态断言**（沿用本文件既有的 readFileSync 核对手法），
          钉住 Timeline.tsx 的播放入口确实按 toggle 分支走。
     两层范围不同，不可互相冒充。 */
  const { podcastClickAction } = await import('../../src/store/selectors.ts');
  checkNew('(P3[F3]) 播放中 + 同 audioUrl ⇒ toggle（播放/暂停切换，不从头重播）',
    podcastClickAction(true, 'https://a.example/ep2.mp3', 'https://a.example/ep2.mp3') === 'toggle');
  checkNew('(P3[F3]) 未激活 ⇒ play（首次点某集应正常开始播放）',
    podcastClickAction(false, '', 'https://a.example/ep2.mp3') === 'play');
  checkNew('(P3[F3]) 播放中但换了另一集 ⇒ play（换集仍从头播，不得被同集判据拦下）',
    podcastClickAction(true, 'https://a.example/ep2.mp3', 'https://a.example/ep3.mp3') === 'play');
  checkNew('(P3[F3]) 无音频地址 ⇒ play（交给 playPodcastEpisode 走它自己的「无可播放地址」提示）',
    podcastClickAction(true, 'https://a.example/ep2.mp3', '') === 'play');
  /* 修前对照：旧行为是无条件 playPodcastEpisode（进度被清零重开），
     即「同集也判 play」——用同一组输入复现修前判据，证明本断言有区分力。 */
  const legacyAction = () => 'play';
  checkNew('(P3[F3]) 修前判据可复现：无条件 play 会把「同集」也判为从头重播（进度清零的根因）',
    legacyAction() === 'play'
    && podcastClickAction(true, 'https://a.example/ep2.mp3', 'https://a.example/ep2.mp3') !== 'play');
  /* 行为侧对照：走 toggle 保留进度、走 play 归零（锁住两条路径的实际后果）。 */
  store.getState().playPodcastEpisode('第 2 集', '节目', null, 'https://a.example/ep2.mp3', null);
  store.getState().syncPlayerProgress(120, 600);
  store.getState().togglePlayerPlay();
  checkNew('(P3[F3]) 走 toggle 路径：暂停且**进度保留**（修前会重头播并清零）',
    store.getState().player.isPlaying === false && store.getState().player.positionSec === 120);
  store.getState().playPodcastEpisode('第 3 集', '节目', null, 'https://a.example/ep3.mp3', null);
  checkNew('(P3[F3]) 走 play 路径（换集）：从头播放、位置归零',
    store.getState().player.isPlaying === true && store.getState().player.positionSec === 0
    && store.getState().player.audioUrl === 'https://a.example/ep3.mp3');

  /* 第二层：源码形态断言 —— 钉住「组件确实按 toggle 分支消费该判据、且传对了实参」。
     没有这一层时：
       · 删掉 Timeline.tsx 的守卫（改回无条件 playPodcastEpisode）→ 纯函数断言仍全绿
         （审查 FINDING TASK-081-R2-F1 实测）；
       · 只做「token 在场」检查也不够：把实参 `cur.audioUrl` 改成 `''`，三处 token 一字未动
         却让 toggle 分支变成不可达死代码、缺陷完全复现，而门禁仍全绿
         （审查 FINDING TASK-081-R3-F2 实测）。
     故本层**必须校验实参表达式本身**（`cur.isActive` / `cur.audioUrl` / `audioUrl` 三者的
     具体写法），而不是只查函数名与 'toggle' 字面量在场。
     手法沿用本文件既有的 readFileSync + slice（见 TASK-065 N8/N11 一处）。
     证据边界如实声明：本层是**源码形态**断言（非 DOM 点击），它证明「调用点写了正确的
     判定与实参」，不证明运行期 DOM 点击路径；后者需 CDP e2e（本项目暂未建）。 */
  {
    const fsT = await import('node:fs');
    const tlSrc2 = fsT.readFileSync(new URL('../../src/components/Timeline.tsx', import.meta.url), 'utf8');
    const playStart = tlSrc2.indexOf('const play = () => {');
    const playBlock = playStart < 0 ? '' : tlSrc2.slice(playStart, tlSrc2.indexOf('return (', playStart));
    /* 实参逐个钉死：任何一处被替换（如 cur.audioUrl → ''）都必须失败 */
    const callMatch = playBlock.match(/podcastClickAction\(\s*([^)]*)\)/);
    const args = callMatch ? callMatch[1].split(',').map((a) => a.trim()) : [];
    checkNew('(P3[F3]) 组件按 toggle 分支消费判据（删掉该守卫即失败）',
      playBlock.includes('podcastClickAction(') && playBlock.includes("=== 'toggle'")
      && playBlock.includes('togglePlayerPlay();'));
    checkNew('(P3[F3]) 判据实参必须取当前播放器状态与卡片音频地址（改实参即失败，非仅查 token 在场）',
      args.length === 3 && args[0] === 'cur.isActive' && args[1] === 'cur.audioUrl'
      && args[2] === 'audioUrl');
    checkNew('(P3[F3]) 组件的 toggle 分支必须 return（否则会继续走 play 造成双重动作）',
      /===\s*'toggle'\s*\)\s*\{\s*togglePlayerPlay\(\);\s*return;/.test(playBlock));
  }

  /* ---------- P3[F2]（TASK-086）：单键快捷键让路浮层的判据 ----------
     判据已抽成 src/components/shortcutYield.ts 的纯函数 shouldYieldToOverlay，
     由 App.tsx 的真实 keydown 分支消费，故这里断言的是**组件实际使用的那份判定**。
     此前该判据内联在 App.tsx 的闭包里、零断言（审查 TASK-081-F2 登记的覆盖缺口）。 */
  {
    const { shouldYieldToOverlay, OVERLAY_YIELD_KEYS, OVERLAY_SOURCES, anyOverlayOpen }
      = await import('../../src/components/shortcutYield.ts');
    checkNew('(P3[F2]) 浮层打开 + 单键 S/M/J/K ⇒ 让路（修前会作用到浮层背后的当前文章）',
      ['s', 'S', 'm', 'M', 'j', 'k'].every((k) => shouldYieldToOverlay(true, k, false) === 'yield'));
    checkNew('(P3[F2]) 浮层未打开 + 单键 ⇒ 不让路（快捷键照常生效）',
      ['s', 'S', 'm', 'M', 'j', 'k'].every((k) => shouldYieldToOverlay(false, k, false) === 'proceed'));
    checkNew('(P3[F2]) 浮层打开 + 带 Ctrl/Meta/Alt ⇒ 不让路（组合键属浮层自身操作，不受影响）',
      shouldYieldToOverlay(true, 's', true) === 'proceed'
      && shouldYieldToOverlay(true, 'k', true) === 'proceed');
    checkNew('(P3[F2]) 浮层打开 + 非目标键 ⇒ 不让路（只拦 S/M/J/K，不误伤其它键）',
      shouldYieldToOverlay(true, 'a', false) === 'proceed'
      && shouldYieldToOverlay(true, 'Enter', false) === 'proceed'
      && shouldYieldToOverlay(true, 'Escape', false) === 'proceed');
    checkNew('(P3[F2]) 适配性：让路键集合恰为 s/S/m/M/j/k（新增可让路键须同步本表）',
      OVERLAY_YIELD_KEYS.length === 6 && OVERLAY_YIELD_KEYS.join(',') === 's,S,m,M,j,k');
    /* 浮层集合逐项钉死：这是 TASK-086 审查者指出「文本断言可被绕过」的正面修复。
       此前 overlayOpen 是内联的 `a || b || c`，回归网只能查 token 在场——
       实测把「全屏播放器」从并集里删掉，322 条断言**全绿**（真实回归零告警）。
       改为清单求值后，每个浮层必须单独登记，任何一项被删/被改都会被下面两条拦下。
       OPT-012：清单由 8 项扩为 9 项（补「关闭确认框」closeAsk）——旧 8 项逐项探针
       一条不少地保留，新增 closeAsk 探针与真实 store 字段驱动断言。 */
    const baseOverlay = {
      searchOpen: false, settingsOpen: false, newCategoryModalOpen: false,
      addFeedModalOpen: false, editFeedModalOpen: false, renameCatModalOpen: false,
      lightboxUrl: null, playerExpanded: false, playerActive: false,
      closeAskVisible: false,
    };
    checkNew('(P3[F2]) 浮层清单恰为 9 项且名称稳定（新增/删除浮层必须同步本表；OPT-012 起含 closeAsk）',
      OVERLAY_SOURCES.length === 9
      && OVERLAY_SOURCES.map((o) => o.name).join(',')
        === 'search,settings,newCategory,addFeed,editFeed,renameCat,lightbox,playerExpanded,closeAsk');
    /* 逐项：只打开这一项 ⇒ anyOverlayOpen 必须为 true（漏判/少算任一项即失败） */
    const overlayProbes = [
      ['search', { searchOpen: true }],
      ['settings', { settingsOpen: true }],
      ['newCategory', { newCategoryModalOpen: true }],
      ['addFeed', { addFeedModalOpen: true }],
      ['editFeed', { editFeedModalOpen: true }],
      ['renameCat', { renameCatModalOpen: true }],
      ['lightbox', { lightboxUrl: 'https://example.com/a.png' }],
      ['playerExpanded', { playerExpanded: true, playerActive: true }],
      /* OPT-012 新增：关闭确认框（审计 2026-10-08「关闭确认框与快捷键」） */
      ['closeAsk', { closeAskVisible: true }],
    ];
    const missed = overlayProbes
      .filter(([, patch]) => anyOverlayOpen({ ...baseOverlay, ...patch }) !== true)
      .map(([name]) => name);
    checkNew('(P3[F2]) 每个浮层单独打开都必须被判为「浮层打开」（漏判任一项即失败，输出缺项名；含 OPT-012 的 closeAsk）',
      missed.length === 0);
    checkNew('(P3[F2]) 无浮层时 anyOverlayOpen 为 false（不误判为打开，否则快捷键全被吞）',
      anyOverlayOpen(baseOverlay) === false);
    /* 全屏播放器是**条件**浮层：仅在播放器激活时才算打开 */
    checkNew('(P3[F2]) playerExpanded 仅在 playerActive 时算浮层（未激活时不算，避免吞掉快捷键）',
      anyOverlayOpen({ ...baseOverlay, playerExpanded: true, playerActive: false }) === false
      && anyOverlayOpen({ ...baseOverlay, playerExpanded: true, playerActive: true }) === true);
    /* 空字符串 lightboxUrl 等同于「无 lightbox」（防 falsy 误判） */
    checkNew('(P3[F2]) lightboxUrl 为空串不算浮层（falsy 边界）',
      anyOverlayOpen({ ...baseOverlay, lightboxUrl: '' }) === false);

    /* OPT-012：关闭确认框的真实 store 字段驱动（非仅清单 token）——
       closeAskVisible=true 时必须判为浮层打开，且 s/m/j/k 全部让路。
       修前（探针 probe-before.mjs 实测）：anyOverlayOpen=false → 四个单键全部 proceed，
       S/M 会改到确认框背后的文章、J/K 会在背后换文章。 */
    store.setState({ closeAskVisible: true });
    const sCloseAsk = store.getState();
    const overlayOpenWithCloseAsk = anyOverlayOpen({
      searchOpen: sCloseAsk.searchOpen,
      settingsOpen: sCloseAsk.settingsOpen,
      newCategoryModalOpen: sCloseAsk.newCategoryModalOpen,
      addFeedModalOpen: sCloseAsk.addFeedModalOpen,
      editFeedModalOpen: sCloseAsk.editFeedModalOpen,
      renameCatModalOpen: sCloseAsk.renameCatModalOpen,
      lightboxUrl: sCloseAsk.lightboxUrl,
      playerExpanded: sCloseAsk.playerExpanded,
      playerActive: sCloseAsk.player.isActive,
      closeAskVisible: sCloseAsk.closeAskVisible,
    });
    checkNew('(P3[F2]/OPT-012) 真实 store：关闭确认框打开（closeAskVisible=true）⇒ anyOverlayOpen=true 且 s/m/j/k 全部让路（修前 proceed：单键作用到确认框背后的文章）',
      overlayOpenWithCloseAsk === true
      && ['s', 'S', 'm', 'M', 'j', 'k'].every((k) => shouldYieldToOverlay(overlayOpenWithCloseAsk, k, false) === 'yield'));
    checkNew('(P3[F2]/OPT-012) 关闭确认框不吞组合键/非目标键（Ctrl+S 等照旧 proceed；Escape 不在让路集合，保持 App.tsx 专门分支语义）',
      shouldYieldToOverlay(overlayOpenWithCloseAsk, 's', true) === 'proceed'
      && shouldYieldToOverlay(overlayOpenWithCloseAsk, 'Escape', false) === 'proceed'
      && shouldYieldToOverlay(overlayOpenWithCloseAsk, 'a', false) === 'proceed');
    store.setState({ closeAskVisible: false });

    /* 修前对照：旧行为完全不看浮层 → 浮层打开时同集键也照旧执行（不让路）。 */
    const legacyYield = () => 'proceed';
    checkNew('(P3[F2]) 修前判据可复现：不看浮层状态时「浮层打开 + S」也不会让路（缺陷根因）',
      legacyYield() === 'proceed' && shouldYieldToOverlay(true, 's', false) !== 'proceed');

    /* 源码形态断言：钉住 App.tsx 的 keydown **确实调用该判据**——纯函数有牙
       ≠ 调用方真的用它（TASK-081-R2-F1 的同类教训）。 */
    const fsS = await import('node:fs');
    const appSrc = fsS.readFileSync(new URL('../../src/App.tsx', import.meta.url), 'utf8');
    const yieldAt = appSrc.indexOf('shouldYieldToOverlay(');
    const yieldBlock = yieldAt < 0 ? '' : appSrc.slice(yieldAt, yieldAt + 200);
    checkNew('(P3[F2]) App.tsx 的 keydown 消费该判据且按 yield 返回（删掉该分支即失败）',
      yieldBlock.includes('shouldYieldToOverlay(') && yieldBlock.includes("=== 'yield'")
      && /===\s*'yield'\s*\)\s*\{\s*return;/.test(yieldBlock));
    checkNew('(P3[F2]) 判据实参为 overlayOpen + 按键 + 修饰键（改实参即失败）',
      /shouldYieldToOverlay\(\s*overlayOpen\s*,\s*e\.key\s*,\s*e\.ctrlKey\s*\|\|\s*e\.metaKey\s*\|\|\s*e\.altKey\s*\)/
        .test(appSrc));
    /* overlayOpen 必须由 anyOverlayOpen 求值（而非退回内联并集）。
       内联写法下「少判一个浮层」无法被断言——见上面清单断言的说明。 */
    checkNew('(P3[F2]) App.tsx 的 overlayOpen 必须由 anyOverlayOpen 求值（退回内联并集即失败）',
      /const\s+overlayOpen\s*=\s*anyOverlayOpen\(/.test(appSrc));
    /* OPT-012 接线：查询实参必须带 closeAskVisible（清单登记但调用点不传 = 漏判依旧）。
       Esc 链对 closeAskVisible 的专门处理由 t100-u7 锁定，此处不动其语义。 */
    checkNew('(P3[F2]/OPT-012) App.tsx 的浮层查询实参传入 closeAskVisible（清单已登记但调用点漏传即失败）',
      /closeAskVisible:\s*s\.closeAskVisible/.test(appSrc));
  }
  store.setState({ player: { ...store.getState().player, speed: 2.0 } });
  store.getState().cyclePlaybackSpeed();
  checkNew('(j) 倍速在 1/1.25/1.5/2 内循环（2.0 → 1.0）并给出 toast',
    store.getState().player.speed === 1 && store.getState().toasts.some((t) => t.text === '倍速已切换至 1x'));
  store.setState({ playerExpanded: true });
  store.getState().closePodcastBar();
  checkNew('(j) 关闭播放条：停止并收起（isActive/isPlaying/seek 复位 + 大播放器收起）',
    store.getState().player.isActive === false && store.getState().player.isPlaying === false
    && store.getState().player.seekToSec === null && store.getState().playerExpanded === false);

  /* ---------- P3[F5]（REQ-102）：滚动出视口标已读，不得把「换序列的异步窗口」误算 ----------
     缺陷（AUDIT-20260919-v2 F5）：切换 布局/视图/排序 换掉 items 并触发 scrollTo(top:0)，
     但归零异步生效；期间滚动效应读到**旧** startIndex，就把新序列前段（用户没见过）
     整段标成已读。列表越长越容易命中，故表现为小概率。
     判据已抽为纯函数 scrollAwayRange（src/components/scrollAwayRead.ts）。 */
  {
    const { scrollAwayRange } = await import('../../src/components/scrollAwayRead.ts');
    const N = 200; // 模拟长列表：旧 startIndex=120，新序列 200 条
    /* 修前逻辑（内联比较，不看是否用户滚动）的等价复刻：用于证明旧实现在同一输入下
       确实会误标——不是靠字面量断言，而是真的跑一遍旧算法。 */
    const legacyAwayRange = (startIndex, lastStartIndex) => (
      startIndex > lastStartIndex ? { from: lastStartIndex, to: startIndex } : null);
    checkNew('(P3[F5]) 修前可复现：非用户滚动 + startIndex 从 0 跳到 120 ⇒ 修前逻辑会误标 120 条',
      JSON.stringify(legacyAwayRange(120, 0)) === JSON.stringify({ from: 0, to: 120 })
      && scrollAwayRange({ scrollDriven: false, startIndex: 120, lastStartIndex: 0, itemCount: N })
        .range === null);
    checkNew('(P3[F5]) 非用户滚动（换布局/视图/排序的异步归零窗口）⇒ 绝不标读',
      scrollAwayRange({ scrollDriven: false, startIndex: 120, lastStartIndex: 0, itemCount: N })
        .range === null);
    checkNew('(P3[F5]) 非用户滚动仍须对齐基准（否则窗口关闭后基准停在旧值、后续真滚动会补标一大段）',
      scrollAwayRange({ scrollDriven: false, startIndex: 120, lastStartIndex: 0, itemCount: N })
        .nextLastStartIndex === 120);
    checkNew('(P3[F5]) 真实滚动 + startIndex 递增 ⇒ 标已读区间恰为 [上次基准, 本次 start)',
      JSON.stringify(scrollAwayRange({ scrollDriven: true, startIndex: 40, lastStartIndex: 25, itemCount: N }).range)
        === JSON.stringify({ from: 25, to: 40 }));
    checkNew('(P3[F5]) 真实滚动但未超过基准（往回滚/抖动）⇒ 不标读，且基准跟随下行不锁定',
      scrollAwayRange({ scrollDriven: true, startIndex: 10, lastStartIndex: 25, itemCount: N }).range === null
      && scrollAwayRange({ scrollDriven: true, startIndex: 10, lastStartIndex: 25, itemCount: N })
        .nextLastStartIndex === 10);
    checkNew('(P3[F5]) 起始基准 0 + 真实滚动 ⇒ 从第 0 条起标（首屏滚出正常生效，未被误伤）',
      JSON.stringify(scrollAwayRange({ scrollDriven: true, startIndex: 15, lastStartIndex: 0, itemCount: N }).range)
        === JSON.stringify({ from: 0, to: 15 }));
    checkNew('(P3[F5]) 越界夹取：startIndex 超过 itemCount 时不得越界标读（数据切换瞬间的防御）',
      JSON.stringify(scrollAwayRange({ scrollDriven: true, startIndex: 999, lastStartIndex: 5, itemCount: 50 }).range)
        === JSON.stringify({ from: 5, to: 50 })
      && scrollAwayRange({ scrollDriven: true, startIndex: -3, lastStartIndex: 0, itemCount: 50 }).range === null);
    checkNew('(P3[F5]) 空列表（itemCount=0）恒不标读（避免对 undefined 条目取值）',
      scrollAwayRange({ scrollDriven: true, startIndex: 5, lastStartIndex: 0, itemCount: 0 }).range === null
      && scrollAwayRange({ scrollDriven: true, startIndex: 5, lastStartIndex: 0, itemCount: 0 })
        .nextLastStartIndex === 0);

    /* 源码形态：Timeline 的滚动效应必须消费该判据，且筛选变化时重置基准。 */
    const fsF5 = await import('node:fs');
    const tlF5 = fsF5.readFileSync(new URL('../../src/components/Timeline.tsx', import.meta.url), 'utf8');
    checkNew('(P3[F5]) Timeline 的滚动效应必须消费 scrollAwayRange（改回手写比较即失败）',
      /scrollAwayRange\(\{/.test(tlF5) && tlF5.includes('lastStartIndexRef.current = nextLastStartIndex'));
    checkNew('(P3[F5]) 筛选上下文变化必须重置 startIndex 基准（filterKey 依赖）',
      /useLayoutEffect\(\(\)\s*=>\s*\{[^}]*lastStartIndexRef\.current\s*=\s*0[^}]*\}\s*,\s*\[filterKey\]\)/
        .test(tlF5));
    checkNew('(P3[F5]) scrollDriven 只能由用户滚动置位（handleScroll 内经 isUserScrollEvent 判定；fix-2 更新：scroll 事件不再无条件算用户滚动）',
      /const handleScroll = \(\) => \{[^]*?scrollDrivenRef\.current = scrollDrivenRef\.current \|\| isUserScrollEvent\(\{/.test(tlF5)
      && !/scrollDrivenRef\.current = true;/.test(tlF5));
  }
}
