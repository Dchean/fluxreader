/** semver 比较：返回 >0 表示 a 更新 */
export function compareVersions(a: string, b: string): number {
  const pa = a.split('.').map(Number);
  const pb = b.split('.').map(Number);
  for (let i = 0; i < Math.max(pa.length, pb.length); i++) {
    const d = (pa[i] ?? 0) - (pb[i] ?? 0);
    if (d !== 0) return d;
  }
  return 0;
}

/** 版本号是否可用于比较（纯数字点分，至少一段）。
    空串表示「本地版本尚未就绪」（getVersion 还在途或失败）：compareVersions
    对空串会把它当 0.0.0，于是任何远端版本都「更新」——这正是 P2-7 的误判来源。 */
export function isComparableVersion(v: string): boolean {
  const s = v.trim();
  return /^\d+(\.\d+)*$/.test(s);
}

/** 「是否有可用更新」的判定收口：只有两端版本号都可比时才比较，
    否则一律返回 false（未知 ≠ 有更新）。调用方对 false 且本地不可比的情况
    应给出「无法确定本地版本」而不是「已是最新」。 */
export function shouldOfferUpdate(remote: string, local: string): boolean {
  if (!isComparableVersion(remote) || !isComparableVersion(local)) return false;
  return compareVersions(remote.trim(), local.trim()) > 0;
}
