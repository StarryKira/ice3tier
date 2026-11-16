/**
 * 复制到剪贴板。
 *
 * `navigator.clipboard` 只在安全的上下文里可用（HTTPS，或者 localhost / 127.0.0.1）。
 * 用局域网地址加明文 HTTP 打开界面时它不存在，这时退回到选中一个临时文本框再执行复制命令的老办法。
 */
export async function copyText(text: string): Promise<boolean> {
  // 类型里它总是存在，实际在非安全上下文里是 undefined。
  const clipboard = navigator.clipboard as Clipboard | undefined
  if (clipboard !== undefined) {
    try {
      await clipboard.writeText(text)
      return true
    } catch {
      // 权限被拒绝等情况：接着试老办法。
    }
  }
  const area = document.createElement('textarea')
  area.value = text
  area.setAttribute('readonly', '')
  area.style.position = 'fixed'
  area.style.opacity = '0'
  document.body.appendChild(area)
  area.select()
  try {
    // 已经标为过时，但它是非安全上下文里唯一的办法。
    // eslint-disable-next-line @typescript-eslint/no-deprecated
    return document.execCommand('copy')
  } catch {
    return false
  } finally {
    area.remove()
  }
}
