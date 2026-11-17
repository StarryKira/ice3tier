/** 节点本地声明：三种能力的名字，和"如何声明"的步骤。 */
export const capability = {
  relay: { name: '中继', full: '可作为中继', short: '中继' },
  exit: { name: '出口', full: '可作为出口', short: '出口' },
  forward: { name: '端口转发目标', full: '可作为端口转发目标', short: '转发目标' },
}

export const declare = {
  howTo: '如何声明',
  hide: '收起',
  notDeclared: (node: string) => `${node} 未声明`,
  step1: (node: string) => `在 ${node} 上运行：`,
  step2: '重启 i3t',
  step3: '声明到达后，此处自动变为可用',
  orConfig: '或写入 config.toml',
  waiting: '等待声明…',
  arrived: '已声明，可以启用',
  copyCommand: '复制命令',
}
