/** 外壳：登录、导航、网络切换、计划中的页面。 */
export const signIn = {
  title: '登录 ice3tier',
  tokenLabel: '管理令牌',
  show: '显示令牌',
  hide: '隐藏令牌',
  remember: '保持登录',
  submit: '登录',
  empty: '输入管理令牌',
  expired: '登录已失效',
  signedOut: '已退出登录',
  lostTitle: '令牌丢失？',
  lostBody: '停止 controller，再用下面的参数运行一次。它会打印新令牌，旧令牌作废。',
  lostCommand: 'i3t-controller --data-dir <数据目录> --public-addr <主机:端口> --reset-admin-token',
}

export const nav = {
  label: '主导航',
  overview: '总览',
  nodes: '节点',
  access: '访问控制',
  groupAccess: '接入',
  exits: '出口',
  listeners: '监听器',
  credentials: '凭据',
  forwards: '端口转发',
  groupDiagnose: '诊断',
  topology: '拓扑',
  diagnosis: '路径诊断',
  tests: '连通性测试',
  tokens: '注册令牌',
  pairs: '节点对',
  network: '网络',
  settings: '设置',
  collapse: '收起侧栏',
  expand: '展开侧栏',
  toggleGroup: (name: string, open: boolean) => `${open ? '收起' : '展开'}${name}`,
  account: '管理员',
  accountMenu: '账号菜单',
  signOut: '退出登录',
  skipToContent: '跳到正文',
  breadcrumb: '当前位置',
}

export const switcher = {
  label: '切换网络',
  current: '当前网络',
  nodes: (n: number) => `${n} 个节点`,
  create: '新建网络',
  manage: '网络设置',
  heading: '网络',
  switched: (name: string) => `已切换到 ${name}`,
}

export const planned = {
  badge: '计划中',
  note: 'controller 尚未提供所需接口',
  meanwhile: '目前',
  tests: {
    title: '连通性测试',
    summary: '由 controller 对节点对发起测试：能否直连，直连与经 relay 的延迟，并保留历史。',
    needs: '缺少接口：发起测试、读取结果',
    meanwhile: '单对节点见「路径诊断」，全网见「拓扑」',
  },
}
