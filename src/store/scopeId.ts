// Note: 单点解析的收敛与依赖方向（叶子模块、selectors re-export / internals import） — 见 .agents/notes/implemented/simplification/2026-10-16-零消费入口清理与id解析单点.md
/* ============================================================
   OPT-017F：字符串 id → 后端数字 id 的单点解析

   从 store 的字符串 id（纯数字 / 'cat-' / 'feed-' 前缀两种形态）提取后端
   数字 id：取**第一段连续数字**，无数字段返回 NaN。

   历史 bug：mock 用 'feed-'+Date.now() 前缀、真实数据用 String(row.id)
   纯数字，定长 slice(2)/slice(4) 会把 '26'.slice(2) 截成 ''→NaN，布局/AI
   开关等全部写库失败且被乐观更新掩盖（7 类模式 C 契约断裂变体）。

   依赖方向（收敛取舍）：本模块是**叶子**（零 import）。selectors.numericId
   （公共导出保留，re-export 本实现）与 internals 的 scope 范围派生都消费
   这里。internals 曾以「避免 internals ↔ selectors 循环依赖」为由维护同名
   私有副本（逐字相同，见 f75c512）；收敛时核对导入图：selectors 不（传递）
   导入 internals，今天的直接 import 不成环——但共享叶模块让双方都只依赖
   叶子，既不把基础设施层耦合到选择器层，也消除未来任一侧新增依赖即成环的
   路径。两份定义就此合一（单点性由回归域 scope-id 的源级守卫锁定）。
   ============================================================ */
export function numericId(id: string): number {
  const m = id.match(/(\d+)/);
  return m ? Number(m[1]) : NaN;
}
