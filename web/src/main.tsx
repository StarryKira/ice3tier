import './index.css'

import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import { BrowserRouter } from 'react-router'

import { configureClient } from '@/api/client'
import { reportReachability } from '@/api/connection'
import { createQueryClient } from '@/api/queries'
import { session } from '@/auth/session'
// 亮色还是暗色：模块一载入就定下来，登录页也跟着。
import '@/lib/theme'

import { App } from './App'

const queryClient = createQueryClient()

configureClient({
  getToken: session.token,
  onReachability: reportReachability,
  // 带着令牌的请求被拒绝：令牌作废了。清掉它和缓存里的数据，路由随即把人送去登录页，登录后回到原来的地址。
  onUnauthorized: () => {
    session.signOut('expired')
    queryClient.clear()
  },
})

// 主动退出时也不留上一个人的数据。
session.subscribe(() => {
  if (session.token() === null) queryClient.clear()
})

const root = document.getElementById('root')
if (root === null) throw new Error('index.html 里没有 #root')

createRoot(root).render(
  <StrictMode>
    {/*
      筛选条件、标签页、选中的节点都写在地址里。路由默认把地址的变化当作可以稍后再渲染的"过渡"，
      这样拨一个筛选框要过一两帧才打上勾；这些都是用户直接操作的控件，要立刻响应，所以关掉。
    */}
    <BrowserRouter useTransitions={false}>
      <App client={queryClient} />
    </BrowserRouter>
  </StrictMode>,
)
