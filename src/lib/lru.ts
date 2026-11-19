/** Map 按插入顺序迭代。调用方先把最近使用的键移到末尾。 */
export function evictOldestEntries<V>(map: Map<string, V>, max: number): void {
  while (map.size > max) {
    const oldest = map.keys().next();
    if (oldest.done) break;
    map.delete(oldest.value);
  }
}
