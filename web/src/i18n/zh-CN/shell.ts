/** 外壳：登录、导航、网络切换器、计划中的页面。 */
export const signIn = {
  title: '登录 ice3tier',
  lead: '用管理令牌登录。controller 第一次启动时把它打印在终端里，只显示那一次。',
  tokenLabel: '管理令牌',
  tokenPlaceholder: '粘贴管理令牌',
  show: '显示令牌',
  hide: '隐藏令牌',
  remember: '在这台电脑上保持登录',
  rememberHint: '不勾的话，关掉这个标签页就退出。',
  submit: '登录',
  empty: '请填管理令牌。',
  expired: '登录已失效，请重新登录。',
  signedOut: '已退出登录。',
  lostTitle: '令牌丢了？',
  lostBody:
    '停掉 controller，用平时启动它的那组参数再加上 --reset-admin-token 执行一次。它会打印一个新令牌然后退出，原来的随即作废。',
  lostCommand: 'i3t-controller --data-dir <数据目录> --public-addr <主机:端口> --reset-admin-token',
  tagline: '把自己的几台机器连成一个网',
}

export const nav = {
  label: '主导航',
  overview: '总览',
  nodes: '节点',
  access: '访问控制',
  groupAccess: '接入',
  exits: '出口',
  listeners: '监听器',
  credentials: '接入凭据',
  forwards: '端口转发',
  groupDiagnose: '诊断',
  topology: '拓扑',
  diagnosis: '路径诊断',
  tests: '连通性测试',
  tokens: '注册',
  pairs: '节点对',
  network: '网络',
  settings: '设置',
  collapse: '收起侧栏',
  expand: '展开侧栏',
  toggleGroup: (name: string, open: boolean) => `${open ? '收起' : '展开'}「${name}」`,
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
  manage: '管理网络',
  heading: '网络',
  switched: (name: string) => `已切换到网络 ${name}`,
}

export const planned = {
  badge: '计划中',
  note: '这个页面需要的接口 controller 还没有提供，所以这里现在没有数据，也不会用假数据占位。',
  meanwhile: '在那之前',
  tests: {
    title: '连通性测试',
    summary:
      '由 controller 对一对节点发起测试，回答能不能直连、直连和经 relay 各有多快；也可以对一批节点两两测，并保留历史。',
    needs: '缺的接口：发起测试、读取测试结果。',
    meanwhile: '两个节点之间现在怎么走、多快，可以在「路径诊断」里现问；全网的情况在「拓扑」里。',
  },
}
