import type { ArticleEntry, CategoryGroup } from './types';

/* 演示数据 —— 浏览器环境（无 Tauri IPC）的回退数据源，以真实客户端的
   数据形态提供（时间戳 / feedId 归属 / tags 数组）。

   条目为单一扁平集合：条目属于哪个内容布局由
   「订阅源布局绑定 → 分类布局」在查询时动态解析，
   修改绑定后条目即时跟随，无需数据迁移。 */

const H = 3_600_000;
const MIN = 60_000;
const DAY = 86_400_000;
/** 以模块加载时刻为"现在"生成时间戳，保证"今天/昨天"判定与展示稳定 */
const NOW = Date.now();

export function createInitialCategories(): CategoryGroup[] {
  return [
    {
      id: 'cat-tech',
      name: '技术开发',
      collapsed: false,
      settingsCollapsed: false,
      layout: 'article',
      autoSummary: true,
      autoTranslate: false,
      feeds: [
        { id: 'f-openai', name: 'OpenAI Blog', url: 'https://openai.com/news/rss.xml', favicon: 'https://openai.com/favicon.ico', layout: 'inherit', autoSummary: true, autoTranslate: false },
        { id: 'f-rust', name: 'Rust Official Blog', url: 'https://blog.rust-lang.org/feed.xml', favicon: 'https://www.rust-lang.org/static/images/favicon.ico', layout: 'inherit', autoSummary: false, autoTranslate: false },
        { id: 'f-ars', name: 'Ars Technica', url: 'https://feeds.arstechnica.com/arstechnica/index', favicon: 'https://arstechnica.com/favicon.ico', layout: 'inherit', autoSummary: false, autoTranslate: false },
      ],
    },
    {
      id: 'cat-social',
      name: '独立社交',
      collapsed: false,
      settingsCollapsed: false,
      layout: 'social',
      autoSummary: false,
      autoTranslate: true,
      feeds: [
        { id: 'f-sama', name: 'Sam Altman (@sama)', url: 'https://rss.social/sama', favicon: '', layout: 'inherit', autoSummary: false, autoTranslate: true },
        { id: 'f-karpathy', name: 'Andrej Karpathy', url: 'https://rss.social/karpathy', favicon: '', layout: 'inherit', autoSummary: false, autoTranslate: false },
      ],
    },
    {
      id: 'cat-design',
      name: '摄影画廊',
      collapsed: false,
      settingsCollapsed: false,
      layout: 'image',
      autoSummary: false,
      autoTranslate: false,
      feeds: [
        { id: 'f-nasa', name: 'NASA APOD', url: 'https://apod.nasa.gov/apod.rss', favicon: 'https://www.nasa.gov/favicon.ico', layout: 'inherit', autoSummary: false, autoTranslate: false },
        { id: 'f-unsplash', name: 'Unsplash Arch', url: 'https://unsplash.com/rss', favicon: 'https://unsplash.com/favicon.ico', layout: 'inherit', autoSummary: false, autoTranslate: false },
      ],
    },
    {
      id: 'cat-podcast',
      name: '精选播客',
      collapsed: false,
      settingsCollapsed: false,
      layout: 'podcast',
      autoSummary: false,
      autoTranslate: false,
      feeds: [
        { id: 'f-hardfork', name: 'Hard Fork Podcast', url: 'https://feeds.simplecast.com/hardfork', favicon: '', layout: 'inherit', autoSummary: false, autoTranslate: false },
      ],
    },
    {
      id: 'cat-notif',
      name: '服务通知',
      collapsed: false,
      settingsCollapsed: false,
      layout: 'notification',
      autoSummary: true,
      autoTranslate: false,
      feeds: [
        { id: 'f-github', name: 'GitHub Releases', url: 'https://github.com/releases.atom', favicon: 'https://github.githubassets.com/favicons/favicon.png', layout: 'inherit', autoSummary: true, autoTranslate: false },
      ],
    },
  ];
}

export function createInitialEntries(): ArticleEntry[] {
  return [
    {
      id: 'art-1', feedId: 'f-openai',
      title: 'DeepSeek-V3 架构解析与多头潜在注意力机制探讨',
      snippet: '现代大语言模型推理阶段的核心瓶颈往往不在算力，而在于高并发与长上下文下的内存带宽限制。本文深度探讨 MLA 机制如何减少 KV 缓存显存占用...',
      publishedAt: NOW - 2 * H,
      source: 'direct', isRead: false, isStarred: false, tags: ['人工智能'],
      author: 'Yann LeCun',
      cover: 'https://picsum.photos/seed/deepseek/160/120',
      aiSummary: '文章深入剖析了 MLA（Multi-head Latent Attention）多头潜在注意力机制，展示其如何将 KV 缓存压缩至原始大小的数分之一，极大缓解了长上下文端侧推理的内存墙瓶颈。',
      content: '<p>现代大语言模型推理阶段的核心瓶颈往往不在算力，而在于高并发与长上下文下的内存带宽限制。</p><p>在 FluxReader 的设计中，Miniflux 承担了订阅关系与状态同步中继，而 SQLite 与 Rust 引擎则完整承载了离线全文索引、正文媒体增强与 AI 摘要持久化。</p><p>通过 MLA 算法，客户端能够在端侧大幅压缩缓存体积，实现毫秒级快速索引与响应。</p>',
      rawContent: '<p><b>[RSS Raw Output]</b> The primary bottleneck in inference is memory bandwidth. Multi-head Latent Attention (MLA) effectively compresses KV cache. Full local-first implementation tested with SQLite and Rust FFI layer.</p>',
      translatedContent: '<p>现代大语言模型推理阶段的核心瓶颈往往不在算力，而在于高并发与长上下文下的内存带宽限制。</p><p>在 FluxReader 的设计中，Miniflux 承担了订阅关系与状态同步中继，而 SQLite 与 Rust 引擎则完整承载了离线全文索引、正文媒体增强与 AI 摘要持久化。</p><p>通过 MLA 算法，客户端能够在端侧大幅压缩缓存体积，实现毫秒级快速索引与响应。</p>',
    },
    {
      id: 'art-2', feedId: 'f-rust',
      title: 'Rust 2024 Edition 核心特性预览与内存安全演进',
      snippet: 'Rust 2024 版本带来了全新的 RPITIT 语法支持、异步闭包以及更严格的生命周期推导规则，进一步巩固系统级软件的高性能底座...',
      publishedAt: NOW - 5 * H,
      source: 'direct', isRead: true, isStarred: true, tags: ['系统开发'],
      author: 'Niko Matsakis',
      aiSummary: 'Rust 2024 版本在语言人体工程学与异步生态上迈出关键一步，使编写高性能桌面应用更加健壮高效。',
      content: '<p>Rust 2024 版本带来了期待已久的特性更新，为桌面引擎开发提供了更加优雅的异步流处理方案。</p><p>RPITIT 原生返回 impl Trait 避免了堆分配，极大地简化了 Tauri 2 底层异步通道设计。</p>',
      rawContent: '<p><b>[RSS Raw Output]</b> Rust 2024 Release Preview: RPITIT support, async closures, and refined borrow checker semantics announced for stable channel.</p>',
      translatedContent: '<p>Rust 2024 版本带来了期待已久的特性更新，为桌面引擎开发提供了更加优雅的异步流处理方案。</p><p>RPITIT 原生返回 impl Trait 避免了堆分配，极大地简化了 Tauri 2 底层异步通道设计。</p>',
    },
    {
      id: 'art-3', feedId: 'f-ars',
      title: 'Windows 11 Mica 材质与 Fluent 2 在桌面应用中的渲染优化实践',
      snippet: '如何在使用 Tauri 2 与 Rust 桌面封装时，实现接近系统原生的亚克力半透明与 Mica 材质性能优化...',
      publishedAt: NOW - 26 * H,
      source: 'direct', isRead: false, isStarred: false, tags: ['桌面开发'],
      author: 'Andrew Cunningham',
      cover: 'https://picsum.photos/seed/windows11/160/120',
      aiSummary: '探讨了跨平台桌面框架如何利用 Windows 11 DWM 特性呈现原汁原味的 Mica 亚克力视觉质感，且不显著消耗额外 GPU 算力。',
      content: '<p>Windows 11 引入的 Mica 材质从桌面壁纸中采样低饱和度颜色，并在不增加 GPU 额外负载的前提下提供优雅视觉层次。</p><p>在轻量级桌面客户端开发中，利用原生亚克力通道能让软件更沉浸地融入系统整体交互。</p>',
      rawContent: '<p><b>[RSS Raw Output]</b> Deep dive into Windows 11 DWM acrylic and Mica composition shaders using native HWND handles.</p>',
      translatedContent: '<p>Windows 11 引入的 Mica 材质从桌面壁纸中采样低饱和度颜色，并在不增加 GPU 额外负载的前提下提供优雅视觉层次。</p><p>在轻量级桌面客户端开发中，利用原生亚克力通道能让软件更沉浸地融入系统整体交互。</p>',
    },
    {
      id: 'soc-1', feedId: 'f-sama',
      title: '端侧 4-bit 量化模型将重新定义个人知识聚合',
      snippet: 'Local-first AI models running on edge devices with 4-bit quantization will redefine personal knowledge aggregation. Fast, private, and always available.',
      publishedAt: NOW - 15 * MIN,
      source: 'direct', isRead: false, isStarred: false, tags: [],
      author: 'Sam Altman',
      content: 'Local-first AI models running on edge devices with 4-bit quantization will redefine personal knowledge aggregation. Fast, private, and always available.',
      rawContent: '', aiSummary: '',
      translatedContent: '运行在边缘端侧设备上的 4-bit 量化本地优先 AI 模型将彻底重新定义个人知识聚合。快速、隐私且始终可用。',
    },
    {
      id: 'soc-2', feedId: 'f-karpathy',
      title: 'Web UI + Rust Core 是构建桌面小工具的绝配',
      snippet: 'Building small, focused desktop utilities that do one thing fast and completely offline is extremely satisfying. Web UI + Rust Core is a lethal combo.',
      publishedAt: NOW - 1 * H,
      source: 'direct', isRead: true, isStarred: true, tags: [],
      author: 'Andrej Karpathy',
      content: 'Building small, focused desktop utilities that do one thing fast and completely offline is extremely satisfying. Web UI + Rust Core is a lethal combo.',
      rawContent: '', aiSummary: '',
      translatedContent: '构建小巧、专注、纯离线且极速的桌面小工具是令人极其满足的。Web 前端 UI + Rust 核心简直是绝配组合。',
    },
    {
      id: 'img-1', feedId: 'f-nasa',
      title: '詹姆斯·韦伯望远镜深空红外星云',
      publishedAt: NOW - 3 * H,
      source: 'direct', isRead: false, isStarred: false, tags: [],
      imageUrl: 'https://picsum.photos/seed/nebula/600/800',
      snippet: '', author: '',
      content: '', rawContent: '', translatedContent: '', aiSummary: '',
    },
    {
      id: 'img-2', feedId: 'f-unsplash',
      title: '北欧极简极地建筑摄影',
      publishedAt: NOW - 30 * H,
      source: 'direct', isRead: true, isStarred: true, tags: [],
      imageUrl: 'https://picsum.photos/seed/arch/600/450',
      snippet: '', author: '',
      content: '', rawContent: '', translatedContent: '', aiSummary: '',
    },
    {
      id: 'img-3', feedId: 'f-nasa',
      title: '东京雨夜赛博光影街景',
      publishedAt: NOW - 32 * H,
      source: 'direct', isRead: false, isStarred: false, tags: [],
      imageUrl: 'https://picsum.photos/seed/tokyo/600/750',
      snippet: '', author: '', content: '', rawContent: '', translatedContent: '', aiSummary: '',
    },
    {
      id: 'pod-1', feedId: 'f-hardfork',
      title: 'Hard Fork: AI 的下一个十年与端侧模型落地',
      publishedAt: NOW - 6 * H,
      source: 'direct', isRead: false, isStarred: false, tags: [],
      durationSec: 48 * 60 + 15,
      cover: 'https://picsum.photos/seed/podcast1/120/120',
      snippet: 'Kevin Roose 和 Casey Newton 探讨桌面 AI 助手与本地阅读器如何颠覆传统信息流。',
      author: 'Hard Fork Podcast',
      content: '', rawContent: '', translatedContent: '', aiSummary: '',
    },
    {
      id: 'not-1', feedId: 'f-github',
      title: 'Tauri v2.2.0 正式发布',
      publishedAt: NOW - 4 * H,
      source: 'direct', isRead: false, isStarred: false, tags: [],
      snippet: '修复了 Windows 平台 Mica 材质在窗口失焦时的重绘闪烁问题，提升 Rust IPC 通信吞吐量。同时增强了系统托盘事件传递与多窗口间消息派发机制，解决了在部分 Windows 10 版本中的亚克力着色通道溢出 Bug。',
      author: 'GitHub',
      aiSummary: '修复了 Windows 平台 Mica 闪烁与托盘事件传递问题，显著优化 Rust IPC 吞吐量。',
      translatedContent: 'Fixed Windows Mica repaint flickering on window blur and improved Rust IPC throughput. Enhanced system tray event delivery and multi-window message dispatching mechanisms.',
      content: '', rawContent: '',
    },
    {
      id: 'not-2', feedId: 'f-github',
      title: 'Rust Analyzer 1.86 Stable',
      publishedAt: NOW - 2 * DAY,
      source: 'direct', isRead: true, isStarred: false, tags: [],
      snippet: 'rust-analyzer 新稳定版发布：新版解析器将宏展开吞吐提升 40%，并修复了 proc-macro 服务在 Windows 管道上的阻塞问题。',
      author: 'GitHub',
      aiSummary: '', translatedContent: '', content: '', rawContent: '',
    },
  ];
}
