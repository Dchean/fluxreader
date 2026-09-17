// 独立复现 L1 复杂度改造的性能与语义等价
// 旧：unread = ids.filter(id => entries.find(x => x.id === id) ...)  → O(n*m)
// 新：先建 Map<id, entry>，再 filter → O(n+m)
function oldImpl(entries, ids) {
  return ids.filter((id) => {
    const e = entries.find((x) => x.id === id);
    return e ? !e.isRead : false;
  });
}
function newImpl(entries, ids) {
  const byId = new Map(entries.map((e) => [e.id, e]));
  return ids.filter((id) => {
    const e = byId.get(id);
    return e ? !e.isRead : false;
  });
}

const N = 20000; // entries
const M = 5000;  // ids
const entries = [];
for (let i = 0; i < N; i++) entries.push({ id: String(i), isRead: i % 3 === 0 });
const ids = [];
for (let i = 0; i < M; i++) ids.push(String((i * 7) % N));
ids.push('not-exist-1', 'not-exist-2'); // 未知 id 必须被忽略

// warm
oldImpl(entries.slice(0, 100), ids.slice(0, 10));
newImpl(entries.slice(0, 100), ids.slice(0, 10));

let t0 = process.hrtime.bigint();
const ro = oldImpl(entries, ids);
let t1 = process.hrtime.bigint();
const rn = newImpl(entries, ids);
let t2 = process.hrtime.bigint();

const oldMs = Number(t1 - t0) / 1e6;
const newMs = Number(t2 - t1) / 1e6;
console.log(`n=${N} m=${ids.length}`);
console.log(`OLD (entries.find per id): ${oldMs.toFixed(2)} ms  -> ${ro.length} ids`);
console.log(`NEW (Map index)          : ${newMs.toFixed(2)} ms  -> ${rn.length} ids`);
console.log(`speedup: ${(oldMs / newMs).toFixed(1)}x`);
console.log(`SEMANTIC EQUIVALENCE (same set, same order):`, JSON.stringify(ro) === JSON.stringify(rn));
