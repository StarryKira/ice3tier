import { describe, expect, it } from 'vitest'

import {
  buildJoinCommand,
  declareCommand,
  declareConfig,
  installCommands,
  joinFlags,
  NO_DECLARATIONS,
  programFor,
  runCommand,
} from './commands'
import { PLATFORMS, RELEASE_BASE } from './product'

const TOKEN = 'i3t-AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE_x-9'

describe('加入网络的命令', () => {
  it('什么都没勾：echo <令牌> | i3t join', () => {
    expect(buildJoinCommand(TOKEN, NO_DECLARATIONS)).toBe(`echo ${TOKEN} | i3t join`)
  })

  it('每一项声明对应一个 --can-* 参数', () => {
    expect(buildJoinCommand(TOKEN, { ...NO_DECLARATIONS, relay: true })).toBe(`echo ${TOKEN} | i3t join --can-relay`)
    expect(buildJoinCommand(TOKEN, { ...NO_DECLARATIONS, exit: true })).toBe(`echo ${TOKEN} | i3t join --can-exit`)
    expect(buildJoinCommand(TOKEN, { ...NO_DECLARATIONS, forward: true })).toBe(
      `echo ${TOKEN} | i3t join --can-forward`,
    )
  })

  it('全勾时参数的顺序固定，和勾选的先后无关', () => {
    const expected = `echo ${TOKEN} | i3t join --can-exit --can-relay --can-forward`
    expect(buildJoinCommand(TOKEN, { exit: true, relay: true, forward: true })).toBe(expected)
    expect(buildJoinCommand(TOKEN, { forward: true, relay: true, exit: true })).toBe(expected)
    expect(joinFlags({ forward: true, relay: false, exit: true })).toEqual(['--can-exit', '--can-forward'])
    expect(joinFlags(NO_DECLARATIONS)).toEqual([])
  })

  it('令牌生成之前，令牌的位置上是占位符，原样显示', () => {
    expect(buildJoinCommand({ placeholder: '<注册令牌>' }, { ...NO_DECLARATIONS, relay: true })).toBe(
      'echo <注册令牌> | i3t join --can-relay',
    )
  })

  it('始终是一行', () => {
    for (const platform of PLATFORMS) {
      expect(buildJoinCommand(TOKEN, { exit: true, relay: true, forward: true }, platform.id)).not.toMatch(/[\r\n]/)
    }
  })

  it('Windows 上执行的是当前目录里的 i3t.exe', () => {
    expect(buildJoinCommand(TOKEN, { ...NO_DECLARATIONS, exit: true }, 'windows-x86_64')).toBe(
      `echo ${TOKEN} | .\\i3t.exe join --can-exit`,
    )
    expect(programFor('windows-x86_64')).toBe('.\\i3t.exe')
    expect(programFor('macos-aarch64')).toBe('i3t')
  })

  it('真正的令牌（i3t- 加 base64url）不需要引号', () => {
    expect(buildJoinCommand('i3t-abc_DEF-123', NO_DECLARATIONS)).toBe('echo i3t-abc_DEF-123 | i3t join')
  })

  it('带着 shell 会误解的字符时加引号，不让它变成另一条命令', () => {
    expect(buildJoinCommand('a b; rm -rf ~', NO_DECLARATIONS)).toBe(`echo 'a b; rm -rf ~' | i3t join`)
    expect(buildJoinCommand(`it's`, NO_DECLARATIONS)).toBe(`echo 'it'\\''s' | i3t join`)
    expect(buildJoinCommand('a "b"', NO_DECLARATIONS, 'windows-x86_64')).toBe('echo "a `"b`"" | .\\i3t.exe join')
  })
})

describe('其余给用户执行的命令', () => {
  it('启动节点', () => {
    expect(runCommand()).toBe('i3t run')
    expect(runCommand('windows-x86_64')).toBe('.\\i3t.exe run')
  })

  it('下载：每个平台取发布页上对应的文件', () => {
    expect(installCommands('linux-x86_64')).toEqual([
      `curl -fLo i3t ${RELEASE_BASE}/i3t-linux-x86_64`,
      'sudo install -m 755 i3t /usr/local/bin/i3t',
    ])
    expect(installCommands('macos-aarch64')[0]).toBe(`curl -fLo i3t ${RELEASE_BASE}/i3t-macos-aarch64`)
    expect(installCommands('windows-x86_64')).toEqual([`curl.exe -fLo i3t.exe ${RELEASE_BASE}/i3t-windows-x86_64.exe`])
  })

  it('声明和撤回：i3t set can-… on / off', () => {
    expect(declareCommand('exit')).toBe('i3t set can-exit on')
    expect(declareCommand('relay')).toBe('i3t set can-relay on')
    expect(declareCommand('forward')).toBe('i3t set can-forward on')
    expect(declareCommand('relay', false)).toBe('i3t set can-relay off')
  })

  it('改配置文件的写法', () => {
    expect(declareConfig('exit')).toBe('[declare]\nexit = true')
  })
})
