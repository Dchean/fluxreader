// 正文图片代理（防盗链兼容）——参考 Papr 方案。
// webview 的 Referer 无法按域名变化：黑名单式防盗链（sinaimg.cn 拒外来 Referer）
// 与白名单式（少数派 cdnfile.sspai.com 要求 sspai.com Referer）无法用单一
// referrerpolicy 同时满足。故对已知白名单式防盗链域名，走后端 fetch_image
// （Referer 候选链）拿 bytes 转 data: URL 替换 src。
import { api } from './api';

/** 需要走后端代理的图床域名（白名单式防盗链：要求特定 Referer）。
    少数派 cdnfile/rssfile 是典型；后续遇到同类站点在此追加。 */
const PROXY_HOSTS = new Set(['cdnfile.sspai.com', 'rssfile.sspai.com']);

/** 判断某图片 URL 是否需要后端代理。 */
export function needsImageProxy(src: string): boolean {
  try {
    const host = new URL(src).hostname.toLowerCase();
    return PROXY_HOSTS.has(host);
  } catch {
    return false;
  }
}

/** 图片字节 → data: URL。Tauri IPC 的 Vec<u8> 在 JS 端是 number[]，需先转 Uint8Array；
    MIME 用字节嗅探（少数派等 CDN URL 带 imageView2 等查询参数，扩展名判断失效）。 */
function imageDataUrl(src: string, bytes: Uint8Array | number[]): string {
  const arr = bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes);
  let mime = sniffImageMime(arr) ?? 'image/jpeg';
  const path = src.split(/[?#]/)[0]?.toLowerCase() ?? '';
  if (!sniffImageMime(arr) && path.endsWith('.png')) mime = 'image/png';
  // 二进制 → base64（分块避免大图撑爆调用栈）
  let binary = '';
  const chunk = 0x8000;
  for (let i = 0; i < arr.length; i += chunk) {
    binary += String.fromCharCode(...arr.subarray(i, i + chunk));
  }
  return `data:${mime};base64,${btoa(binary)}`;
}

/** 字节签名嗅探 MIME（PNG/JPEG/GIF/WebP），未知返回 null。 */
function sniffImageMime(b: Uint8Array): string | null {
  if (b.length >= 4 && b[0] === 0x89 && b[1] === 0x50 && b[2] === 0x4e && b[3] === 0x47) return 'image/png';
  if (b.length >= 3 && b[0] === 0xff && b[1] === 0xd8 && b[2] === 0xff) return 'image/jpeg';
  if (b.length >= 3 && b[0] === 0x47 && b[1] === 0x49 && b[2] === 0x46) return 'image/gif';
  if (b.length >= 12 && b[0] === 0x52 && b[1] === 0x49 && b[2] === 0x46 && b[3] === 0x46 && b[8] === 0x57 && b[9] === 0x45 && b[10] === 0x42 && b[11] === 0x50) return 'image/webp';
  return null;
}

/** 把 HTML 里需要代理的 <img> 替换为 data: URL（后端抓取的字节）。
    失败（后端也拿不到）则隐藏该图。返回替换后的 HTML；无需要代理的图则原样返回。
    浏览器 mock 环境（无 IPC）返回 null，调用方跳过代理。 */
export async function proxyImagesInHtml(
  html: string,
  pageUrl?: string,
): Promise<string | null> {
  if (typeof window === 'undefined' || !('__TAURI_INTERNALS__' in window)) return null;
  if (!html || !html.toLowerCase().includes('<img')) return html;

  const doc = new DOMParser().parseFromString(html, 'text/html');
  const imgs = Array.from(doc.body.querySelectorAll('img')).filter((img) => {
    const src = img.getAttribute('src') || '';
    return /^https?:\/\//.test(src) && needsImageProxy(src);
  });
  if (imgs.length === 0) return html;

  await Promise.all(
    imgs.map(async (img) => {
      const src = img.getAttribute('src') || '';
      try {
        const bytes = await api.fetchImage(src, pageUrl);
        if (!bytes || bytes.length === 0) {
          img.style.display = 'none';
          return;
        }
        img.setAttribute('src', imageDataUrl(src, bytes));
        img.removeAttribute('srcset');
        img.removeAttribute('referrerpolicy');
      } catch {
        img.style.display = 'none';
      }
    }),
  );
  return doc.body.innerHTML;
}
