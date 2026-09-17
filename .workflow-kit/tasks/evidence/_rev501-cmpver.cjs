// 独立实测：compareVersions(remote, '') 到底返回什么？
function compareVersions(a, b) {
  const pa = a.split('.').map(Number);
  const pb = b.split('.').map(Number);
  for (let i = 0; i < Math.max(pa.length, pb.length); i++) {
    const d = (pa[i] ?? 0) - (pb[i] ?? 0);
    if (d !== 0) return d;
  }
  return 0;
}
function isComparableVersion(v) {
  const s = v.trim();
  return /^\d+(\.\d+)*$/.test(s);
}
function shouldOfferUpdate(remote, local) {
  if (!isComparableVersion(remote) || !isComparableVersion(local)) return false;
  return compareVersions(remote.trim(), local.trim()) > 0;
}

console.log("compareVersions('9.9.9', '') =", compareVersions('9.9.9', ''));
console.log("Number.isNaN(that) =", Number.isNaN(compareVersions('9.9.9', '')));
console.log("'' .split('.') =", JSON.stringify(''.split('.')));
console.log("'' .split('.').map(Number) =", JSON.stringify(''.split('.').map(Number)));
console.log("Number('') =", Number(''));
console.log("=> NaN claim (compareVersions returns NaN) is:", Number.isNaN(compareVersions('9.9.9', '')) ? 'CORRECT' : 'FALSE');
console.log("shouldOfferUpdate('9.9.9','') =", shouldOfferUpdate('9.9.9', ''));
console.log("=> pre-fix 'always says update' behavior reproducible via raw compareVersions:", compareVersions('9.9.9', '') > 0);
