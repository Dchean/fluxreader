import react from '@vitejs/plugin-react'
import { defineConfig } from 'vite'

// https://vite.dev/config/
export default defineConfig({
  plugins: [react()],
  server: {
    // 固定端口 + 全协议监听：Tauri devUrl 写死 5173（tauri.conf.json），
    // WebView2 解析 localhost 可能走 IPv4/IPv6 任一栈，strictPort 防止端口被占后静默漂移
    port: 5173,
    strictPort: true,
    host: true,
    watch: {
      // Windows + Tauri：cargo target 目录含数万构建产物且 .exe 文件被锁，
      // Vite watcher 会以 EBUSY 崩溃，必须整体忽略
      ignored: ['**/src-tauri/target/**'],
    },
  },
  // Tauri 窗口内固定使用系统字体渲染，无需浏览器兼容性降级
  build: {
    target: 'chrome120',
  },
})
