import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'

import tailwindcss from '@tailwindcss/vite'
import react from '@vitejs/plugin-react'
import { loadEnv } from 'vite'
import { defineConfig } from 'vitest/config'

// 开发时 API 不在同一个地址上：把 /api 代理到 controller 的管理端口。
// 地址写在环境变量 I3T_CONTROLLER 里（也可以写进 .env.local），默认是 controller 的默认管理地址。
const DEFAULT_CONTROLLER = 'http://127.0.0.1:20332'

/**
 * 界面自己的版本号只有一个来处：package.json 的 version（每次迭代由仓库的脚本改它）。
 * 构建时读出来，作为常量 `__APP_VERSION__` 写进代码里；运行时不去读 package.json。
 */
function packageVersion(): string {
  const manifest: unknown = JSON.parse(readFileSync(new URL('./package.json', import.meta.url), 'utf8'))
  if (
    typeof manifest === 'object' &&
    manifest !== null &&
    'version' in manifest &&
    typeof manifest.version === 'string'
  )
    return manifest.version
  throw new Error('package.json 里没有 version')
}

export default defineConfig(({ mode }) => {
  const env = loadEnv(mode, process.cwd(), 'I3T_')
  const controller = process.env.I3T_CONTROLLER ?? env.I3T_CONTROLLER ?? DEFAULT_CONTROLLER

  return {
    plugins: [react(), tailwindcss()],
    define: { __APP_VERSION__: JSON.stringify(packageVersion()) },
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
