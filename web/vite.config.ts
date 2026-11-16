import { fileURLToPath } from 'node:url'

import tailwindcss from '@tailwindcss/vite'
import react from '@vitejs/plugin-react'
import { loadEnv } from 'vite'
import { defineConfig } from 'vitest/config'

// 开发时 API 不在同一个地址上：把 /api 代理到 controller 的管理端口。
// 地址写在环境变量 I3T_CONTROLLER 里（也可以写进 .env.local），默认是 controller 的默认管理地址。
const DEFAULT_CONTROLLER = 'http://127.0.0.1:20332'

export default defineConfig(({ mode }) => {
  const env = loadEnv(mode, process.cwd(), 'I3T_')
  const controller = process.env.I3T_CONTROLLER ?? env.I3T_CONTROLLER ?? DEFAULT_CONTROLLER

  return {
    plugins: [react(), tailwindcss()],
    resolve: {
      alias: { '@': fileURLToPath(new URL('./src', import.meta.url)) },
    },
    server: {
      proxy: {
        '/api': { target: controller, changeOrigin: true },
      },
    },
    build: {
      outDir: 'dist',
      sourcemap: false,
      // 关系图的库单独成块，登录页和多数页面用不到它。
      chunkSizeWarningLimit: 700,
    },
    test: {
      environment: 'jsdom',
      globals: true,
      setupFiles: ['./src/test/setup.ts'],
      include: ['src/**/*.test.{ts,tsx}'],
      css: false,
      restoreMocks: true,
      unstubGlobals: true,
    },
  }
})
