/** 节点本地声明：三种能力的名字，和"怎么声明"的那张操作说明。 */
export const capability = {
  relay: { name: 'relay', full: '可以作为 relay', short: 'relay' },
  exit: { name: '出口', full: '可以作为出口', short: '出口' },
  forward: { name: '端口转发的目标', full: '可以作为端口转发的目标', short: '转发目标' },
}

export const declare = {
  howTo: '怎么声明',
  hide: '收起',
  notDeclared: (node: string, what: string) => `${node} 没有声明${what}，所以这里启用不了。`,
  notDeclaredForward: (node: string) => `${node} 没有声明可以作为端口转发的目标，所以还不能把它选作转发的目标。`,
  why: '声明要在那台机器上做：这样即使 controller 被人控制，也碰不到没有声明的机器。',
  step1: (node: string) => `在 ${node} 上执行：`,
  step2: '重启那台机器上的 i3t，声明在重启之后上报。',
  step3: '回到这里。声明一到，这个开关会自动变为可用，不用刷新。',
  orConfig: '不用命令的话，也可以直接改它的 config.toml，写上这两行，再重启 i3t：',
  waiting: '正在等这台机器上报声明…',
  arrived: (what: string) => `声明已到：现在可以启用${what}了。`,
  copyCommand: '复制命令',
}
